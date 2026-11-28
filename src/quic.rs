//! The network: a server and its clients over QUIC (`quinn`, on async-io,
//! with rustls), carrying the messages of `defaria_core::net` between a
//! link in memory at each end. The game on either side sees the same
//! [`ServerLink`] or [`ClientLink`] it does in single player; a background
//! thread moves what goes in one end over the network and out the other.
//!
//! Both ends present a self-signed certificate for their key
//! (`identity`): the client's says which player it is (its id is the
//! key's hash, which the server reads from the handshake, never from what
//! the client says), and the server's is checked against the fingerprint
//! the client remembers for it ([`connect`]'s `expected`), so a server
//! whose key changed is refused. A connection's client then opens a
//! stream both ways, the control stream, and on it:
//!
//! 1. the client says hello ([`Handshake::Hello`]: the protocol and the
//!    player's name);
//! 2. the server lists its mods ([`Handshake::Mods`]; `mod_transfer`), or
//!    refuses a protocol not its own;
//! 3. the client asks for those it has neither of its own nor in its
//!    cache ([`Handshake::Fetch`]), and the server sends each
//!    ([`Handshake::Mod`]), which the client checks and caches;
//! 4. the client builds its content from exactly the server's mods (and
//!    its own `client` ones) and says its fingerprint
//!    ([`Handshake::Ready`]);
//! 5. the server welcomes it, or refuses content that isn't its own.
//!
//! Then:
//!
//! - chunks, their changes and drops, whether the client's settled, and
//!   which of its predicted edits are done (after the chunks they
//!   changed) go on a stream of their own from the server ([`bulk`]), so
//!   a lost packet of chunks doesn't hold up the rest;
//! - messages that needn't arrive (a client's inputs, which it sends
//!   again until they're applied, and where the server has its player:
//!   `reliable()` says) go as datagrams, when they fit in one;
//! - everything else goes on the control stream, in order.
//!
//! On a stream, each message is a frame: its length, a flag byte (whether
//! it's compressed: big ones, chunks mostly, are deflated) and its bytes.
//!
//! A connection says something every few seconds while it's quiet, so it
//! isn't timed out; a server takes only small frames from clients, and a
//! tiny hello within a few seconds; and either end finishes writing before
//! it closes. A stopping server closes every connection.

use std::{
    io,
    net::{SocketAddr, ToSocketAddrs},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use smol::future::FutureExt as _;

use bevy::prelude::*;
use defaria_core::{
    identity::{PlayerId, clean_name},
    net::{ClientLink, ClientMessage, ServerLink, ServerMessage, decode, encode, link},
};
use defaria_server::server::Joining;
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use serde::{Deserialize, Serialize};

use defaria_core::content::Content;

use crate::{
    identity::{CERT_NAME, Fingerprint, Identity, key_hash},
    mod_transfer::{
        Library, MAX_FILES, MAX_MOD, MAX_MODS, MAX_PATH, ModEntry, ModHash, Offer, Package,
    },
};

/// The port a server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 25_575;

/// The protocol's version: a client and server of different ones refuse
/// each other.
pub const PROTOCOL: u32 = 3;

const ALPN: &[u8] = b"defaria/3";
/// Frames bigger than this are deflated.
const COMPRESS_OVER: usize = 512;
/// The biggest frame a client accepts from its server, so a bad length
/// can't ask for all memory.
const MAX_FRAME: usize = 64 << 20;
/// The biggest frame a server accepts from a client: what clients send is
/// small, and they're not trusted.
const MAX_CLIENT_FRAME: usize = 64 << 10;
/// The biggest hello accepted, before we know who's asking.
const MAX_HANDSHAKE: usize = 1 << 10;
/// The biggest of a client's other handshake frames (which mods it
/// wants, its content).
const MAX_CLIENT_HANDSHAKE: usize = MAX_MODS * 40 + 64;
/// The biggest list of mods a client takes from a server.
const MAX_MOD_LIST: usize = MAX_MODS * 200 + 64;
/// The biggest mod a client takes from a server: the files' limit, and
/// their paths and lengths.
const MAX_MOD_FRAME: usize = MAX_MOD + MAX_FILES * (MAX_PATH + 16) + 64;
/// How long a client has to say hello, and to connect.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long fetching mods and building content may take, once a server
/// has answered (and how long a server goes on sending them).
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a server waits for each of a client's answers in the
/// handshake (which mods it wants, its content).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// The most clients a server has in its handshake at once, and from one
/// address: a handshake costs the server, and any key may try one.
const MAX_HANDSHAKES: usize = 64;
const MAX_HANDSHAKES_PER_ADDRESS: usize = 4;
/// How often each end sends something when it has nothing to say, so an
/// idle connection isn't timed out.
const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// How often the background thread looks for messages the game sent.
const POLL: Duration = Duration::from_millis(1);

/// What a client and server say first, on the control stream, in this
/// order (see the module's docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handshake {
    /// Client: who's joining.
    Hello {
        protocol: u32,
        /// The player's name, only shown (who they are is their key).
        name: String,
    },
    /// Server: the mods it plays with, in load order.
    Mods(Vec<ModEntry>),
    /// Client: the mods it needs sent, by hash.
    Fetch(Vec<ModHash>),
    /// Server: one of them.
    Mod(Package),
    /// Client: the fingerprint of the content it built.
    Ready {
        content: u64,
    },
    /// Server: play.
    Welcome,
    Refused(String),
}

