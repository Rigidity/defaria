//! The network: a server and its clients over QUIC (`quinn`, on async-io,
//! with rustls), carrying the messages of `defaria_core::net` between a
//! link in memory at each end. The game on either side sees the same
//! [`ServerLink`] or [`ClientLink`] it does in single player; a background
//! thread moves what goes in one end over the network and out the other.
//!
//! A connection's client opens a stream both ways, the control stream,
//! and says who it is ([`Handshake::Hello`]: the protocol and its
//! content's fingerprint); the server answers on it, accepting or refusing.
//! Then:
//!
//! - chunks, their changes and drops, and whether the client's settled go
//!   on a stream of their own from the server ([`bulk`]), so a burst of
//!   chunks never holds up anything else;
//! - a client's pose goes as a datagram: unreliable, the newest is all
//!   that matters;
//! - everything else goes on the control stream, in order.
//!
//! On a stream, each message is a frame: its length, a flag byte (whether
//! it's compressed: big ones, chunks mostly, are deflated) and its bytes.
//!
//! The server's certificate is made afresh each time it starts, and the
//! client accepts any: who the server is, and remembering it, is #216.

use std::{
    io,
    net::{SocketAddr, ToSocketAddrs},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use bevy::prelude::*;
use defaria_core::net::{
    ClientLink, ClientMessage, ServerLink, ServerMessage, decode, encode, link,
};
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use serde::{Deserialize, Serialize};

/// The port a server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 25_575;

/// The protocol's version: a client and server of different ones refuse
/// each other.
pub const PROTOCOL: u32 = 1;

const ALPN: &[u8] = b"defaria/1";
/// The name the client asks the server's certificate for.
const SERVER_NAME: &str = "defaria";
/// Frames bigger than this are deflated.
const COMPRESS_OVER: usize = 512;
/// The biggest frame accepted, so a bad length can't ask for all memory.
const MAX_FRAME: usize = 64 << 20;
/// How often the background thread looks for messages the game sent.
const POLL: Duration = Duration::from_millis(1);

/// What a client and server say first, on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handshake {
    Hello { protocol: u32, content: u64 },
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
    )
}

/// A server listening for clients: each that connects, and says it has the
/// same content, comes out of [`Listener::accept`] as a link to it.
pub struct Listener {
    pub address: SocketAddr,
    incoming: mpsc::Receiver<ServerLink>,
}

impl Listener {
    /// The clients that connected since the last call.
    pub fn accept(&self) -> Vec<ServerLink> {
        self.incoming.try_iter().collect()
    }
}

