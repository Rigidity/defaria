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
//! stream both ways, the control stream, and says hello
//! ([`Handshake::Hello`]: the protocol, its content's fingerprint and the
//! player's name); the server answers on it, accepting or refusing. Then:
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

use crate::identity::{CERT_NAME, Fingerprint, Identity, key_hash};

/// The port a server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 25_575;

/// The protocol's version: a client and server of different ones refuse
/// each other.
pub const PROTOCOL: u32 = 2;

const ALPN: &[u8] = b"defaria/2";
/// Frames bigger than this are deflated.
const COMPRESS_OVER: usize = 512;
/// The biggest frame a client accepts from its server, so a bad length
/// can't ask for all memory.
const MAX_FRAME: usize = 64 << 20;
/// The biggest frame a server accepts from a client: what clients send is
/// small, and they're not trusted.
const MAX_CLIENT_FRAME: usize = 64 << 10;
/// The biggest handshake accepted, before we know who's asking.
const MAX_HANDSHAKE: usize = 1 << 10;
/// How long a client has to say hello, and to connect.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often each end sends something when it has nothing to say, so an
/// idle connection isn't timed out.
const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// How often the background thread looks for messages the game sent.
const POLL: Duration = Duration::from_millis(1);

/// What a client and server say first, on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handshake {
    Hello {
        protocol: u32,
        content: u64,
        /// The player's name, only shown (who they are is their key).
        name: String,
    },
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

/// A server listening for clients: each that connects, and says it has the
/// same content, comes out of [`Listener::accept`] as a player joining.
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

