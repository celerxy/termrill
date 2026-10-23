//! 统计进入目标 SSH 协议的密文字节；只观察字节数，不保存内容或连接地址。
//! 跳板模式统计 direct-tcpip 内的目标 SSH 流，不能代表外层 TCP 的全部流量。
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

pub(super) struct TransportReceive<S> {
    stream: S,
    received: Arc<AtomicU64>,
}
impl<S> TransportReceive<S> {
    pub fn new(stream: S, received: Arc<AtomicU64>) -> Self {
        Self { stream, received }
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for TransportReceive<S> {
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
impl<S: AsyncWrite + Unpin> AsyncWrite for TransportReceive<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