/// Whether a server message goes on the bulk stream.
fn bulk(message: &ServerMessage) -> bool {
    matches!(
        message,
        ServerMessage::Chunk { .. }
            | ServerMessage::ChunkChanged { .. }
            | ServerMessage::LightChanged { .. }
            | ServerMessage::Unload(_)
            | ServerMessage::Settled(_)
            | ServerMessage::EditDone(_)
    )
}

/// A server listening for clients: each that connects, takes its mods and
/// builds the same content, comes out of [`Listener::accept`] as a player
/// joining.
pub struct Listener {
    pub address: SocketAddr,
    incoming: mpsc::Receiver<Joining>,
    endpoint: Endpoint,
}

impl Listener {
    /// The clients that connected since the last call.
    pub fn accept(&self) -> Vec<Joining> {
        self.incoming.try_iter().collect()
    }

    /// Closes every connection, telling the clients the server stopped,
    /// and waits (a moment at most) for them to hear it.
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"the server stopped");
        smol::block_on(within(Duration::from_secs(1), self.endpoint.wait_idle()));
    }
}

/// Listens on `address` (port 0 for any free one) for clients, offering
/// them the mods in `offer`, as the server whose key is `identity`.
pub fn listen(address: SocketAddr, offer: Offer, identity: &Identity) -> io::Result<Listener> {
    // Encoded and deflated once, here, not on the network's executor for
    // each client.
    let offer = Arc::new(Encoded::new(offer));
    let handshakes = Arc::new(std::sync::Mutex::new(Handshakes::default()));
    let config = server_config(identity).map_err(io::Error::other)?;
    let endpoint = Endpoint::server(config, address)?;
    let address = endpoint.local_addr()?;
    let (send, incoming) = mpsc::channel();
    let listening = endpoint.clone();
    thread::Builder::new()
        .name("server network".into())
        .spawn(move || {
            let endpoint = listening;
            smol::block_on(async move {
                while let Some(connecting) = endpoint.accept().await {
                    let send = send.clone();
                    let offer = offer.clone();
                    let handshakes = handshakes.clone();
                    smol::spawn(async move {
                        match connecting.await {
                            Ok(connection) => {
                                let Some(slot) = Handshakes::start(&handshakes, &connection) else {
                                    connection.close(1u32.into(), b"the server is busy");
                                    return;
                                };
                                serve(connection, &offer, slot, send).await
                            }
                            Err(error) => warn!("a client couldn't connect: {error}"),
                        }
                    })
                    .detach();
                }
            });
        })?;
    info!(
        "listening on {address}, as the server with fingerprint {}",
        Fingerprint(identity.id.0)
    );
    Ok(Listener {
        address,
        incoming,
        endpoint,
    })
}

