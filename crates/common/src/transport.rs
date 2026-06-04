use std::collections::HashMap;

use bytes::BytesMut;
use snow::{Builder, Error as SnowError, HandshakeState, TransportState};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Frame, ProtocolError};

pub const NOISE_XX_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const NOISE_IK_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
pub const MAX_NOISE_MESSAGE_LEN: usize = 65_535;
pub const MAX_NOISE_PLAINTEXT_CHUNK: usize = 16 * 1024;
const NOISE_TAG_OVERHEAD: usize = 16;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("noise error")]
    Noise(#[from] SnowError),
    #[error("protocol error")]
    Protocol(#[from] ProtocolError),
    #[error("noise message length {0} exceeds maximum {MAX_NOISE_MESSAGE_LEN}")]
    NoiseMessageTooLarge(usize),
    #[error("noise key hex is invalid: {0}")]
    InvalidKeyHex(String),
    #[error("noise handshake did not reveal remote static key")]
    MissingRemoteStatic,
    #[error("noise remote static key did not match enrolled identity")]
    RemoteStaticMismatch,
    #[error("stale or replayed frame on stream {stream_id}: seq {seq} <= last {last_seq}")]
    StaleFrame {
        stream_id: u32,
        seq: u64,
        last_seq: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoiseKeypair {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl NoiseKeypair {
    pub fn private_hex(&self) -> String {
        hex::encode(&self.private)
    }

    pub fn public_hex(&self) -> String {
        hex::encode(&self.public)
    }
}

pub fn noise_builder() -> Result<Builder<'static>, SnowError> {
    noise_builder_for(NOISE_XX_PATTERN)
}

pub fn noise_builder_for(pattern: &str) -> Result<Builder<'static>, SnowError> {
    Ok(Builder::new(pattern.parse()?))
}

pub fn generate_noise_keypair() -> Result<NoiseKeypair, TransportError> {
    let keypair = noise_builder()?.generate_keypair()?;
    Ok(NoiseKeypair {
        private: keypair.private,
        public: keypair.public,
    })
}

pub fn decode_noise_key_hex(value: &str) -> Result<Vec<u8>, TransportError> {
    let key =
        hex::decode(value).map_err(|error| TransportError::InvalidKeyHex(error.to_string()))?;
    if key.len() != 32 {
        return Err(TransportError::InvalidKeyHex(format!(
            "expected 32 bytes, got {}",
            key.len()
        )));
    }
    Ok(key)
}

pub fn client_handshake() -> Result<HandshakeState, SnowError> {
    let builder = noise_builder()?;
    let keypair = builder.generate_keypair()?;
    noise_builder()?
        .local_private_key(&keypair.private)
        .build_initiator()
}

pub fn server_handshake() -> Result<HandshakeState, SnowError> {
    let builder = noise_builder()?;
    let keypair = builder.generate_keypair()?;
    noise_builder()?
        .local_private_key(&keypair.private)
        .build_responder()
}

pub fn enrolled_client_handshake(
    client_private_key: &[u8],
    server_public_key: &[u8],
) -> Result<HandshakeState, SnowError> {
    noise_builder_for(NOISE_IK_PATTERN)?
        .local_private_key(client_private_key)
        .remote_public_key(server_public_key)
        .build_initiator()
}

pub fn enrolled_server_handshake(server_private_key: &[u8]) -> Result<HandshakeState, SnowError> {
    noise_builder_for(NOISE_IK_PATTERN)?
        .local_private_key(server_private_key)
        .build_responder()
}

pub fn complete_handshake(
    mut client: HandshakeState,
    mut server: HandshakeState,
) -> Result<(TransportState, TransportState), SnowError> {
    let mut msg1 = [0_u8; 1024];
    let msg1_len = client.write_message(&[], &mut msg1)?;

    let mut msg2 = [0_u8; 1024];
    server.read_message(&msg1[..msg1_len], &mut [])?;
    let msg2_len = server.write_message(&[], &mut msg2)?;

    let mut msg3 = [0_u8; 1024];
    client.read_message(&msg2[..msg2_len], &mut [])?;
    let msg3_len = client.write_message(&[], &mut msg3)?;

    server.read_message(&msg3[..msg3_len], &mut [])?;
    Ok((client.into_transport_mode()?, server.into_transport_mode()?))
}

pub struct NoiseFrameStream<S> {
    io: S,
    transport: TransportState,
    remote_static: Option<Vec<u8>>,
    plaintext_buf: BytesMut,
    decrypt_buf: Vec<u8>,
    encrypt_buf: Vec<u8>,
    highest_inbound_seq: HashMap<u32, u64>,
}

impl<S> NoiseFrameStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn client(io: S) -> Result<Self, TransportError> {
        let handshake = client_handshake()?;
        Self::handshake_xx(io, handshake, true).await
    }

    pub async fn server(io: S) -> Result<Self, TransportError> {
        let handshake = server_handshake()?;
        Self::handshake_xx(io, handshake, false).await
    }

    pub async fn client_enrolled(
        io: S,
        client_private_key: &[u8],
        server_public_key: &[u8],
    ) -> Result<Self, TransportError> {
        let handshake = enrolled_client_handshake(client_private_key, server_public_key)?;
        Self::handshake_ik_client(io, handshake).await
    }

    pub async fn server_enrolled(
        io: S,
        server_private_key: &[u8],
        expected_client_public_key: Option<&[u8]>,
    ) -> Result<Self, TransportError> {
        let handshake = enrolled_server_handshake(server_private_key)?;
        Self::handshake_ik_server(io, handshake, expected_client_public_key).await
    }

    async fn handshake_xx(
        mut io: S,
        mut handshake: HandshakeState,
        initiator: bool,
    ) -> Result<Self, TransportError> {
        let mut msg = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE_LEN];

        if initiator {
            let len = handshake.write_message(&[], &mut msg)?;
            write_noise_message(&mut io, &msg[..len]).await?;

            let incoming = read_noise_message(&mut io).await?;
            handshake.read_message(&incoming, &mut payload)?;

            let len = handshake.write_message(&[], &mut msg)?;
            write_noise_message(&mut io, &msg[..len]).await?;
        } else {
            let incoming = read_noise_message(&mut io).await?;
            handshake.read_message(&incoming, &mut payload)?;

            let len = handshake.write_message(&[], &mut msg)?;
            write_noise_message(&mut io, &msg[..len]).await?;

            let incoming = read_noise_message(&mut io).await?;
            handshake.read_message(&incoming, &mut payload)?;
        }

        let remote_static = handshake.get_remote_static().map(ToOwned::to_owned);
        Self::from_handshake(io, handshake, remote_static)
    }

    async fn handshake_ik_client(
        mut io: S,
        mut handshake: HandshakeState,
    ) -> Result<Self, TransportError> {
        let mut msg = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE_LEN];

        let len = handshake.write_message(&[], &mut msg)?;
        write_noise_message(&mut io, &msg[..len]).await?;

        let incoming = read_noise_message(&mut io).await?;
        handshake.read_message(&incoming, &mut payload)?;

        let remote_static = handshake.get_remote_static().map(ToOwned::to_owned);
        Self::from_handshake(io, handshake, remote_static)
    }

    async fn handshake_ik_server(
        mut io: S,
        mut handshake: HandshakeState,
        expected_client_public_key: Option<&[u8]>,
    ) -> Result<Self, TransportError> {
        let mut msg = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE_LEN];

        let incoming = read_noise_message(&mut io).await?;
        handshake.read_message(&incoming, &mut payload)?;
        let remote_static = handshake
            .get_remote_static()
            .map(ToOwned::to_owned)
            .ok_or(TransportError::MissingRemoteStatic)?;
        if let Some(expected) = expected_client_public_key {
            if remote_static != expected {
                return Err(TransportError::RemoteStaticMismatch);
            }
        }

        let len = handshake.write_message(&[], &mut msg)?;
        write_noise_message(&mut io, &msg[..len]).await?;

        Self::from_handshake(io, handshake, Some(remote_static))
    }

    fn from_handshake(
        io: S,
        handshake: HandshakeState,
        remote_static: Option<Vec<u8>>,
    ) -> Result<Self, TransportError> {
        Ok(Self {
            io,
            transport: handshake.into_transport_mode()?,
            remote_static,
            plaintext_buf: BytesMut::with_capacity(8192),
            decrypt_buf: vec![0_u8; MAX_NOISE_MESSAGE_LEN],
            encrypt_buf: vec![0_u8; MAX_NOISE_PLAINTEXT_CHUNK + NOISE_TAG_OVERHEAD],
            highest_inbound_seq: HashMap::new(),
        })
    }

    pub fn remote_static(&self) -> Option<&[u8]> {
        self.remote_static.as_deref()
    }

    pub async fn write_frame(&mut self, frame: &Frame) -> Result<(), TransportError> {
        let mut plaintext = BytesMut::new();
        frame.encode(&mut plaintext)?;

        for chunk in plaintext.chunks(MAX_NOISE_PLAINTEXT_CHUNK) {
            let len = self.transport.write_message(chunk, &mut self.encrypt_buf)?;
            write_noise_message(&mut self.io, &self.encrypt_buf[..len]).await?;
        }
        self.io.flush().await?;
        Ok(())
    }

    pub async fn read_frame(&mut self) -> Result<Frame, TransportError> {
        loop {
            if let Some(frame) = Frame::decode(&mut self.plaintext_buf)? {
                self.validate_inbound_frame(&frame)?;
                return Ok(frame);
            }

            let encrypted = read_noise_message(&mut self.io).await?;
            let len = self
                .transport
                .read_message(&encrypted, &mut self.decrypt_buf)?;
            self.plaintext_buf
                .extend_from_slice(&self.decrypt_buf[..len]);
        }
    }

    fn validate_inbound_frame(&mut self, frame: &Frame) -> Result<(), TransportError> {
        match self.highest_inbound_seq.get_mut(&frame.stream_id) {
            Some(last_seq) if frame.seq <= *last_seq => Err(TransportError::StaleFrame {
                stream_id: frame.stream_id,
                seq: frame.seq,
                last_seq: *last_seq,
            }),
            Some(last_seq) => {
                *last_seq = frame.seq;
                Ok(())
            }
            None => {
                self.highest_inbound_seq.insert(frame.stream_id, frame.seq);
                Ok(())
            }
        }
    }
}

