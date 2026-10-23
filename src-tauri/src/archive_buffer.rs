//! 压缩包生成缓冲区逐次写入限制，防止在 finish 前无界增长。
use crate::archive_io::ARCHIVE_LIMIT;
use std::io::{Cursor, Seek, SeekFrom, Write};
#[derive(Default)]
pub struct ArchiveBuffer(
    Cursor<Vec<u8>>,
    Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
);
impl ArchiveBuffer {
    pub fn tracked(counter: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Self {
        Self(Cursor::default(), Some(counter))
    }
    pub fn into_inner(self) -> Vec<u8> {
        self.0.into_inner()
    }
}
impl Write for ArchiveBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.position().saturating_add(bytes.len() as u64) > ARCHIVE_LIMIT {
            return Err(std::io::Error::other("生成的压缩包超过 512 MiB"));
        }
        let count = self.0.write(bytes)?;
        if let Some(counter) = &self.1 {
            counter.store(
                self.0.get_ref().len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl Seek for ArchiveBuffer {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let position = self.0.seek(position)?;
        if position > ARCHIVE_LIMIT {
            return Err(std::io::Error::other("压缩包写入位置超过上限"));
        }
        Ok(position)
    }
}