/// Why [`connect`] didn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The server's key isn't the one remembered for it: it may be
    /// someone else's server, pretending.
    ChangedKey {
        address: String,
        expected: Fingerprint,
        found: Fingerprint,
    },
    /// Anything else: no answer, refused, a broken connection.
    Failed(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChangedKey {
                address,
                expected,
                found,
            } => write!(
                f,
                "the server at {address} isn't the one played on before: its key changed \
                 (fingerprint {found}, expected {expected}). It may be someone else's \
                 server; if its owner says they changed its key, forget it (remove it from \
                 servers.json5 in the game's folder) and connect again"
            ),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

impl From<String> for ConnectError {
    fn from(reason: String) -> Self {
        Self::Failed(reason)
    }
}

/// A connection made by [`connect`].
pub struct Connected {
    /// The link to the server.
    pub link: ClientLink,
    /// The server's key's fingerprint, to remember.
    pub fingerprint: Fingerprint,
    /// The content built from the server's mods, to play with.
    pub content: Content,
    /// The mods that had to be fetched (the rest were the client's own, or
    /// cached).
    pub fetched: Vec<ModHash>,
}

/// Connects to the server at `address` (a host and port) as `identity`,
/// named `name`, takes the server's mods (from `library` where it has
/// them), and returns the link to it, and the content built from them,
/// once the server has accepted. Blocks until then. With `expected`, a
/// server whose key's fingerprint isn't it is refused
/// ([`ConnectError::ChangedKey`]); without, any is trusted (the first
/// connection), and the caller remembers the one it got. A name with
/// several addresses (`localhost`: IPv6 and IPv4) is tried at each, IPv4
/// first, until one answers.
pub fn connect(
    address: &str,
    mut library: Library,
    identity: &Identity,
    name: &str,
    expected: Option<Fingerprint>,
) -> Result<Connected, ConnectError> {
    let given = address.to_string();
    let mut addresses: Vec<SocketAddr> = address
        .to_socket_addrs()
        .map_err(|error| format!("{address}: {error}"))?
        .collect();
    addresses.sort_by_key(SocketAddr::is_ipv6);
    if addresses.is_empty() {
        return Err(format!("{address}: no such address").into());
    }
    let seen = Arc::new(std::sync::Mutex::new(None));
    let config = client_config(identity, expected, seen.clone())?;
    let name = name.to_string();
    let (result_send, result) = mpsc::channel();
    thread::Builder::new()
        .name("client network".into())
        .spawn(move || {
            smol::block_on(async move {
                let mut error = String::new();
                for address in addresses {
                    let timeout = async {
                        smol::Timer::after(HANDSHAKE_TIMEOUT).await;
                        Err(format!("{address} didn't answer"))
                    };
                    let (connection, mut control, mods) =
                        match open(address, &name, config.clone()).or(timeout).await {
                            Ok(opened) => opened,
                            Err(this) => {
                                error = this;
                                continue;
                            }
                        };
                    // It answered: no other address is tried.
                    let timeout = async {
                        smol::Timer::after(TRANSFER_TIMEOUT).await;
                        Err("fetching the server's mods took too long".to_string())
                    };
                    match take_mods(&mut control, mods, &mut library)
                        .or(timeout)
                        .await
                    {
                        Ok(content) => {
                            let (game_end, server_end) = link();
                            let fetched = std::mem::take(&mut library.fetched);
                            let _ = result_send.send(Ok((game_end, content, fetched)));
                            play(connection, control, server_end).await;
                        }
                        Err(this) => {
                            connection.close(1u32.into(), b"refused");
                            let _ = result_send.send(Err(this));
                        }
                    }
                    return;
                }
                let _ = result_send.send(Err(error));
            });
        })
        .map_err(|error| error.to_string())?;
    let outcome = result
        .recv()
        .map_err(|_| "the network thread stopped".to_string())?;
    let found = *seen.lock().unwrap_or_else(|e| e.into_inner());
    match (outcome, found) {
        (Ok((link, content, fetched)), Some(fingerprint)) => Ok(Connected {
            link,
            fingerprint,
            content,
            fetched,
        }),
        (Ok(_), None) => Err("the server showed no certificate".to_string().into()),
        (Err(_), Some(found)) if expected.is_some_and(|expected| expected != found) => {
            Err(ConnectError::ChangedKey {
                address: given,
                expected: expected.unwrap(),
                found,
            })
        }
        (Err(error), _) => Err(error.into()),
    }
}

/// The client's side of setting up: connects, says hello, and waits for
/// the server's mods.
async fn open(
    address: SocketAddr,
    name: &str,
    config: ClientConfig,
) -> Result<(Connection, (SendStream, RecvStream), Vec<ModEntry>), String> {
    let bind: SocketAddr = if address.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let mut endpoint = Endpoint::client(bind).map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(config);
    let connection = endpoint
        .connect(address, CERT_NAME)
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| format!("can't connect to {address}: {error}"))?;
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|error| error.to_string())?;
    let hello = Handshake::Hello {
        protocol: PROTOCOL,
        name: name.to_string(),
    };
    write_frame(&mut send, &encode(&hello))
        .await
        .map_err(|error| error.to_string())?;
    let answer = read_frame(&mut receive, MAX_MOD_LIST)
        .await
        .map_err(|error| match connection.close_reason() {
            // Turned away before any answer (busy, say): why.
            Some(quinn::ConnectionError::ApplicationClosed(close)) => format!(
                "the server closed the connection: {}",
                String::from_utf8_lossy(&close.reason)
            ),
            _ => format!("the server didn't answer: {error}"),
        })?;
    match decode::<Handshake>(&answer)? {
        Handshake::Mods(mods) => Ok((connection, (send, receive), mods)),
        Handshake::Refused(reason) => Err(format!("the server refused: {reason}")),
        _ => Err("the server answered oddly".into()),
    }
}

/// The client's side of the server's mods: asks for those `library`
/// lacks, checks and keeps each as it arrives, builds the content from
/// them, and waits for the server to take it.
async fn take_mods(
    (send, receive): &mut (SendStream, RecvStream),
    mods: Vec<ModEntry>,
    library: &mut Library,
) -> Result<Content, String> {
    let missing = library.missing(&mods)?;
    write_frame(send, &encode(&Handshake::Fetch(missing.clone())))
        .await
        .map_err(|error| error.to_string())?;
    if !missing.is_empty() {
        info!(
            "fetching {} of the server's {} mods",
            missing.len(),
            mods.len()
        );
    }
    for hash in missing {
        let entry = mods
            .iter()
            .find(|entry| entry.hash == hash)
            .expect("asked for an offered mod");
        let frame = read_frame(receive, MAX_MOD_FRAME)
            .await
            .map_err(|error| format!("mod {} didn't arrive: {error}", entry.id))?;
        let package = match decode::<Handshake>(&frame)? {
            Handshake::Mod(package) => package,
            Handshake::Refused(reason) => return Err(format!("the server refused: {reason}")),
            _ => return Err("the server answered oddly".into()),
        };
        library
            .receive(entry, package)
            .map_err(|error| format!("the server sent a bad copy of mod {}: {error}", entry.id))?;
    }
    let content = library.content(&mods)?;
    let ready = Handshake::Ready {
        content: content.fingerprint(),
    };
    write_frame(send, &encode(&ready))
        .await
        .map_err(|error| error.to_string())?;
    let answer = read_frame(receive, MAX_HANDSHAKE)
        .await
        .map_err(|error| format!("the server didn't answer: {error}"))?;
    match decode::<Handshake>(&answer)? {
        Handshake::Welcome => Ok(content),
        Handshake::Refused(reason) => Err(format!("the server refused: {reason}")),
        _ => Err("the server answered oddly".into()),
    }
}

