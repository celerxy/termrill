//! 有界只读预览：请求独立取消，读取后校验类型签名，不接受 SVG 等主动内容。
use crate::model::*;
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

const LIMIT: u64 = 32 * 1024 * 1024;
type Requests = BTreeMap<String, Arc<AtomicBool>>;
fn requests() -> &'static Mutex<Requests> {
    static REQUESTS: OnceLock<Mutex<Requests>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub fn begin() -> Result<String> {
    let id = identifier();
    lock(requests())?.insert(id.clone(), Arc::new(AtomicBool::new(false)));
    Ok(id)
}
pub fn cancel(id: &str) -> Result<()> {
    if let Some(flag) = lock(requests())?.remove(id) {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}
pub(crate) fn flag(id: &str) -> Result<Arc<AtomicBool>> {
    lock(requests())?
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::new("操作已取消"))
}
pub fn read_local(
    path: &Path,
    request: &str,
    progress: tauri::ipc::Channel<u64>,
) -> Result<Vec<u8>> {
    let result = (|| {
        let flag = lock(requests())?
            .get(request)
            .cloned()
            .ok_or_else(|| AppError::new("预览已取消"))?;
        let stat = crate::local_files::regular(path)?;
        if stat.len() > LIMIT {
            return Err(AppError::new("预览文件超过 32 MiB"));
        }
        read_stream(&mut std::fs::File::open(path)?, &flag, progress)
    })();
    let _ = cancel(request);
    result
}
fn read_stream(
    file: &mut impl Read,
    flag: &AtomicBool,
    progress: tauri::ipc::Channel<u64>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        if flag.load(Ordering::Relaxed) {
            return Err(AppError::new("预览已取消"));
        }
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > LIMIT {
            return Err(AppError::new("预览文件超过 32 MiB"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        let _ = progress.send(bytes.len() as u64);
    }
    validate(&bytes)?;
    Ok(bytes)
}

pub(crate) fn validate(bytes: &[u8]) -> Result<()> {
    let valid = bytes.starts_with(b"%PDF-")
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || bytes.starts_with(b"BM")
        || bytes.starts_with(b"\x00\x00\x01\x00")
        || (bytes.get(4..8) == Some(b"ftyp")
            && bytes
                .get(8..12)
                .is_some_and(|brand| brand == b"avif" || brand == b"avis"))
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"));
    if !valid {
        return Err(AppError::new("文件内容不是支持的图片或 PDF"));
    }
    Ok(())
}
