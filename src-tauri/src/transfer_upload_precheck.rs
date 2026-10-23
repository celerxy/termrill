//! 上传只读预检：父目录属性批量复用，并发限于文件句柄预算；全量通过才写入。
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn missing(error: &SftpError) -> bool {
    matches!(error, SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile)
}

async fn directory(
    sftp: &RawSftpSession,
    parent: String,
) -> Result<(Option<BTreeMap<String, FileAttributes>>, bool)> {
    let handle = match sftp.opendir(parent).await {
        Ok(handle) => handle.handle,
        Err(error) if missing(&error) => return Ok((Some(BTreeMap::new()), true)),
        // 只允许按名称投递的目录无法列举，后续逐项 LSTAT，不能假定目标不存在。
        Err(SftpError::Status(status)) if status.status_code == StatusCode::PermissionDenied => {
            return Ok((None, false))
        }
        Err(error) => return Err(error.into()),
    };
    let result = async {
        let mut entries = BTreeMap::new();
        loop {
            match sftp.readdir(handle.clone()).await {
                Ok(batch) => {
                    if batch.files.is_empty() {
                        return Err(AppError::new("目标目录返回空批次，未开始上传"));
                    }
                    for file in batch.files {
                        if [".", ".."].contains(&file.filename.as_str()) {
                            continue;
                        }
                        if file.filename.contains(['/', '\\', '\0']) {
                            return Err(AppError::new("目标目录返回无效名称"));
                        }
                        entries.insert(file.filename, file.attrs);
                        if entries.len() > 100_000 {
                            return Err(AppError::new("目标目录预检项目数超限"));
                        }
                    }
                }
                Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
                Err(error) => return Err(error.into()),
            }
        }
        Ok((Some(entries), false))
    }
    .await;
    let closed = sftp.close(handle).await;
    match result {
        Ok(entries) => {
            closed?;
            Ok(entries)
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    native: &ssh::Connection,
    job: &Job,
    items: &mut [(usize, Item)],
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
) -> Result<()> {
    let started = Instant::now();
    // 按每个父目录的项目数选择策略；不能因为整个任务很大就枚举每个稀疏目录。
    // READDIR 至少有 OPEN、枚举、EOF、CLOSE，最多四项时直接并发 LSTAT。
    let mut parent_counts = BTreeMap::<String, usize>::new();
    for (_, item) in items.iter() {
        let parent = item
            .remote
            .parent()
            .ok_or_else(|| AppError::new("目标路径无父目录"))?;
        *parent_counts.entry(remote(parent)?).or_default() += 1;
    }
    let parents = parent_counts
        .into_iter()
        .filter_map(|(parent, count)| (count > 4).then_some(parent))
        .collect::<BTreeSet<_>>();
    let mut directories = BTreeMap::new();
    let mut absent = BTreeSet::<String>::new();
    let count = native.max_files.clamp(1, 16);
    let mut pending = parents;
    let mut queries = FuturesUnordered::new();
    let directory_started = Instant::now();
    let mut queried = 0usize;
    let mut skipped = 0usize;
    // 预检期间没有写入，不要求先读祖先再读子目录；各目录共享有界句柄预算。
    // 已确认不存在的祖先仍可省去尚未发出的子目录查询。
    while !pending.is_empty() || !queries.is_empty() {
        while queries.len() < count {
            let Some(parent) = pending.pop_first() else {
                break;
            };
            if absent.iter().any(|ancestor| {
                parent
                    .strip_prefix(ancestor.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
            }) {
                directories.insert(parent, Some(BTreeMap::new()));
                skipped += 1;
                continue;
            }
            queried += 1;
            queries.push(async move {
                let started = Instant::now();
                let result = directory(&native.sftp, parent.clone()).await;
                crate::performance_log!(
                    "[TermRill transfer] task={} phase=precheck_directory success={} elapsed_ms={}",
                    job.id,
                    result.is_ok(),
                    started.elapsed().as_millis()
                );
                (parent, result)
            });
        }
        let Some((parent, result)) =
            controlled(queries.next().map(Ok::<_, AppError>), cancel, closed).await?
        else {
            break;
        };
        let (entries, missing) = result?;
        if missing {
            absent.insert(parent.clone());
        }
        directories.insert(parent, entries);
        send(event);
    }
    crate::performance_log!("[TermRill transfer] task={} phase=precheck_directories queried={} skipped={} concurrency={} elapsed_ms={}", job.id, queried, skipped, count, directory_started.elapsed().as_millis());
    let attributes_started = Instant::now();
    let read_budget = tokio::sync::Semaphore::new(64);
    let mut checks = futures::stream::iter(items.iter_mut().map(|(index, item)| {
        let directories = &directories;
        let absent = &absent;
        let read_budget = &read_budget;
        async move {
            cancelled(cancel, closed)?;
            let parent = remote(
                item.remote
                    .parent()
                    .ok_or_else(|| AppError::new("目标路径无父目录"))?,
            )?;
            let name = item
                .remote
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| AppError::new("目标文件名无效"))?;
            let attrs = match directories.get(&parent) {
                Some(Some(entries)) => entries.get(name).cloned(),
                _ if absent.iter().any(|ancestor| parent == *ancestor || parent.strip_prefix(ancestor.as_str()).is_some_and(|suffix| suffix.starts_with('/'))) => None,
                _ => match controlled(
                    async { Ok::<_, AppError>(native.sftp.lstat(remote(&item.remote)?).await) },
                    cancel,
                    closed,
                )
                .await?
                {
                    Ok(attrs) => Some(attrs.attrs),
                    Err(error) if missing(&error) => None,
                    Err(error) => return Err(error.into()),
                },
            };
            let attrs = if attrs
                .as_ref()
                .is_some_and(|attrs| attrs.permissions.is_none())
            {
                Some(
                    controlled(native.sftp.lstat(remote(&item.remote)?), cancel, closed)
                        .await?
                        .attrs,
                )
            } else {
                attrs
            };
            item.remote_target = attrs.map(|attrs| crate::model::FileStat {
                size: attrs.size,
                uid: attrs.uid,
                gid: attrs.gid,
                perm: attrs.permissions,
                #[cfg(test)]
                atime: attrs.atime.map(u64::from),
                mtime: attrs.mtime.map(u64::from),
            });
            if let Some(digest) = job
                .completed
                .get(&item.remote.to_string_lossy().to_string())
            {
                let started = Instant::now();
                let result = verify_completed(native, item, digest, cancel, closed, read_budget).await;
                crate::performance_log!("[TermRill transfer] task={} file_index={} phase=precheck_resume success={} elapsed_ms={}", job.id, index, result.is_ok(), started.elapsed().as_millis());
                result?;
                return Ok::<_, AppError>(item.size);
            }
            if let Some(stat) = &item.remote_target {
                let kind = stat.perm.map(|mode| mode & 0o170000);
                let expected = if item.link.is_some() {
                    0o120000
                } else if item.directory {
                    0o040000
                } else {
                    0o100000
                };
                if kind != Some(expected) || (!item.directory && !job.overwrite) {
                    return Err(AppError::new(format!(
                        "目标已存在或类型不匹配：{}",
                        item.remote.display()
                    )));
                }
            }
            Ok(0)
        }
    }))
    .buffer_unordered(count);
    while let Some(result) =
        controlled(checks.next().map(Ok::<_, AppError>), cancel, closed).await?
    {
        event.bytes += result?;
        send(event);
    }
    crate::performance_log!("[TermRill transfer] task={} phase=precheck_attributes_and_resume concurrency={} elapsed_ms={}", job.id, count, attributes_started.elapsed().as_millis());
    crate::performance_log!(
        "[TermRill transfer] task={} transport=russh phase=precheck elapsed_ms={}",
        job.id,
        started.elapsed().as_millis()
    );
    Ok(())
}

async fn verify_completed(
    native: &ssh::Connection,
    item: &Item,
    digest: &str,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    read_budget: &tokio::sync::Semaphore,
) -> Result<()> {
    if let Some(link) = &item.link {
        let local = tokio::fs::read_link(&item.local).await?;
        let response =
            controlled(native.sftp.readlink(remote(&item.remote)?), cancel, closed).await?;
        if response.files.len() != 1
            || link_digest(link) != digest
            || link_digest(&local) != digest
            || link_digest(Path::new(&response.files[0].filename)) != digest
        {
            return Err(AppError::new("已完成链接目标已变化，请重新创建任务"));
        }
        return Ok(());
    }
    let attrs = tokio::fs::symlink_metadata(&item.local).await?;
    if item.directory
        || local_link(&attrs)
        || !attrs.is_file()
        || !item
            .remote_target
            .as_ref()
            .is_some_and(crate::model::FileStat::is_file)
    {
        return Err(AppError::new("已完成文件类型已变化，请重新创建任务"));
    }
    let handle = controlled(
        native.sftp.open(
            remote(&item.remote)?,
            Flags::READ,
            FileAttributes::default(),
        ),
        cancel,
        closed,
    )
    .await?
    .handle;
    let result = async {
        let mut local = tokio::fs::File::open(&item.local).await?;
        let mut local_hash = Sha256::new();
        let mut remote_hash = Sha256::new();
        let before = controlled(native.sftp.fstat(handle.clone()), cancel, closed)
            .await?
            .attrs;
        if before.size != Some(item.size) || attrs.len() != item.size {
            return Err(AppError::new("已完成文件大小已变化，请重新创建任务"));
        }
        let block = native.download_read_size;
        // 按文件顺序生成摘要；所有文件共享 64 个 READ，每文件最多缓存 16 块。
        let mut reads = futures::stream::iter((0..item.size).step_by(block).map(|at| {
            let handle = handle.clone();
            async move {
                let length = (item.size - at).min(block as u64) as usize;
                let mut bytes = Vec::with_capacity(length);
                while bytes.len() < length {
                    let _permit = controlled(
                        async {
                            read_budget
                                .acquire()
                                .await
                                .map_err(|_| AppError::new("预检请求预算已关闭"))
                        },
                        cancel,
                        closed,
                    )
                    .await?;
                    let data = controlled(
                        native.sftp.read(
                            handle.clone(),
                            at + bytes.len() as u64,
                            (length - bytes.len()) as u32,
                        ),
                        cancel,
                        closed,
                    )
                    .await?
                    .data;
                    if data.is_empty() || data.len() > length - bytes.len() {
                        return Err(AppError::new("已完成文件读取范围不完整"));
                    }
                    bytes.extend(data);
                }
                Ok::<_, AppError>(bytes)
            }
        }))
        .buffered(16);
        while let Some(bytes) = reads.next().await {
            let bytes = bytes?;
            let mut buffer = vec![0; bytes.len()];
            controlled(local.read_exact(&mut buffer), cancel, closed).await?;
            local_hash.update(buffer);
            remote_hash.update(bytes);
        }
        let after = controlled(native.sftp.fstat(handle.clone()), cancel, closed)
            .await?
            .attrs;
        let local_after = local.metadata().await?;
        if after.size != before.size
            || after.mtime != before.mtime
            || local_after.len() != attrs.len()
            || local_after.modified()? != attrs.modified()?
        {
            return Err(AppError::new("已完成文件在校验期间发生变化"));
        }
        if format!("{:x}", local_hash.finalize()) != digest
            || format!("{:x}", remote_hash.finalize()) != digest
        {
            return Err(AppError::new("已完成文件内容已变化，请重新创建任务"));
        }
        Ok(())
    }
    .await;
    let closing = native.sftp.close(handle).await;
    result?;
    closing?;
    Ok(())
}