/// An [`Offer`] as it's sent: the list of mods and each mod as the frame
/// that sends it (encoded and deflated once, for every client that asks),
/// and the content's fingerprint.
struct Encoded {
    list: Vec<u8>,
    frames: std::collections::HashMap<ModHash, Vec<u8>>,
    content: u64,
}

impl Encoded {
    fn new(offer: Offer) -> Self {
        let frames = (offer.entries.iter())
            .filter_map(|entry| {
                let package = offer.package(entry.hash)?;
                let bytes = encode(&Handshake::Mod((**package).clone()));
                Some((entry.hash, frame(&bytes)))
            })
            .collect();
        Self {
            list: frame(&encode(&Handshake::Mods(offer.entries.clone()))),
            frames,
            content: offer.content,
        }
    }
}

/// The clients in a server's handshake, in all and by address.
#[derive(Default)]
struct Handshakes {
    total: usize,
    by_address: std::collections::HashMap<std::net::IpAddr, usize>,
}

/// A client's place in [`Handshakes`], given up when it's dropped.
struct HandshakeSlot {
    handshakes: Arc<std::sync::Mutex<Handshakes>>,
    address: std::net::IpAddr,
}

impl Handshakes {
    /// A place for `connection`'s handshake, or `None` if the server has
    /// as many as it takes, in all or from that address.
    fn start(
        handshakes: &Arc<std::sync::Mutex<Self>>,
        connection: &Connection,
    ) -> Option<HandshakeSlot> {
        let address = connection.remote_address().ip();
        let mut all = handshakes.lock().unwrap_or_else(|e| e.into_inner());
        let from = all.by_address.get(&address).copied().unwrap_or(0);
        if all.total >= MAX_HANDSHAKES || from >= MAX_HANDSHAKES_PER_ADDRESS {
            warn!("turned away {address}: too many handshakes at once");
            return None;
        }
        all.total += 1;
        all.by_address.insert(address, from + 1);
        Some(HandshakeSlot {
            handshakes: handshakes.clone(),
            address,
        })
    }
}

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        let mut all = self.handshakes.lock().unwrap_or_else(|e| e.into_inner());
        all.total -= 1;
        if let Some(count) = all.by_address.get_mut(&self.address) {
            *count -= 1;
            if *count == 0 {
                all.by_address.remove(&self.address);
            }
        }
    }
}

/// The id of the player whose certificate `connection`'s client showed.
fn player_id(connection: &Connection) -> Option<PlayerId> {
    let certs = connection
        .peer_identity()?
        .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    key_hash(certs.first()?).ok().map(PlayerId)
}

/// The server's side of a connection: hears the client out, sends it the
/// mods it asks for, and if it's welcome, carries its messages until
/// either side goes.
async fn serve(
    connection: Connection,
    offer: &Encoded,
    slot: HandshakeSlot,
    accepted: mpsc::Sender<Joining>,
) {
    // Checked in the handshake: a client without a certificate can't get
    // this far.
    let Some(id) = player_id(&connection) else {
        connection.close(1u32.into(), b"no certificate");
        return;
    };
    let Ok((mut send, mut receive)) = connection.accept_bi().await else {
        return;
    };
    let no_hello = async {
        smol::Timer::after(HANDSHAKE_TIMEOUT).await;
        Err(io::Error::other("no hello"))
    };
    let Ok(hello) = read_frame(&mut receive, MAX_HANDSHAKE).or(no_hello).await else {
        connection.close(1u32.into(), b"no hello");
        return;
    };
    let mut name = String::new();
    let refusal = match decode::<Handshake>(&hello) {
        Ok(Handshake::Hello {
            protocol,
            name: theirs_called,
        }) => {
            name = clean_name(&theirs_called);
            if protocol != PROTOCOL {
                Some(format!(
                    "this server speaks protocol {PROTOCOL}, the game {protocol}"
                ))
            } else {
                None
            }
        }
        _ => Some("that's not a hello".to_string()),
    };
    let refusal = match refusal {
        Some(reason) => Some(reason),
        None => {
            let timeout = async {
                smol::Timer::after(TRANSFER_TIMEOUT).await;
                Err("it took too long to take the server's mods".to_string())
            };
            send_mods(&mut send, &mut receive, offer)
                .or(timeout)
                .await
                .err()
        }
    };
    if let Some(reason) = refusal {
        info!("refused {}: {reason}", connection.remote_address());
        let _ = write_frame(&mut send, &encode(&Handshake::Refused(reason))).await;
        let _ = send.finish();
        // Let the answer get there: the client closes once it's read it.
        within(HANDSHAKE_TIMEOUT, connection.closed()).await;
        return;
    }
    if write_frame(&mut send, &encode(&Handshake::Welcome))
        .await
        .is_err()
    {
        return;
    }
    let Ok(bulk_stream) = connection.open_uni().await else {
        return;
    };
    // In: no longer a handshake.
    drop(slot);
    info!(
        "{name} ({id}) connected from {}",
        connection.remote_address()
    );
    let (net_end, game_end) = link();
    if accepted
        .send(Joining {
            id,
            name,
            link: game_end,
        })
        .is_err()
    {
        return;
    }
    carry_for_server(connection, (send, receive), bulk_stream, net_end).await;
}