async fn write_noise_message<S>(io: &mut S, message: &[u8]) -> Result<(), TransportError>
where
    S: AsyncWrite + Unpin,
{
    if message.len() > MAX_NOISE_MESSAGE_LEN {
        return Err(TransportError::NoiseMessageTooLarge(message.len()));
    }
    io.write_u16(message.len() as u16).await?;
    io.write_all(message).await?;
    Ok(())
}

async fn read_noise_message<S>(io: &mut S) -> Result<Vec<u8>, TransportError>
where
    S: AsyncRead + Unpin,
{
    let len = io.read_u16().await? as usize;
    if len > MAX_NOISE_MESSAGE_LEN {
        return Err(TransportError::NoiseMessageTooLarge(len));
    }
    let mut message = vec![0_u8; len];
    io.read_exact(&mut message).await?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    struct RecordingIo<S> {
        inner: S,
        writes: Arc<Mutex<Vec<u8>>>,
    }

    impl<S> RecordingIo<S> {
        fn new(inner: S, writes: Arc<Mutex<Vec<u8>>>) -> Self {
            Self { inner, writes }
        }
    }

    impl<S> AsyncRead for RecordingIo<S>
    where
        S: AsyncRead + Unpin,
    {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl<S> AsyncWrite for RecordingIo<S>
    where
        S: AsyncWrite + Unpin,
    {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            match Pin::new(&mut self.inner).poll_write(cx, buf) {
                Poll::Ready(Ok(written)) => {
                    self.writes
                        .lock()
                        .expect("recording mutex")
                        .extend_from_slice(&buf[..written]);
                    Poll::Ready(Ok(written))
                }
                other => other,
            }
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[test]
    fn noise_xx_handshake_reaches_transport_mode() {
        const TEST_TRANSPORT_PING_PAYLOAD: &[u8] = b"ping";

        let client = client_handshake().unwrap();
        let server = server_handshake().unwrap();
        let (mut client_transport, mut server_transport) =
            complete_handshake(client, server).unwrap();

        let mut ciphertext = [0_u8; 128];
        let len = client_transport
            .write_message(TEST_TRANSPORT_PING_PAYLOAD, &mut ciphertext)
            .unwrap();
        let mut plaintext = [0_u8; 128];
        let plaintext_len = server_transport
            .read_message(&ciphertext[..len], &mut plaintext)
            .unwrap();

        assert_eq!(&plaintext[..plaintext_len], TEST_TRANSPORT_PING_PAYLOAD);
    }

    #[tokio::test]
    async fn noise_frame_stream_round_trips_tlv_frames() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client = NoiseFrameStream::client(client_io);
        let server = NoiseFrameStream::server(server_io);
        let (mut client, mut server) = tokio::try_join!(client, server).unwrap();

        let frame = Frame::new(
            crate::MessageKind::Telemetry,
            4,
            9,
            b"secret telemetry".to_vec(),
        );
        client.write_frame(&frame).await.unwrap();
        let received = server.read_frame().await.unwrap();

        assert_eq!(received.kind, crate::MessageKind::Telemetry);
        assert_eq!(received.stream_id, 4);
        assert_eq!(received.seq, 9);
        assert_eq!(received.payload, b"secret telemetry");
    }

    #[tokio::test]
    async fn noise_wire_does_not_expose_tlv_magic_or_payload() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client_writes = Arc::new(Mutex::new(Vec::new()));
        let client_io = RecordingIo::new(client_io, Arc::clone(&client_writes));
        let client = NoiseFrameStream::client(client_io);
        let server = NoiseFrameStream::server(server_io);
        let (mut client, mut server) = tokio::try_join!(client, server).unwrap();

        let secret_payload = b"secret telemetry payload that must stay encrypted".to_vec();
        client
            .write_frame(&Frame::new(
                crate::MessageKind::Telemetry,
                2,
                1,
                secret_payload.clone(),
            ))
            .await
            .unwrap();
        let received = server.read_frame().await.unwrap();
        assert_eq!(received.payload, secret_payload);

        let wire = client_writes.lock().expect("recording mutex").clone();
        assert!(
            !contains_subsequence(&wire, crate::MAGIC),
            "raw TCP-side Noise bytes unexpectedly exposed TLV magic"
        );
        assert!(
            !contains_subsequence(&wire, b"secret telemetry payload"),
            "raw TCP-side Noise bytes unexpectedly exposed plaintext payload"
        );
    }

    #[tokio::test]
    async fn noise_frame_stream_rejects_stale_sequence_per_stream() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client = NoiseFrameStream::client(client_io);
        let server = NoiseFrameStream::server(server_io);
        let (mut client, mut server) = tokio::try_join!(client, server).unwrap();

        client
            .write_frame(&Frame::new(
                crate::MessageKind::Telemetry,
                7,
                10,
                b"first".to_vec(),
            ))
            .await
            .unwrap();
        assert_eq!(server.read_frame().await.unwrap().seq, 10);

        client
            .write_frame(&Frame::new(
                crate::MessageKind::Telemetry,
                8,
                10,
                b"different stream".to_vec(),
            ))
            .await
            .unwrap();
        let other_stream = server.read_frame().await.unwrap();
        assert_eq!(other_stream.stream_id, 8);
        assert_eq!(other_stream.seq, 10);

        client
            .write_frame(&Frame::new(
                crate::MessageKind::Telemetry,
                7,
                10,
                b"replay".to_vec(),
            ))
            .await
            .unwrap();
        assert!(matches!(
            server.read_frame().await,
            Err(TransportError::StaleFrame {
                stream_id: 7,
                seq: 10,
                last_seq: 10,
            })
        ));
    }

    #[tokio::test]
    async fn enrolled_ik_authenticates_client_and_pins_server() {
        let server_key = generate_noise_keypair().unwrap();
        let client_key = generate_noise_keypair().unwrap();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);

        let client =
            NoiseFrameStream::client_enrolled(client_io, &client_key.private, &server_key.public);
        let server = NoiseFrameStream::server_enrolled(
            server_io,
            &server_key.private,
            Some(&client_key.public),
        );
        let (client, server) = tokio::try_join!(client, server).unwrap();

        assert_eq!(client.remote_static(), Some(server_key.public.as_slice()));
        assert_eq!(server.remote_static(), Some(client_key.public.as_slice()));
    }

    #[tokio::test]
    async fn enrolled_ik_rejects_unexpected_client_key() {
        let server_key = generate_noise_keypair().unwrap();
        let client_key = generate_noise_keypair().unwrap();
        let other_client_key = generate_noise_keypair().unwrap();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);

        let client =
            NoiseFrameStream::client_enrolled(client_io, &client_key.private, &server_key.public);
        let server = NoiseFrameStream::server_enrolled(
            server_io,
            &server_key.private,
            Some(&other_client_key.public),
        );

        let result = tokio::try_join!(client, server);
        assert!(matches!(
            result,
            Err(TransportError::RemoteStaticMismatch)
                | Err(TransportError::Io(_))
                | Err(TransportError::Noise(_))
        ));
    }

    fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}