/// Listens on `address` (port 0 for any free one) for clients whose
/// content has the fingerprint `content`.
pub fn listen(address: SocketAddr, content: u64) -> io::Result<Listener> {
    let config = server_config().map_err(io::Error::other)?;
    let endpoint = Endpoint::server(config, address)?;
    let address = endpoint.local_addr()?;
    let (send, incoming) = mpsc::channel();
    thread::Builder::new()
        .name("server network".into())
        .spawn(move || {
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
    info!("listening on {address}");
    Ok(Listener { address, incoming })
}

/// Connects to the server at `address` (a host and port), saying our
/// content has the fingerprint `content`, and returns the link to it once
/// the server has accepted. Blocks until then.
pub fn connect(address: &str, content: u64) -> Result<ClientLink, String> {
    let address = address
        .to_socket_addrs()
        .map_err(|error| format!("{address}: {error}"))?
        .next()
        .ok_or_else(|| format!("{address}: no such address"))?;
    let (result_send, result) = mpsc::channel();
    thread::Builder::new()
        .name("client network".into())
        .spawn(move || {
            smol::block_on(async move {
                match open(address, content).await {
                    Ok((connection, control, server_end, game_end)) => {
                        let _ = result_send.send(Ok(game_end));
                        play(connection, control, server_end).await;
                    }
                    Err(error) => {
                        let _ = result_send.send(Err(error));
                    }
                }
            });
        })
        .map_err(|error| error.to_string())?;
    result
        .recv()
        .map_err(|_| "the network thread stopped".to_string())?
}

/// The client's side of setting up: connects, says hello, and waits for
/// the answer.
async fn open(
    address: SocketAddr,
    content: u64,
) -> Result<(Connection, (SendStream, RecvStream), ServerLink, ClientLink), String> {
    let bind: SocketAddr = if address.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let mut endpoint = Endpoint::client(bind).map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(client_config()?);
    let connection = endpoint
        .connect(address, SERVER_NAME)
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
    };
    write_frame(&mut send, &encode(&hello))
        .await
        .map_err(|error| error.to_string())?;
    let answer = read_frame(&mut receive)
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

/// The server's side of a connection: hears the client out, and if it's
/// welcome, carries its messages until either side goes.
async fn serve(connection: Connection, content: u64, accepted: mpsc::Sender<ServerLink>) {
    let Ok((mut send, mut receive)) = connection.accept_bi().await else {
        return;
    };
    let Ok(hello) = read_frame(&mut receive).await else {
        return;
    };
    let refusal = match decode::<Handshake>(&hello) {
        Ok(Handshake::Hello {
            protocol,
            content: theirs,
        }) => {
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
        // Let the answer get there before the connection goes.
        smol::Timer::after(Duration::from_millis(200)).await;
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
    info!("{} joined", connection.remote_address());
    let (net_end, game_end) = link();
    if accepted.send(game_end).is_err() {
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
            while let Ok(frame) = read_frame(&mut from_client).await {
                match decode::<ClientMessage>(&frame) {
                    Ok(message) => {
                        if to_game.send(message).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        warn!("a client sent something odd: {error}");
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
            let stream = if bulk(&message) {
                &mut bulk_stream
            } else {
                &mut control
            };
            if write_frame(stream, &encode(&message)).await.is_err() {
                return;
            }
        }
        if game.is_closed() || connection.close_reason().is_some() {
            connection.close(0u32.into(), b"bye");
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
    let _control = {
        let to_game = to_game.clone();
        smol::spawn(async move {
            while let Ok(frame) = read_frame(&mut from_server).await {
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
            while let Ok(frame) = read_frame(&mut bulk_stream).await {
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
            let small = connection
                .max_datagram_size()
                .is_some_and(|max| bytes.len() <= max);
            if matches!(message, ClientMessage::Pose(_)) && small {
                let _ = connection.send_datagram(bytes.into());
            } else if write_frame(&mut control, &bytes).await.is_err() {
                return;
            }
        }
        if game.is_closed() || connection.close_reason().is_some() {
            connection.close(0u32.into(), b"bye");
            return;
        }
        smol::Timer::after(POLL).await;
    }
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

/// Reads a frame's bytes, inflated if they were deflated.
async fn read_frame(stream: &mut RecvStream) -> io::Result<Vec<u8>> {
    let mut length = [0; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(io::Error::other)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(io::Error::other(format!("a frame of {length} bytes")));
    }
    let mut frame = vec![0; length];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(io::Error::other)?;
    match frame[0] {
        0 => Ok(frame.split_off(1)),
        1 => miniz_oxide::inflate::decompress_to_vec_with_limit(&frame[1..], MAX_FRAME)
            .map_err(|error| io::Error::other(format!("{error:?}"))),
        flag => Err(io::Error::other(format!("a frame flagged {flag}"))),
    }
}

/// A server's config: a certificate made just now, for [`SERVER_NAME`].
fn server_config() -> Result<ServerConfig, String> {
    let certified = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_string()])
        .map_err(|error| error.to_string())?;
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|error| error.to_string())?
    .with_no_client_auth()
    .with_single_cert(vec![certified.cert.der().clone()], key.into())
    .map_err(|error| error.to_string())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicServerConfig::try_from(tls).map_err(|error| error.to_string())?;
    Ok(ServerConfig::with_crypto(Arc::new(crypto)))
}

/// A client's config, accepting any server's certificate (for now: #216).
fn client_config() -> Result<ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyServer(provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicClientConfig::try_from(tls).map_err(|error| error.to_string())?;
    Ok(ClientConfig::new(Arc::new(crypto)))
}

/// Takes any server at its word, checking only that it holds the key its
/// certificate names (until #216 remembers servers).
#[derive(Debug)]
struct AnyServer(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyServer {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
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
    /// pose as a datagram.
    #[test]
    fn messages_cross_the_network() {
        let listener = listen(local(), 7).unwrap();
        let address = listener.address.to_string();
        let mut client = connect(&address, 7).unwrap();
        let mut server = wait_for(|| listener.accept(), 1).pop().unwrap();

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

        client.send(ClientMessage::Pose(defaria_core::net::Pose {
            feet: bevy::math::DVec3::new(1.0, 2.0, 3.0),
            velocity: bevy::math::DVec3::ZERO,
            on_ground: true,
            look: default(),
            player: default(),
        }));
        assert!(matches!(
            wait_for(|| server.receive(), 1)[0],
            ClientMessage::Pose(_)
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
        let listener = listen(local(), 7).unwrap();
        let error = connect(&listener.address.to_string(), 8).err().unwrap();
        assert!(error.contains("mods"), "{error}");
        thread::sleep(Duration::from_millis(50));
        assert!(listener.accept().is_empty());
    }
}
