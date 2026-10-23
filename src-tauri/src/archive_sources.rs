//! 打包来源扫描：父目录按操作去重，复用 READDIR 属性，最多 16 个异步扫描请求。
//! 属性仅作为本次打包快照；读取时仍验证实际句柄和结束属性，不作为跨操作缓存。
use crate::{
    archive_io::{safe_name, ArchiveIo, Stat, CONTENT_LIMIT, ENTRY_LIMIT},
    model::*,
    remote_async::{attrs, Remote},
};
use futures::{stream, stream::FuturesUnordered, StreamExt};
use russh_sftp::protocol::FileAttributes;
use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::Instant,
};

struct Pending {
    name: String,
    path: PathBuf,
    metadata: Option<FileAttributes>,
}

async fn inspect(
    mut source: Pending,
    remote: &Remote,
    cancel: &AtomicBool,
) -> Result<(Pending, Vec<(String, FileAttributes)>, bool)> {
    // 不完整的 READDIR 属性才回退 LSTAT；已声明的链接和特殊文件直接拒绝。
    let fallback = source.metadata.as_ref().is_none_or(|value| {
        value.permissions.is_none()
            || (attrs(value).is_file() && (value.size.is_none() || value.mtime.is_none()))
    });
    if fallback {
        source.metadata = Some(remote.stat(&source.path.to_string_lossy(), cancel).await?);
    }
    let metadata = source
        .metadata
        .as_ref()
        .ok_or_else(|| AppError::new("打包来源属性缺失"))?;
    let kind = attrs(metadata);
    if !kind.is_file() && !kind.is_dir() {
        return Err(AppError::new("不支持链接或特殊文件"));
    }
    let children = if kind.is_dir() {
        crate::remote_delete::children_attrs(remote, &source.path.to_string_lossy(), cancel).await?
    } else {
        Vec::new()
    };
    Ok((source, children, fallback))
}

pub(super) async fn collect(
    paths: &[String],
    io: &ArchiveIo<'_>,
    remote: &Remote,
) -> Result<Vec<super::Source>> {
    let started = Instant::now();
    let mut pending = VecDeque::new();
    let mut parents = BTreeSet::new();
    for path in paths {
        let path = PathBuf::from(path);
        io.check()?;
        io.valid_path(&path)?;
        let leaf = path
            .file_name()
            .ok_or_else(|| AppError::new("不能打包根目录"))?;
        pending.push_back(Pending {
            name: safe_name(&leaf.to_string_lossy())?,
            path: path.clone(),
            metadata: None,
        });
        parents.extend(path.ancestors().skip(1).map(|parent| parent.to_path_buf()));
    }
    if pending.len() > ENTRY_LIMIT {
        return Err(AppError::new("打包项目过多"));
    }
    let parent_count = parents.len();
    // 收齐已派发的请求后才返回错误，避免中途丢弃仍持有目录句柄的扫描任务。
    let checks: Vec<Result<()>> = stream::iter(parents.into_iter().map(|path| async move {
        let metadata = remote.stat(&path.to_string_lossy(), io.cancel).await?;
        if !attrs(&metadata).is_dir() {
            return Err(AppError::new("父级缺失或不是普通目录"));
        }
        Ok(())
    }))
    .buffer_unordered(16)
    .collect()
    .await;
    for check in checks {
        check?;
    }
    scan(pending, io, remote, started, parent_count).await
}

async fn scan(
    mut pending: VecDeque<Pending>,
    io: &ArchiveIo<'_>,
    remote: &Remote,
    started: Instant,
    parent_count: usize,
) -> Result<Vec<super::Source>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut size = 0u64;
    let mut fallback_count = 0usize;
    let mut directories = 0usize;
    let mut scans = FuturesUnordered::new();
    // 完成一项立即补充，不让同批慢目录挡住其他分支。失败后不再派发，
    // 但必须排空已派发任务，让其执行 READDIR 的 CLOSE。
    let outcome: Result<()> = async {
        while !pending.is_empty() || !scans.is_empty() {
            io.check()?;
            while scans.len() < 16 {
                let Some(source) = pending.pop_front() else {
                    break;
                };
                scans.push(inspect(source, remote, io.cancel));
            }
            if let Some(item) = scans.next().await {
                let (source, children, fallback) = item?;
                fallback_count += usize::from(fallback);
                if !seen.insert(source.name.to_lowercase()) {
                    return Err(AppError::new("打包来源存在重名"));
                }
                let metadata = source
                    .metadata
                    .ok_or_else(|| AppError::new("打包来源属性缺失"))?;
                let directory = attrs(&metadata).is_dir();
                directories += usize::from(directory);
                let length = if directory {
                    0
                } else {
                    metadata
                        .size
                        .ok_or_else(|| AppError::new("无法取得文件大小"))?
                };
                size = size
                    .checked_add(length)
                    .ok_or_else(|| AppError::new("文件大小无效"))?;
                if size > CONTENT_LIMIT || length > 256 * 1024 * 1024 {
                    return Err(AppError::new("打包内容超过限制"));
                }
                if result.len() + pending.len() + scans.len() + children.len() + 1 > ENTRY_LIMIT {
                    return Err(AppError::new("打包项目过多"));
                }
                for (path, metadata) in children {
                    let path = PathBuf::from(path);
                    io.valid_path(&path)?;
                    let leaf = path
                        .file_name()
                        .ok_or_else(|| AppError::new("文件名无效"))?;
                    let name = safe_name(&format!(
                        "{}/{leaf}",
                        source.name,
                        leaf = leaf.to_string_lossy()
                    ))?;
                    pending.push_back(Pending {
                        name,
                        path,
                        metadata: Some(metadata),
                    });
                }
                result.push((
                    source.name,
                    source.path,
                    Stat {
                        directory,
                        size: length,
                        remote: Some(metadata),
                    },
                ));
            }
            io.pack_scan(result.len() - directories, directories);
        }
        Ok(())
    }
    .await;
    while scans.next().await.is_some() {}
    outcome?;
    // 网络完成顺序不影响 ZIP 条目顺序和重名处理。
    result.sort_by(|left, right| left.0.cmp(&right.0));
    crate::performance_log!(
        "[TermRill archive] operation=pack scan_ms={} entries={} bytes={} parent_checks={} source_lstat={}",
        started.elapsed().as_millis(), result.len(), size, parent_count, fallback_count
    );
    Ok(result)
}
