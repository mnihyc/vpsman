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
