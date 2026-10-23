//! 有界日志追踪：只读 SFTP/本地文件，不执行 tail 或其他远程命令。
use crate::{
    model::*,
    remote_async::{attrs, Remote},
};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
};
const BATCH: u64 = 64 * 1024;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cursor {
    pub next: u64,
    pub size: u64,
    pub probe: u32,
    pub head: String,
    pub modified: Option<u64>,
    pub identity: String,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub bytes: String,
    pub cursor: Cursor,
    pub reset: bool,
    pub skipped_prefix: bool,
    pub encoding: &'static str,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn position(
    size: u64,
    head: &[u8],
    modified: Option<u64>,
    identity: &str,
    old: Option<&Cursor>,
) -> Result<(u64, bool)> {
    if size > 9_007_199_254_740_991 {
        return Err(AppError::new("日志文件大小超出精确游标范围"));
    }
    if let Some(old) = old {
        if old.probe > 64
            || old.head.len() != 64
            || old.identity.len() > 4096
            || old.next > old.size
        {
            return Err(AppError::new("日志游标无效"));
        }
        let replaced = size < old.size
            || identity != old.identity
            || digest(&head[..head.len().min(old.probe as usize)]) != old.head
            || (size == old.size && modified != old.modified);
        if !replaced {
            return Ok((old.next, false));
        }
        return Ok((size.saturating_sub(BATCH), true));
    }
    Ok((size.saturating_sub(BATCH), false))
}
fn align_start(start: u64, head: &[u8], fresh: bool) -> u64 {
    if fresh && crate::text_encoding::detect(head, false).starts_with("UTF-16") {
        start & !1
    } else {
        start
    }
}
fn batch(
    bytes: Vec<u8>,
    head: &[u8],
    size: u64,
    start: u64,
    reset: bool,
    modified: Option<u64>,
    identity: String,
) -> Batch {
    Batch {
        cursor: Cursor {
            next: start + bytes.len() as u64,
            size,
            probe: head.len() as u32,
            head: digest(head),
            modified,
            identity,
        },
        skipped_prefix: start > 0 && (reset || start == size.saturating_sub(BATCH)),
        encoding: crate::text_encoding::detect(head, head.len() as u64 >= size),
        bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
        reset,
    }
}
pub fn read_local(path: &Path, old: Option<&Cursor>) -> Result<Batch> {
    let before = crate::local_files::regular(path)?;
    let mut file = fs::File::open(path)?;
    let stat = file.metadata()?;
    if before.len() != stat.len() {
        return Err(AppError::new("日志在打开时变化，请重试"));
    }
    let size = stat.len();
    let modified = stat
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64);
    let identity = format!("{}:{:?}", path.display(), stat.created());
    let mut head = vec![0; size.min(64) as usize];
    file.read_exact(&mut head)?;
    let (start, reset) = position(size, &head, modified, &identity, old)?;
    let start = align_start(start, &head, old.is_none() || reset);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take((size - start).min(BATCH))
        .read_to_end(&mut bytes)?;
    Ok(batch(bytes, &head, size, start, reset, modified, identity))
}
pub async fn read_remote(remote: &Remote, path: &str, old: Option<&Cursor>) -> Result<Batch> {
    let cancel = AtomicBool::new(false);
    let _permit = remote.permit(&cancel).await?;
    crate::remote_delete::parents(remote, path, &cancel).await?;
    let before = remote.stat(path, &cancel).await?;
    if !attrs(&before).is_file() {
        return Err(AppError::new("仅支持普通日志文件"));
    }
    let handle = remote
        .checked(
            remote.native.sftp.open(
                path,
                russh_sftp::protocol::OpenFlags::READ,
                Default::default(),
            ),
            &cancel,
        )
        .await?
        .handle;
    let result = async {
        let opened = remote
            .checked(remote.native.sftp.fstat(handle.clone()), &cancel)
            .await?
            .attrs;
        if !attrs(&opened).is_file() || opened.size != before.size || opened.mtime != before.mtime {
            return Err(AppError::new("日志在打开时变化，请重试"));
        }
        let size = opened
            .size
            .ok_or_else(|| AppError::new("无法取得日志大小"))?;
        let read_size = remote.native.read_size.clamp(1, 32768);
        let head = read_range(remote, &handle, 0, size.min(64), read_size, &cancel).await?;
        let modified = opened.mtime.map(u64::from);
        let (start, reset) = position(size, &head, modified, path, old)?;
        let start = align_start(start, &head, old.is_none() || reset);
        let bytes = read_range(
            remote,
            &handle,
            start,
            (size - start).min(BATCH),
            read_size,
            &cancel,
        )
        .await?;
        let after = remote
            .checked(remote.native.sftp.fstat(handle.clone()), &cancel)
            .await?
            .attrs;
        if after.size.is_none_or(|value| value < size) || !attrs(&after).is_file() {
            return Err(AppError::new("日志读取期间被截断，请重试"));
        }
        Ok(batch(
            bytes,
            &head,
            size,
            start,
            reset,
            modified,
            path.into(),
        ))
    }
    .await;
    let closed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        remote.native.sftp.close(handle),
    )
    .await;
    let result = result?;
    closed.map_err(|_| AppError::new("日志句柄关闭超时"))??;
    Ok(result)
}
async fn read_range(
    remote: &Remote,
    handle: &str,
    mut offset: u64,
    mut count: u64,
    read_size: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    while count > 0 {
        let data = remote
            .checked(
                remote.native.sftp.read(
                    handle.to_owned(),
                    offset,
                    count.min(read_size as u64) as u32,
                ),
                cancel,
            )
            .await?
            .data;
        if data.is_empty() || data.len() as u64 > count {
            return Err(AppError::new("日志读取范围发生变化"));
        }
        offset += data.len() as u64;
        count -= data.len() as u64;
        result.extend_from_slice(&data);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn append_keeps_offset_and_rotation_resets() {
        let first = batch(
            b"one\n".to_vec(),
            b"one\n",
            4,
            0,
            false,
            Some(1),
            "file".into(),
        );
        assert_eq!(
            position(8, b"one\ntwo\n", Some(2), "file", Some(&first.cursor)).unwrap(),
            (4, false)
        );
        assert_eq!(
            position(2, b"x\n", Some(3), "file", Some(&first.cursor)).unwrap(),
            (0, true)
        );
        assert_eq!(
            position(4, b"two\n", Some(1), "file", Some(&first.cursor)).unwrap(),
            (0, true)
        );
        assert!(
            position(4, b"one\n", Some(1), "other", Some(&first.cursor))
                .unwrap()
                .1
        );
    }
}
