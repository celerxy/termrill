//! 下载目录异步并发扫描；确定性排序保证续传编号稳定。
use super::*;
pub(super) async fn scan(remote: &Remote, job: &Job, cancel: &AtomicBool) -> Result<Vec<Item>> {
    crate::remote_delete::parents(remote, &job.remote, cancel).await?;
    let initial = attrs(&remote.stat(&job.remote, cancel).await?);
    let mut pending = std::collections::VecDeque::from([(
        PathBuf::from(&job.local),
        job.remote.clone(),
        initial,
        0usize,
    )]);
    let mut items = Vec::new();
    let mut active = FuturesUnordered::new();
    while !pending.is_empty() || !active.is_empty() {
        while active.len() < 16 {
            let Some((local, path, stat, depth)) = pending.pop_front() else {
                break;
            };
            active.push(async move {
                let mut children = Vec::new();
                cancelled(cancel, &remote.closed)?;
                if depth > 64 {
                    return Err(AppError::new("下载超过层级或项目限制"));
                }
                safe_remote_name(
                    Path::new(&path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| AppError::new("远程名称无效"))?,
                )?;
                if let Some(selection) = &job.selection {
                    let prefix = format!("{path}/");
                    if !selection.contains(&path)
                        && !selection
                            .iter()
                            .any(|selected| selected.starts_with(&prefix))
                    {
                        return Ok::<_, AppError>(None);
                    }
                }
                let symlink = stat.perm.is_some_and(|mode| mode & 0o170000 == 0o120000);
                if !stat.is_file() && !stat.is_dir() && !symlink {
                    return Err(AppError::new("不支持下载特殊文件"));
                }
                let mut link_directory = false;
                let link = if symlink {
                    let target = remote
                        .checked(remote.native.sftp.readlink(path.clone()), cancel)
                        .await?
                        .files
                        .into_iter()
                        .next()
                        .ok_or_else(|| AppError::new("链接目标缺失"))?
                        .filename;
                    #[cfg(windows)]
                    {
                        if target.starts_with('/') || target.contains(':') {
                            return Err(AppError::new("Windows 只支持下载相对链接"));
                        }
                        link_directory = attrs(
                            &remote
                                .checked(remote.native.sftp.stat(path.clone()), cancel)
                                .await?
                                .attrs,
                        )
                        .is_dir();
                    }
                    Some(PathBuf::from(target))
                } else {
                    None
                };
                let item = Item {
                    local: local.clone(),
                    remote: path.clone().into(),
                    directory: stat.is_dir() && !symlink,
                    size: if stat.is_dir() || symlink {
                        0
                    } else {
                        stat.size.ok_or_else(|| AppError::new("远程文件大小缺失"))?
                    },
                    link,
                    link_directory,
                    remote_target: Some(stat.clone()),
                };
                if stat.is_dir() && !symlink {
                    // 重新检查目录类型，READDIR 属性复用于子项，避免逐文件额外 LSTAT。
                    if !attrs(&remote.stat(&path, cancel).await?).is_dir() {
                        return Err(AppError::new("远程目录类型已变化"));
                    }
                    for (child, metadata) in
                        crate::remote_delete::children_attrs(remote, &path, cancel).await?
                    {
                        let name = child
                            .rsplit('/')
                            .next()
                            .ok_or_else(|| AppError::new("远程名称无效"))?;
                        let stat = if metadata.permissions.is_some() {
                            attrs(&metadata)
                        } else {
                            attrs(&remote.stat(&child, cancel).await?)
                        };
                        children.push((local.join(name), child, stat, depth + 1));
                    }
                }
                Ok(Some((item, children)))
            });
        }
        let Some(result) = active.next().await else {
            break;
        };
        if let Some((item, children)) = result? {
            if items.len() + pending.len() + active.len() + children.len() >= 100_000 {
                return Err(AppError::new("下载超过项目限制"));
            }
            items.push(item);
            pending.extend(children);
        }
    }
    // 并发完成顺序不稳定；排序使续传临时文件编号跨重试保持一致。
    items.sort_by(|left, right| left.remote.cmp(&right.remote));
    Ok(items)
}
