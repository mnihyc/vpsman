use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"VPSM";
pub const CURRENT_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 24;
pub const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;
pub const FLAG_COMPRESSED_LZ4: u8 = 0b0000_0001;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid frame magic")]
    InvalidMagic,
    #[error("unsupported frame version {0}")]
    UnsupportedVersion(u8),
    #[error("payload length {0} exceeds maximum {MAX_PAYLOAD_LEN}")]
    PayloadTooLarge(usize),
    #[error("decompressed payload length {0} exceeds maximum {MAX_PAYLOAD_LEN}")]
    DecompressedPayloadTooLarge(usize),
    #[error("compressed payload could not be decoded: {0}")]
    Compression(String),
    #[error("payload could not be decoded as json")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MessageKind {
    ClientHello,
    ServerHello,
    Telemetry,
    Command,
    CommandAck,
    CommandCancel,
    CommandCancelAck,
    CommandOutput,
    TerminalStreamOutput,
    FileChunk,
    ConfigUpdate,
    Keepalive,
    Error,
    Unknown(u16),
}

impl MessageKind {
    pub fn from_u16(value: u16) -> Self {
        match value {
            1 => Self::ClientHello,
            2 => Self::ServerHello,
            16 => Self::Telemetry,
            32 => Self::Command,
            33 => Self::CommandAck,
            35 => Self::CommandCancel,
            37 => Self::CommandCancelAck,
            34 => Self::CommandOutput,
            36 => Self::TerminalStreamOutput,
            48 => Self::FileChunk,
            49 => Self::ConfigUpdate,
            64 => Self::Keepalive,
            255 => Self::Error,
            other => Self::Unknown(other),
        }
    }

    pub fn as_u16(self) -> u16 {
        match self {
            Self::ClientHello => 1,
            Self::ServerHello => 2,
            Self::Telemetry => 16,
            Self::Command => 32,
            Self::CommandAck => 33,
            Self::CommandCancel => 35,
            Self::CommandCancelAck => 37,
            Self::CommandOutput => 34,
            Self::TerminalStreamOutput => 36,
            Self::FileChunk => 48,
            Self::ConfigUpdate => 49,
            Self::Keepalive => 64,
            Self::Error => 255,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub version: u8,
    pub flags: u8,
    pub kind: MessageKind,
    pub stream_id: u32,
    pub seq: u64,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: MessageKind, stream_id: u32, seq: u64, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            version: CURRENT_VERSION,
            flags: 0,
            kind,
            stream_id,
            seq,
            payload: payload.into(),
        }
    }

    pub fn encode(&self, dst: &mut BytesMut) -> Result<(), ProtocolError> {
        if self.version != CURRENT_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(ProtocolError::PayloadTooLarge(self.payload.len()));
        }

        dst.reserve(HEADER_LEN + self.payload.len());
        dst.put_slice(MAGIC);
        dst.put_u8(self.version);
        dst.put_u8(self.flags);
        dst.put_u16(self.kind.as_u16());
        dst.put_u32(self.stream_id);
        dst.put_u64(self.seq);
        dst.put_u32(self.payload.len() as u32);
        dst.put_slice(&self.payload);
        Ok(())
    }

    pub fn decode(src: &mut BytesMut) -> Result<Option<Self>, ProtocolError> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }
        if &src[..4] != MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }

        let version = src[4];
        if version != CURRENT_VERSION {
            return Err(ProtocolError::UnsupportedVersion(version));
        }

        let payload_len = u32::from_be_bytes([src[20], src[21], src[22], src[23]]) as usize;
        if payload_len > MAX_PAYLOAD_LEN {
            return Err(ProtocolError::PayloadTooLarge(payload_len));
        }
        if src.len() < HEADER_LEN + payload_len {
            return Ok(None);
        }

        src.advance(4);
        let version = src.get_u8();
        let flags = src.get_u8();
        let kind = MessageKind::from_u16(src.get_u16());
        let stream_id = src.get_u32();
        let seq = src.get_u64();
        let payload_len = src.get_u32() as usize;
        let payload = src.split_to(payload_len).to_vec();

        Ok(Some(Self {
            version,
            flags,
            kind,
            stream_id,
            seq,
            payload,
        }))
    }

    pub fn decoded_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.flags & FLAG_COMPRESSED_LZ4 == 0 {
            return Ok(self.payload.clone());
        }
        let decompressed_len = decompressed_len_prefix(&self.payload)?;
        if decompressed_len > MAX_PAYLOAD_LEN {
            return Err(ProtocolError::DecompressedPayloadTooLarge(decompressed_len));
        }
        lz4_flex::decompress_size_prepended(&self.payload)
            .map_err(|error| ProtocolError::Compression(error.to_string()))
    }
}

fn decompressed_len_prefix(payload: &[u8]) -> Result<usize, ProtocolError> {
    let size = payload
        .get(..4)
        .ok_or_else(|| ProtocolError::Compression("missing size prefix".to_string()))?;
    let size: [u8; 4] = size
        .try_into()
        .map_err(|_| ProtocolError::Compression("invalid size prefix".to_string()))?;
    Ok(u32::from_le_bytes(size) as usize)
}

pub fn maybe_compress_payload(
    payload: &[u8],
    threshold: usize,
) -> Result<(u8, Vec<u8>), ProtocolError> {
    if payload.len() < threshold {
        return Ok((0, payload.to_vec()));
    }
    let compressed = lz4_flex::compress_prepend_size(payload);
    if compressed.len() >= payload.len() {
        return Ok((0, payload.to_vec()));
    }
    Ok((FLAG_COMPRESSED_LZ4, compressed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let frame = Frame::new(MessageKind::Telemetry, 7, 42, b"hello".to_vec());
        let mut buf = BytesMut::new();
        frame.encode(&mut buf).unwrap();

        let decoded = Frame::decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.kind, MessageKind::Telemetry);
        assert_eq!(decoded.stream_id, 7);
        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.payload, b"hello");
        assert!(buf.is_empty());
    }

    #[test]
    fn waits_for_complete_payload() {
        let frame = Frame::new(MessageKind::Keepalive, 0, 1, b"abc".to_vec());
        let mut buf = BytesMut::new();
        frame.encode(&mut buf).unwrap();
        let last = buf.split_off(buf.len() - 1);

        assert!(Frame::decode(&mut buf).unwrap().is_none());
        buf.extend_from_slice(&last);
        assert!(Frame::decode(&mut buf).unwrap().is_some());
    }

    #[test]
    fn compressed_payload_round_trip() {
        let payload = vec![42_u8; 2048];
        let (flags, compressed) = maybe_compress_payload(&payload, 128).unwrap();
        assert_eq!(flags, FLAG_COMPRESSED_LZ4);

        let mut frame = Frame::new(MessageKind::Telemetry, 0, 1, compressed);
        frame.flags = flags;
        assert_eq!(frame.decoded_payload().unwrap(), payload);
    }

    #[test]
    fn rejects_oversized_decompressed_payload_before_allocation() {
        let mut oversized = ((MAX_PAYLOAD_LEN as u32) + 1).to_le_bytes().to_vec();
        oversized.extend_from_slice(b"not-valid-lz4");
        let mut frame = Frame::new(MessageKind::Telemetry, 0, 1, oversized);
        frame.flags = FLAG_COMPRESSED_LZ4;

        assert!(matches!(
            frame.decoded_payload(),
            Err(ProtocolError::DecompressedPayloadTooLarge(size)) if size == MAX_PAYLOAD_LEN + 1
        ));
    }
}
