use std::collections::HashMap;

use bytes::BytesMut;
use snow::{Builder, Error as SnowError, HandshakeState, TransportState};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Frame, ProtocolError};

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
    noise_builder_for(NOISE_IK_PATTERN)
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

pub struct NoiseFrameStream<S> {
    io: S,
    transport: TransportState,
    remote_static: Option<Vec<u8>>,
    wire_read_buf: BytesMut,
    plaintext_buf: BytesMut,
    decrypt_buf: Vec<u8>,
    encrypt_buf: Vec<u8>,
    highest_inbound_seq: HashMap<u32, u64>,
}

impl<S> NoiseFrameStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
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
            wire_read_buf: BytesMut::with_capacity(8192),
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

            let encrypted = self.read_noise_message_buffered().await?;
            let len = self
                .transport
                .read_message(&encrypted, &mut self.decrypt_buf)?;
            self.plaintext_buf
                .extend_from_slice(&self.decrypt_buf[..len]);
        }
    }

    /// Read one Noise wire record into a caller-owned buffer.
    ///
    /// [`read_frame`] is a branch of `tokio::select!` on both the
    /// gateway and agent.  Partial wire records must survive
    /// cancellation and re-creation of the `read_frame()` future, so
    /// this helper uses the cancellation-safe [`AsyncReadExt::read_buf`]
    /// and stages data in [`self.wire_read_buf`] until a complete
    /// `[length][ciphertext]` record is available.
    async fn read_noise_message_buffered(&mut self) -> Result<BytesMut, TransportError> {
        loop {
            if self.wire_read_buf.len() >= 2 {
                let len = usize::from(u16::from_be_bytes([
                    self.wire_read_buf[0],
                    self.wire_read_buf[1],
                ]));
                if len > MAX_NOISE_MESSAGE_LEN {
                    return Err(TransportError::NoiseMessageTooLarge(len));
                }
                let record_len = 2 + len;
                if self.wire_read_buf.len() >= record_len {
                    let mut record = self.wire_read_buf.split_to(record_len);
                    return Ok(record.split_off(2));
                }
            }

            // Ensure read_buf cannot return zero merely because the
            // BytesMut has no spare allocation.
            self.wire_read_buf.reserve(8192);

            if self.io.read_buf(&mut self.wire_read_buf).await? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed while reading Noise message",
                )
                .into());
            }
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
#[path = "tests_transport.rs"]
mod tests;