/// The server's side of its mods: lists them, sends those the client
/// asks for, and checks the content it built from them is the server's.
/// The error is why the client's refused.
async fn send_mods(
    send: &mut SendStream,
    receive: &mut RecvStream,
    offer: &Encoded,
) -> Result<(), String> {
    let broken = |error: io::Error| format!("the handshake broke: {error}");
    send.write_all(&offer.list)
        .await
        .map_err(io::Error::other)
        .map_err(broken)?;
    let frame = answer(receive).await?;
    let Ok(Handshake::Fetch(wanted)) = decode::<Handshake>(&frame) else {
        return Err("that's not which mods to send".into());
    };
    let mut sent = std::collections::HashSet::new();
    for hash in wanted {
        let Some(frame) = offer.frames.get(&hash) else {
            return Err(format!("asked for a mod this server doesn't have ({hash})"));
        };
        if !sent.insert(hash) {
            return Err(format!("asked for a mod twice ({hash})"));
        }
        send.write_all(frame)
            .await
            .map_err(io::Error::other)
            .map_err(broken)?;
    }
    let frame = answer(receive).await?;
    match decode::<Handshake>(&frame) {
        Ok(Handshake::Ready { content }) if content == offer.content => Ok(()),
        Ok(Handshake::Ready { .. }) => Err(
            "the game built different blocks and items from this server's mods: \
             are the game and the server the same version?"
                .into(),
        ),
        _ => Err("that's not ready".into()),
    }
}

/// A client's next answer in the handshake, if it comes within
/// [`ANSWER_TIMEOUT`].
async fn answer(receive: &mut RecvStream) -> Result<Vec<u8>, String> {
    let read = async {
        read_frame(receive, MAX_CLIENT_HANDSHAKE)
            .await
            .map_err(|error| format!("the handshake broke: {error}"))
    };
    let late = async {
        smol::Timer::after(ANSWER_TIMEOUT).await;
        Err("no answer in time".to_string())
    };
    read.or(late).await
}

/// Carries a server's messages to its client and the client's back, until
/// either goes.
async fn carry_for_server(
    connection: Connection,
    (mut control, mut from_client): (SendStream, RecvStream),
    mut bulk_stream: SendStream,
    mut game: ClientLink,
) {
    let incoming = {
        let connection = connection.clone();
        let (to_game, from_net) = mpsc::channel::<ClientMessage>();
        let datagrams = {
            let connection = connection.clone();
            let to_game = to_game.clone();
            smol::spawn(async move {
                while let Ok(bytes) = connection.read_datagram().await {
                    if let Ok(message) = decode::<ClientMessage>(&bytes) {
                        let _ = to_game.send(message);
                    }
                }
            })
        };
        let stream = smol::spawn(async move {
            loop {
                let frame = match read_frame(&mut from_client, MAX_CLIENT_FRAME).await {
                    Ok(frame) => frame,
                    Err(error) => {
                        if connection.close_reason().is_none() {
                            warn!("a client's stream broke: {error}");
                            connection.close(1u32.into(), b"bad frame");
                        }
                        break;
                    }
                };
                match decode::<ClientMessage>(&frame) {
                    Ok(message) => {
                        if to_game.send(message).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        warn!("a client sent something odd: {error}");
                        connection.close(1u32.into(), b"bad message");
                        break;
                    }
                }
            }
        });
        (from_net, datagrams, stream)
    };
    let (from_net, _datagrams, _stream) = incoming;
    loop {
        for message in from_net.try_iter() {
            game.send(message);
        }
        for message in game.receive() {
            let bytes = encode(&message);
            if !message.reliable() && fits(&connection, &bytes) {
                let _ = connection.send_datagram(bytes.into());
                continue;
            }
            let stream = if bulk(&message) {
                &mut bulk_stream
            } else {
                &mut control
            };
            if write_frame(stream, &bytes).await.is_err() {
                return;
            }
        }
        if game.is_closed() || connection.close_reason().is_some() {
            goodbye(&connection, [&mut control, &mut bulk_stream]).await;
            return;
        }
        smol::Timer::after(POLL).await;
    }
}

