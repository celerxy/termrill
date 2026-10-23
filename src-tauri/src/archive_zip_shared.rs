//! 已校验 ZIP 索引和原始范围跨面板复用；会话隔离，密码和解码内容不落盘。
use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const DISK_LIMIT: u64 = 128 * 1024 * 1024;
struct Block {
    offset: u64,
    length: usize,
    digest: String,
    bytes: Option<Arc<Vec<u8>>>,
    disk: Option<PathBuf>,
}
struct Record {
    remote: Weak<crate::remote_async::Remote>,
    path: String,
    size: Option<u64>,
    mtime: Option<u32>,
    index: Arc<index::Index>,
    blocks: Vec<Block>,
}
fn storage() -> &'static Mutex<VecDeque<Record>> {
    static STORAGE: OnceLock<Mutex<VecDeque<Record>>> = OnceLock::new();
    STORAGE.get_or_init(|| Mutex::new(VecDeque::new()))
}
fn directory() -> &'static OnceLock<PathBuf> {
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
    &DIRECTORY
}
pub(super) fn initialize(path: PathBuf) {
    // 专属原始范围缓存，错误仅导致缓存未命中，不影响读取。
    if std::fs::create_dir_all(&path).is_ok() {
        let _ = directory().set(path);
    }
}
fn matches(record: &Record, reader: &Reader<'_, '_>, path: &str) -> bool {
    record.path == path
        && record.size == reader.before.size
        && record.mtime == reader.before.mtime
        && record
            .remote
            .upgrade()
            .is_some_and(|remote| Arc::ptr_eq(&remote, &reader.remote))
}
pub(super) fn restore(reader: &mut Reader<'_, '_>, path: &str) {
    let blocks = {
        let Ok(mut records) = storage().lock() else {
            return;
        };
        let Some(position) = records.iter().position(|record| {
            matches(record, reader, path)
                && reader
                    .index
                    .as_ref()
                    .is_none_or(|index| index.revision == record.index.revision)
        }) else {
            return;
        };
        let Some(record) = records.remove(position) else {
            return;
        };
        if reader.index.is_none() {
            reader.index = Some(record.index.clone());
        }
        let blocks = record
            .blocks
            .iter()
            .map(|block| {
                (
                    block.offset,
                    block.length,
                    block.digest.clone(),
                    block.bytes.clone(),
                    block.disk.clone(),
                )
            })
            .collect::<Vec<_>>();
        records.push_back(record);
        blocks
    };
    // 磁盘 IO 在归档工作线程执行，绝不持共享缓存锁等待 IO。
    for (offset, length, digest, bytes, disk) in blocks {
        let bytes = bytes.map(|bytes| bytes.as_ref().clone()).or_else(|| {
            let path = disk?;
            if std::fs::metadata(&path).ok()?.len() != length as u64 {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            (format!("{:x}", Sha256::digest(&bytes)) == digest).then_some(bytes)
        });
        if let Some(bytes) = bytes {
            if reader.cache.iter().any(|(cached_offset, cached_bytes)| {
                *cached_offset == offset && cached_bytes.len() == bytes.len()
            }) {
                continue;
            }
            while reader
                .cache
                .iter()
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>()
                + bytes.len()
                > 2 * 1024 * 1024
            {
                reader.cache.pop_front();
            }
            reader.cache.push_back((offset, bytes));
        }
    }
    crate::performance_log!(
        "[TermRill archive] phase=shared_cache blocks={}",
        reader.cache.len()
    );
}
pub(super) fn publish(reader: &Reader<'_, '_>, path: &str) {
    if reader.before.size.is_none() || reader.before.mtime.is_none() {
        return;
    }
    let Some(index) = reader.index.as_ref() else {
        return;
    };
    let Ok(mut records) = storage().lock() else {
        return;
    };
    if let Some(position) = records
        .iter()
        .position(|record| matches(record, reader, path))
    {
        records.remove(position);
    }
    records.retain(|record| {
        record.remote.upgrade().is_some_and(|remote| {
            !remote.native.is_closed() && !remote.closed.load(std::sync::atomic::Ordering::Relaxed)
        })
    });
    records.push_back(Record {
        remote: Arc::downgrade(&reader.remote),
        path: path.to_owned(),
        size: reader.before.size,
        mtime: reader.before.mtime,
        index: index.clone(),
        blocks: reader
            .cache
            .iter()
            .map(|(offset, bytes)| Block {
                offset: *offset,
                length: bytes.len(),
                digest: format!("{:x}", Sha256::digest(bytes)),
                bytes: Some(Arc::new(bytes.clone())),
                disk: None,
            })
            .collect(),
    });
    while records.len() > 16
        || records
            .iter()
            .map(|record| record.index.memory())
            .sum::<usize>()
            > MEMORY_LIMIT
    {
        records.pop_front();
    }
    spill(&mut records);
}
fn spill(records: &mut VecDeque<Record>) {
    let mut memory = records
        .iter()
        .map(|record| {
            record.index.memory()
                + record
                    .blocks
                    .iter()
                    .filter_map(|block| block.bytes.as_ref())
                    .map(|bytes| bytes.len())
                    .sum::<usize>()
        })
        .sum::<usize>();
    for record in records {
        for block in &mut record.blocks {
            let Some(bytes) = block.bytes.as_ref() else {
                continue;
            };
            if let Some(directory) = directory().get() {
                let path = directory.join(format!("{}.block", block.digest));
                // 有界后台写队列，磁盘繁忙时丢弃落盘机会，前台不等待。
                if block.disk.is_none() && writer().try_send((path.clone(), bytes.clone())).is_ok()
                {
                    block.disk = Some(path);
                }
            }
            if memory > MEMORY_LIMIT {
                memory = memory.saturating_sub(bytes.len());
                block.bytes = None;
            }
        }
    }
}
type Write = (PathBuf, Arc<Vec<u8>>);
fn writer() -> &'static std::sync::mpsc::SyncSender<Write> {
    static WRITER: OnceLock<std::sync::mpsc::SyncSender<Write>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Write>(4);
        std::thread::spawn(move || {
            while let Ok((path, bytes)) = receiver.recv() {
                if path.exists() {
                    continue;
                }
                let Some(parent) = path.parent() else {
                    continue;
                };
                let mut files = std::fs::read_dir(parent)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        if entry.path().extension()?.to_str()? != "block" {
                            return None;
                        }
                        let metadata = entry.metadata().ok()?;
                        if !metadata.is_file() {
                            return None;
                        }
                        Some((metadata.modified().ok()?, metadata.len(), entry.path()))
                    })
                    .collect::<Vec<_>>();
                files.sort_by_key(|file| file.0);
                let mut size = files.iter().map(|file| file.1).sum::<u64>();
                for (_, length, old) in files {
                    if size + bytes.len() as u64 <= DISK_LIMIT {
                        break;
                    }
                    if std::fs::remove_file(old).is_ok() {
                        size = size.saturating_sub(length);
                    }
                }
                if size + bytes.len() as u64 <= DISK_LIMIT {
                    let _ = std::fs::write(path, bytes.as_ref());
                }
            }
        });
        sender
    })
}
