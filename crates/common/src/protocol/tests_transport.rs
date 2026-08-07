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

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn noise_frame_stream_round_trips_tlv_frames() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (mut client, mut server) = enrolled_stream_pair(client_io, server_io).await;

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
    let (mut client, mut server) = enrolled_stream_pair(client_io, server_io).await;

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
    let (mut client, mut server) = enrolled_stream_pair(client_io, server_io).await;

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
    let server =
        NoiseFrameStream::server_enrolled(server_io, &server_key.private, Some(&client_key.public));
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

async fn enrolled_stream_pair<C, S>(
    client_io: C,
    server_io: S,
) -> (NoiseFrameStream<C>, NoiseFrameStream<S>)
where
    C: AsyncRead + AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let server_key = generate_noise_keypair().unwrap();
    let client_key = generate_noise_keypair().unwrap();
    let client =
        NoiseFrameStream::client_enrolled(client_io, &client_key.private, &server_key.public);
    let server =
        NoiseFrameStream::server_enrolled(server_io, &server_key.private, Some(&client_key.public));
    tokio::try_join!(client, server).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn noise_frame_stream_read_survives_select_cancellation() {
    let (client_io, server_io) = tokio::io::duplex(1);
    let (mut client, mut server) = enrolled_stream_pair(client_io, server_io).await;

    let payload = b"cancel-safe frame".to_vec();
    let sent_payload = payload.clone();

    let writer = tokio::spawn(async move {
        client
            .write_frame(&Frame::new(
                crate::MessageKind::Telemetry,
                4,
                9,
                sent_payload,
            ))
            .await
            .unwrap();
    });

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            tokio::select! {
                biased;

                result = server.read_frame() => {
                    break result;
                }

                // If read_frame() returned Pending after consuming a byte,
                // this branch wins and drops that read future.
                _ = std::future::ready(()) => {
                    tokio::task::yield_now().await;
                }
            }
        }
    })
    .await
    .expect("read timed out")
    .expect("read failed");

    writer.await.unwrap();

    assert_eq!(received.kind, crate::MessageKind::Telemetry);
    assert_eq!(received.stream_id, 4);
    assert_eq!(received.seq, 9);
    assert_eq!(received.payload, payload);
}