/// Carries a client's messages to its server and the server's back, until
/// either goes.
async fn play(
    connection: Connection,
    (mut control, mut from_server): (SendStream, RecvStream),
    mut game: ServerLink,
) {
    let (to_game, from_net) = mpsc::channel::<ServerMessage>();
    let _datagrams = {
        let connection = connection.clone();
        let to_game = to_game.clone();
        smol::spawn(async move {
            while let Ok(bytes) = connection.read_datagram().await {
                if let Ok(message) = decode::<ServerMessage>(&bytes) {
                    let _ = to_game.send(message);
                }
            }
        })
    };
    let _control = {
        let to_game = to_game.clone();
        smol::spawn(async move {
            while let Ok(frame) = read_frame(&mut from_server, MAX_FRAME).await {
                let Ok(message) = decode::<ServerMessage>(&frame) else {
                    break;
                };
                if to_game.send(message).is_err() {
                    break;
                }
            }
        })
    };
    let _bulk = {
        let connection = connection.clone();
        smol::spawn(async move {
            let Ok(mut bulk_stream) = connection.accept_uni().await else {
                return;
            };
            while let Ok(frame) = read_frame(&mut bulk_stream, MAX_FRAME).await {
                let Ok(message) = decode::<ServerMessage>(&frame) else {
                    break;
                };
                if to_game.send(message).is_err() {
                    break;
                }
            }
        })
    };
    loop {
        for message in from_net.try_iter() {
            game.send(message);
        }
        for message in game.receive() {
            let bytes = encode(&message);
            if !message.reliable() && fits(&connection, &bytes) {
                let _ = connection.send_datagram(bytes.into());
            } else if write_frame(&mut control, &bytes).await.is_err() {
                return;
            }
        }
        if game.is_closed() || connection.close_reason().is_some() {
            goodbye(&connection, [&mut control]).await;
            return;
        }
        smol::Timer::after(POLL).await;
    }
}

/// Whether `bytes` fit in a datagram on `connection`.
fn fits(connection: &Connection, bytes: &[u8]) -> bool {
    connection
        .max_datagram_size()
        .is_some_and(|max| bytes.len() <= max)
}

/// What `future` comes to, or nothing if it takes longer than `limit`.
async fn within<T>(limit: Duration, future: impl Future<Output = T>) -> Option<T> {
    async { Some(future.await) }
        .or(async {
            smol::Timer::after(limit).await;
            None
        })
        .await
}

/// Ends a connection, once what's been written on its streams has gone (or
/// a moment has passed): closing drops anything still unsent.
async fn goodbye<const N: usize>(connection: &Connection, streams: [&mut SendStream; N]) {
    if connection.close_reason().is_none() {
        for stream in streams {
            let _ = stream.finish();
            within(Duration::from_secs(1), stream.stopped()).await;
        }
    }
    connection.close(0u32.into(), b"bye");
}

/// Writes a frame ([`frame`]).
async fn write_frame(stream: &mut SendStream, bytes: &[u8]) -> io::Result<()> {
    stream
        .write_all(&frame(bytes))
        .await
        .map_err(io::Error::other)
}

/// A frame's bytes: its length, whether it's deflated, and its bytes.
fn frame(bytes: &[u8]) -> Vec<u8> {
    let (flag, payload) = if bytes.len() > COMPRESS_OVER {
        (1u8, miniz_oxide::deflate::compress_to_vec(bytes, 1))
    } else {
        (0u8, bytes.to_vec())
    };
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    frame.push(flag);
    frame.extend_from_slice(&payload);
    frame
}

