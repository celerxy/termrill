//! 统计收到的 SFTP DATA 正文字节，包括尚未收齐的分片，不计协议头和状态响应。
//! 开发期另记录帧接收时间；不改变字节、顺序或错误，不保存正文。
#[cfg(debug_assertions)]
use std::time::Instant;
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

pub(super) struct ReceiveTrace<S> {
    stream: S,
    header: [u8; 9],
    filled: usize,
    remaining: usize,
    data_header_left: usize,
    received: Arc<AtomicU64>,
    #[cfg(debug_assertions)]
    started: Option<Instant>,
    enabled: bool,
}
impl<S> ReceiveTrace<S> {
    pub fn new(stream: S, received: Arc<AtomicU64>) -> Self {
        Self {
            stream,
            header: [0; 9],
            filled: 0,
            remaining: 0,
            data_header_left: 0,
            received,
            #[cfg(debug_assertions)]
            started: None,
            enabled: true,
        }
    }
    fn receive(&mut self, mut bytes: &[u8]) {
        while self.enabled && !bytes.is_empty() {
            #[cfg(debug_assertions)]
            if self.started.is_none() {
                self.started = Some(Instant::now());
            }
            if self.filled < 4 {
                let count = (4 - self.filled).min(bytes.len());
                self.header[self.filled..self.filled + count].copy_from_slice(&bytes[..count]);
                self.filled += count;
                bytes = &bytes[count..];
                if self.filled < 4 {
                    continue;
                }
                self.remaining =
                    u32::from_be_bytes(self.header[..4].try_into().expect("固定四字节头")) as usize;
                // 非法帧交给原库报错；诊断器停止解析，不影响传输。
                if !(5..=16 * 1024 * 1024).contains(&self.remaining) {
                    self.enabled = false;
                    break;
                }
            }
            if self.filled < 9 {
                let count = (9 - self.filled).min(bytes.len());
                self.header[self.filled..self.filled + count].copy_from_slice(&bytes[..count]);
                self.filled += count;
                self.remaining -= count;
                bytes = &bytes[count..];
                if self.filled == 9 && self.header[4] == 103 {
                    self.data_header_left = 4;
                    #[cfg(debug_assertions)]
                    crate::performance_log!(
                        "[TermRill SFTP wire] phase=data_start id={} frame_bytes={}",
                        u32::from_be_bytes(self.header[5..9].try_into().expect("固定请求编号")),
                        u32::from_be_bytes(self.header[..4].try_into().expect("固定长度字段"))
                    );
                }
            }
            if self.filled == 9 {
                let count = self.remaining.min(bytes.len());
                if self.header[4] == 103 {
                    let skipped = self.data_header_left.min(count);
                    self.data_header_left -= skipped;
                    self.received
                        .fetch_add((count - skipped) as u64, Ordering::Relaxed);
                }
                self.remaining -= count;
                bytes = &bytes[count..];
                if self.remaining == 0 {
                    #[cfg(debug_assertions)]
                    if self.header[4] == 103 {
                        crate::performance_log!("[TermRill SFTP wire] phase=data_complete id={} frame_bytes={} receive_ms={}",
                            u32::from_be_bytes(self.header[5..9].try_into().expect("固定请求编号")),
                            u32::from_be_bytes(self.header[..4].try_into().expect("固定长度字段")),
                            self.started.map_or(0, |started| started.elapsed().as_millis()));
                    }
                    self.filled = 0;
                    #[cfg(debug_assertions)]
                    {
                        self.started = None;
                    }
                }
            }
        }
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for ReceiveTrace<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = output.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, output);
        if matches!(&result, Poll::Ready(Ok(()))) {
            self.receive(&output.filled()[before..]);
        }
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for ReceiveTrace<S> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_counter_excludes_headers_and_counts_partial_payload() {
        let received = Arc::new(AtomicU64::new(0));
        let mut reader = ReceiveTrace::new(tokio::io::empty(), received.clone());
        // 两个 DATA 帧和一个 STATUS 帧；故意将长度字段与正文拆到不同读取中。
        let first = [0, 0, 0, 14, 103, 0, 0, 0, 1, 0, 0, 0, 5, 11, 12, 13, 14, 15];
        reader.receive(&first[..11]);
        assert_eq!(received.load(Ordering::Relaxed), 0);
        reader.receive(&first[11..15]);
        assert_eq!(received.load(Ordering::Relaxed), 2);
        reader.receive(&first[15..]);
        assert_eq!(received.load(Ordering::Relaxed), 5);
        let status = [
            0, 0, 0, 17, 101, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let second = [0, 0, 0, 12, 103, 0, 0, 0, 3, 0, 0, 0, 3, 21, 22, 23];
        reader.receive(&[status.as_slice(), second.as_slice()].concat());
        assert_eq!(received.load(Ordering::Relaxed), 8);
    }
}