/// Listens on `address` (port 0 for any free one) for clients whose
/// content has the fingerprint `content`, as the server whose key is
/// `identity`.
pub fn listen(address: SocketAddr, content: u64, identity: &Identity) -> io::Result<Listener> {
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
                    smol::spawn(async move {
                        match connecting.await {
                            Ok(connection) => serve(connection, content, send).await,
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
}

/// Connects to the server at `address` (a host and port) as `identity`,
/// named `name`, saying our content has the fingerprint `content`, and
/// returns the link to it once the server has accepted. Blocks until
/// then. With `expected`, a server whose key's fingerprint isn't it is
/// refused ([`ConnectError::ChangedKey`]); without, any is trusted (the
/// first connection), and the caller remembers the one it got. A name
/// with several addresses (`localhost`: IPv6 and IPv4) is tried at each,
/// IPv4 first.
pub fn connect(
    address: &str,
    content: u64,
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
                    match open(address, content, &name, config.clone())
                        .or(timeout)
                        .await
                    {
                        Ok((connection, control, server_end, game_end)) => {
                            let _ = result_send.send(Ok(game_end));
                            play(connection, control, server_end).await;
                            return;
                        }
                        Err(this) => error = this,
                    }
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
        (Ok(link), Some(fingerprint)) => Ok(Connected { link, fingerprint }),
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
/// the answer.
async fn open(
    address: SocketAddr,
    content: u64,
    name: &str,
    config: ClientConfig,
) -> Result<(Connection, (SendStream, RecvStream), ServerLink, ClientLink), String> {
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
        content,
        name: name.to_string(),
    };
    write_frame(&mut send, &encode(&hello))
        .await
        .map_err(|error| error.to_string())?;
    let answer = read_frame(&mut receive, MAX_HANDSHAKE)
        .await
        .map_err(|error| format!("the server didn't answer: {error}"))?;
    match decode::<Handshake>(&answer)? {
        Handshake::Welcome => {}
        Handshake::Refused(reason) => return Err(format!("the server refused: {reason}")),
        Handshake::Hello { .. } => return Err("the server answered oddly".into()),
    }
    let (game_end, server_end) = link();
    Ok((connection, (send, receive), server_end, game_end))
}

/// The id of the player whose certificate `connection`'s client showed.
fn player_id(connection: &Connection) -> Option<PlayerId> {
    let certs = connection
        .peer_identity()?
        .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    key_hash(certs.first()?).ok().map(PlayerId)
}

/// The server's side of a connection: hears the client out, and if it's
/// welcome, carries its messages until either side goes.
async fn serve(connection: Connection, content: u64, accepted: mpsc::Sender<Joining>) {
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
            content: theirs,
            name: theirs_called,
        }) => {
            name = clean_name(&theirs_called);
            if protocol != PROTOCOL {
                Some(format!(
                    "this server speaks protocol {PROTOCOL}, the game {protocol}"
                ))
            } else if theirs != content {
                Some("the game's mods aren't the server's".to_string())
            } else {
                None
            }
        }
        _ => Some("that's not a hello".to_string()),
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

/// Writes a frame: its length, whether it's deflated, and its bytes.
async fn write_frame(stream: &mut SendStream, bytes: &[u8]) -> io::Result<()> {
    let (flag, payload) = if bytes.len() > COMPRESS_OVER {
        (1u8, miniz_oxide::deflate::compress_to_vec(bytes, 1))
    } else {
        (0u8, bytes.to_vec())
    };
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    frame.push(flag);
    frame.extend_from_slice(&payload);
    stream.write_all(&frame).await.map_err(io::Error::other)
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
        light::ChunkLight,
        world::{Chunk, ChunkCoord},
    };

    use super::*;

    fn local() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// A server, listening on this machine with a key of its own.
    fn server(content: u64) -> Listener {
        listen(local(), content, &Identity::generate()).unwrap()
    }

    /// Connects to `listener` as `identity`, trusting any server.
    fn join(listener: &Listener, content: u64, identity: &Identity) -> Result<ClientLink, String> {
        connect(
            &listener.address.to_string(),
            content,
            identity,
            "Tester",
            None,
        )
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
        let listener = server(7);
        let me = Identity::generate();
        let mut client = join(&listener, 7, &me).unwrap();
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

    /// A client whose content isn't the server's is refused, and told why.
    #[test]
    fn different_content_is_refused() {
        let listener = server(7);
        let error = join(&listener, 8, &Identity::generate()).err().unwrap();
        assert!(error.contains("mods"), "{error}");
        assert!(listener.accept().is_empty());
    }

    /// A server that stops tells its clients, who see their link close.
    #[test]
    fn clients_see_the_server_stop() {
        let listener = server(7);
        let mut client = join(&listener, 7, &Identity::generate()).unwrap();
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
        let listener = server(7);
        let (first, second) = (Identity::generate(), Identity::generate());
        let _a = join(&listener, 7, &first).unwrap();
        let _b = join(&listener, 7, &second).unwrap();
        let mut ids: Vec<PlayerId> = wait_for(|| listener.accept(), 2)
            .into_iter()
            .map(|joining| joining.id)
            .collect();
        ids.sort();
        let mut expected = vec![first.id, second.id];
        expected.sort();
        assert_eq!(ids, expected);
        assert_ne!(first.id, second.id);
        let _again = join(&listener, 7, &first).unwrap();
        assert_eq!(wait_for(|| listener.accept(), 1)[0].id, first.id);
    }

    /// A client learns the server's fingerprint on first connecting;
    /// connecting expecting it works, and a server whose key isn't the one
    /// expected (another server at the same address) is refused, saying
    /// so.
    #[test]
    fn a_changed_server_key_is_refused() {
        let key = Identity::generate();
        let listener = listen(local(), 7, &key).unwrap();
        let address = listener.address.to_string();
        let me = Identity::generate();
        let first = connect(&address, 7, &me, "Tester", None).unwrap();
        assert_eq!(first.fingerprint, Fingerprint(key.id.0));
        let again = connect(&address, 7, &me, "Tester", Some(first.fingerprint));
        assert!(again.is_ok());
        let _ = wait_for(|| listener.accept(), 2);
        drop(listener);

        // A server with another key, where that one was.
        let impostor = listen(local(), 7, &Identity::generate()).unwrap();
        let address = impostor.address.to_string();
        let error = connect(&address, 7, &me, "Tester", Some(first.fingerprint))
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