/// Reads a frame's bytes, inflated if they were deflated, refusing one of
/// more than `max` bytes either way.
async fn read_frame(stream: &mut RecvStream, max: usize) -> io::Result<Vec<u8>> {
    let mut length = [0; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(io::Error::other)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > max + 1 {
        return Err(io::Error::other(format!("a frame of {length} bytes")));
    }
    let mut frame = vec![0; length];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(io::Error::other)?;
    match frame[0] {
        0 => Ok(frame.split_off(1)),
        1 => miniz_oxide::inflate::decompress_to_vec_with_limit(&frame[1..], max)
            .map_err(|error| io::Error::other(format!("{error:?}"))),
        flag => Err(io::Error::other(format!("a frame flagged {flag}"))),
    }
}

/// A server's config: a certificate for its key, and every client asked
/// for one of theirs.
fn server_config(identity: &Identity) -> Result<ServerConfig, String> {
    let (cert, key) = identity.certificate()?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_client_cert_verifier(Arc::new(AnyClient(provider)))
        .with_single_cert(vec![cert], key)
        .map_err(|error| error.to_string())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicServerConfig::try_from(tls).map_err(|error| error.to_string())?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

/// A client's config: its certificate, and the server's checked against
/// `expected` (any trusted without), the fingerprint it had put in
/// `seen`.
fn client_config(
    identity: &Identity,
    expected: Option<Fingerprint>,
    seen: Arc<std::sync::Mutex<Option<Fingerprint>>>,
) -> Result<ClientConfig, String> {
    let (cert, key) = identity.certificate()?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = KnownServer {
        provider: provider.clone(),
        expected,
        seen,
    };
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(vec![cert], key)
        .map_err(|error| error.to_string())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicClientConfig::try_from(tls).map_err(|error| error.to_string())?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

/// How both ends' connections behave: kept alive while they're quiet.
fn transport() -> Arc<quinn::TransportConfig> {
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    Arc::new(transport)
}

/// Checks a server's certificate is for the key remembered for it, or
/// takes any the first time, noting which it was. Self-signed: there's no
/// authority to ask, so who a server is is its key, as with SSH. The
/// handshake's signature (checked below) proves the server holds it.
#[derive(Debug)]
struct KnownServer {
    provider: Arc<rustls::crypto::CryptoProvider>,
    expected: Option<Fingerprint>,
    seen: Arc<std::sync::Mutex<Option<Fingerprint>>>,
}

impl rustls::client::danger::ServerCertVerifier for KnownServer {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let found = Fingerprint(key_hash(end_entity).map_err(rustls::Error::General)?);
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(found);
        match self.expected {
            Some(expected) if expected != found => Err(rustls::Error::General(
                "the server's key isn't the one remembered for it".into(),
            )),
            _ => Ok(rustls::client::danger::ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Takes any client's self-signed certificate, so long as the client
/// proves it holds the key (the handshake's signature, checked below):
/// which key it is says which player it is, and any key may play.
#[derive(Debug)]
struct AnyClient(Arc<rustls::crypto::CryptoProvider>);

impl rustls::server::danger::ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        key_hash(end_entity).map_err(rustls::Error::General)?;
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use defaria_core::{
        content::load::ModFiles,
        light::ChunkLight,
        world::{Chunk, ChunkCoord},
    };

    use super::*;

    fn local() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// What a server with one tiny mod offers.
    fn offer() -> Offer {
        let tiny = ModFiles::new("mods/tiny", r#"{ id: "tiny", name: "Tiny" }"#, &[]);
        Offer::new(&Content::from_mods(vec![tiny])).unwrap()
    }

    /// A server, listening on this machine with a key of its own.
    fn server() -> Listener {
        listen(local(), offer(), &Identity::generate()).unwrap()
    }

    /// Connects to `address` as `identity`, with no mods of its own.
    fn connect_to(
        address: &str,
        identity: &Identity,
        expected: Option<Fingerprint>,
    ) -> Result<Connected, ConnectError> {
        connect(address, Library::empty(None), identity, "Tester", expected)
    }

    /// Connects to `listener` as `identity`, trusting any server.
    fn join(listener: &Listener, identity: &Identity) -> Result<ClientLink, String> {
        connect_to(&listener.address.to_string(), identity, None)
            .map(|connected| connected.link)
            .map_err(|error| error.to_string())
    }

    /// Waits for what `receive` gets to be `count` messages long.
    fn wait_for<T>(mut receive: impl FnMut() -> Vec<T>, count: usize) -> Vec<T> {
        let mut got = Vec::new();
        for _ in 0..5000 {
            got.extend(receive());
            if got.len() >= count {
                return got;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("only {} of {count} arrived", got.len());
    }

    /// A client connects over UDP on this machine; messages go both ways,
    /// in order on each stream, a big one (deflated on its way) too, and a
    /// player's inputs as a datagram.
    #[test]
    fn messages_cross_the_network() {
        let listener = server();
        let me = Identity::generate();
        let mut client = join(&listener, &me).unwrap();
        let joining = wait_for(|| listener.accept(), 1).pop().unwrap();
        // Who it is comes from its key; what it's called from its hello.
        assert_eq!(joining.id, me.id);
        assert_eq!(joining.name, "Tester");
        let mut server = joining.link;

        client.send(ClientMessage::ToggleGameMode);
        client.send(ClientMessage::Mining(true));
        assert_eq!(
            wait_for(|| server.receive(), 2),
            [ClientMessage::ToggleGameMode, ClientMessage::Mining(true)]
        );

        let mut chunk = Chunk::default();
        for i in 0..16 {
            chunk.set(i, i, i, defaria_core::content::BlockState(i as u16 + 1));
        }
        let big = ServerMessage::Chunk {
            coord: ChunkCoord(IVec3::new(1, -2, 3)),
            chunk,
            light: ChunkLight::default(),
        };
        assert!(encode(&big).len() > COMPRESS_OVER);
        server.send(ServerMessage::Settled(false));
        server.send(big.clone());
        server.send(ServerMessage::CloseScreen);
        let got = wait_for(|| client.receive(), 3);
        // Each stream in order; the two streams in either.
        let bulk: Vec<_> = got.iter().filter(|m| bulk(m)).cloned().collect();
        assert_eq!(bulk, [ServerMessage::Settled(false), big]);
        assert!(got.contains(&ServerMessage::CloseScreen));

        client.send(ClientMessage::Inputs(vec![defaria_core::net::Input {
            tick: 1,
            controls: default(),
            look: default(),
            teleport: None,
        }]));
        assert!(matches!(
            wait_for(|| server.receive(), 1)[0],
            ClientMessage::Inputs(_)
        ));

        // The client goes: the server's end sees it.
        drop(client);
        for _ in 0..5000 {
            server.receive();
            if server.is_closed() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the server never saw the client go");
    }

    /// A client that builds other content from the server's mods (another
    /// version of the game, baking them differently) is refused, and told
    /// why.
    #[test]
    fn different_content_is_refused() {
        let mut offer = offer();
        offer.content ^= 1;
        let listener = listen(local(), offer, &Identity::generate()).unwrap();
        let error = join(&listener, &Identity::generate()).err().unwrap();
        assert!(error.contains("same version"), "{error}");
        assert!(listener.accept().is_empty());
    }

    /// A client that asks for a mod the server didn't offer is refused.
    #[test]
    fn asking_for_a_mod_not_offered_is_refused() {
        let listener = server();
        let config = client_config(&Identity::generate(), None, Default::default()).unwrap();
        let address = listener.address;
        let error = smol::block_on(async move {
            let (_connection, (mut send, mut receive), _) =
                open(address, "Tester", config).await.unwrap();
            let fetch = Handshake::Fetch(vec![ModHash([7; 32])]);
            write_frame(&mut send, &encode(&fetch)).await.unwrap();
            let answer = read_frame(&mut receive, MAX_HANDSHAKE).await.unwrap();
            decode::<Handshake>(&answer).unwrap()
        });
        let Handshake::Refused(reason) = error else {
            panic!("{error:?}");
        };
        assert!(reason.contains("doesn't have"), "{reason}");
        assert!(listener.accept().is_empty());
    }

    /// A server takes only a few unfinished handshakes from one address
    /// at a time, and more once they're done; one that says nothing more
    /// after its hello is dropped in time.
    #[test]
    fn handshakes_are_held_to_a_few_an_address() {
        let listener = server();
        let address = listener.address;
        let config = client_config(&Identity::generate(), None, Default::default()).unwrap();
        // Clients that connect, say hello, and then nothing.
        let silent: Vec<_> = smol::block_on(async {
            let mut silent = Vec::new();
            for _ in 0..MAX_HANDSHAKES_PER_ADDRESS {
                silent.push(open(address, "Quiet", config.clone()).await.unwrap());
            }
            silent
        });
        let error = join(&listener, &Identity::generate()).err().unwrap();
        assert!(error.contains("busy"), "{error}");
        // Once they're gone, there's room again.
        drop(silent);
        let mut joined = None;
        for _ in 0..200 {
            if let Ok(link) = join(&listener, &Identity::generate()) {
                joined = Some(link);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(joined.is_some(), "never had room again");
    }

    /// A client that stops answering mid-handshake is refused once
    /// [`ANSWER_TIMEOUT`] has passed, and gives up its place.
    #[test]
    fn a_silent_client_is_refused_in_time() {
        let listener = server();
        let address = listener.address;
        let config = client_config(&Identity::generate(), None, Default::default()).unwrap();
        let answer = smol::block_on(async move {
            let (_connection, (_send, mut receive), _) =
                open(address, "Quiet", config).await.unwrap();
            let started = std::time::Instant::now();
            let frame = read_frame(&mut receive, MAX_HANDSHAKE).await.unwrap();
            (decode::<Handshake>(&frame).unwrap(), started.elapsed())
        });
        let (Handshake::Refused(reason), waited) = answer else {
            panic!("{answer:?}");
        };
        assert!(reason.contains("in time"), "{reason}");
        assert!(
            waited >= ANSWER_TIMEOUT - Duration::from_secs(1),
            "{waited:?}"
        );
        assert!(waited < ANSWER_TIMEOUT * 2, "{waited:?}");
    }

    /// A server that stops tells its clients, who see their link close.
    #[test]
    fn clients_see_the_server_stop() {
        let listener = server();
        let mut client = join(&listener, &Identity::generate()).unwrap();
        let _server = wait_for(|| listener.accept(), 1);
        listener.close();
        for _ in 0..5000 {
            client.receive();
            if client.is_closed() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the client never saw the server stop");
    }

    /// Two installs are two players, and the same install is the same
    /// player each time it connects.
    #[test]
    fn players_are_who_their_keys_say() {
        let listener = server();
        let (first, second) = (Identity::generate(), Identity::generate());
        let _a = join(&listener, &first).unwrap();
        let _b = join(&listener, &second).unwrap();
        let mut ids: Vec<PlayerId> = wait_for(|| listener.accept(), 2)
            .into_iter()
            .map(|joining| joining.id)
            .collect();
        ids.sort();
        let mut expected = vec![first.id, second.id];
        expected.sort();
        assert_eq!(ids, expected);
        assert_ne!(first.id, second.id);
        let _again = join(&listener, &first).unwrap();
        assert_eq!(wait_for(|| listener.accept(), 1)[0].id, first.id);
    }

    /// A client learns the server's fingerprint on first connecting;
    /// connecting expecting it works, and a server whose key isn't the one
    /// expected (another server at the same address) is refused, saying
    /// so.
    #[test]
    fn a_changed_server_key_is_refused() {
        let key = Identity::generate();
        let listener = listen(local(), offer(), &key).unwrap();
        let address = listener.address.to_string();
        let me = Identity::generate();
        let first = connect_to(&address, &me, None).unwrap();
        assert_eq!(first.fingerprint, Fingerprint(key.id.0));
        let again = connect_to(&address, &me, Some(first.fingerprint));
        assert!(again.is_ok());
        let _ = wait_for(|| listener.accept(), 2);
        drop(listener);

        // A server with another key, where that one was.
        let impostor = listen(local(), offer(), &Identity::generate()).unwrap();
        let address = impostor.address.to_string();
        let error = connect_to(&address, &me, Some(first.fingerprint))
            .err()
            .unwrap();
        let ConnectError::ChangedKey {
            expected, found, ..
        } = &error
        else {
            panic!("{error}");
        };
        assert_eq!(*expected, first.fingerprint);
        assert_ne!(*found, first.fingerprint);
        assert!(error.to_string().contains("key changed"), "{error}");
        assert!(impostor.accept().is_empty());
    }
}
