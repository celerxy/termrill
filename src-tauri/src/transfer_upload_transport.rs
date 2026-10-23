//! 上传目标 SSH 流的实际读写计数；不保存正文，不把派发量当作线上发送量。
//! 跳板模式统计 direct-tcpip 内的目标流，不代表外层 TCP 已被服务器确认。
use std::{
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(super) struct UploadTransport<S> {
    stream: S,
    received: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
}
impl<S> UploadTransport<S> {
    pub fn new(stream: S, received: Arc<AtomicU64>, sent: Arc<AtomicU64>) -> Self {
        Self {
            stream,
            received,
            sent,
        }
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for UploadTransport<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = output.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, output);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.received
                .fetch_add((output.filled().len() - before) as u64, Ordering::Relaxed);
        }
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for UploadTransport<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(cx, bytes);
        if let Poll::Ready(Ok(written)) = result {
            self.sent.fetch_add(written as u64, Ordering::Relaxed);
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[test]
    fn partial_writes_preserve_payload_and_count_only_accepted_bytes() {
        tauri::async_runtime::block_on(async {
            let (client, mut server) = tokio::io::duplex(2);
            let received = Arc::new(AtomicU64::new(0));
            let sent = Arc::new(AtomicU64::new(0));
            let mut client = UploadTransport::new(client, received.clone(), sent.clone());
            tokio::join!(
                async {
                    client.write_all(b"probe").await.unwrap();
                    let mut reply = [0; 3];
                    client.read_exact(&mut reply).await.unwrap();
                    assert_eq!(&reply, b"ack");
                },
                async {
                    let mut request = [0; 5];
                    server.read_exact(&mut request).await.unwrap();
                    assert_eq!(&request, b"probe");
                    server.write_all(b"ack").await.unwrap();
                }
            );
            assert_eq!(sent.load(Ordering::Relaxed), 5);
            assert_eq!(received.load(Ordering::Relaxed), 3);
        });
    }
}
