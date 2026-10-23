//! 原生异步 SFTP 属性、权限和所有者更新；不执行隐式远程脚本。
use crate::model::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    path: String,
    kind: String,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    size: Option<u64>,
    modified: Option<u64>,
    editable: bool,
}

fn describe(path: &str, stat: crate::model::FileStat) -> Properties {
    let kind = if stat.is_dir() {
        "directory"
    } else if stat.is_file() {
        "file"
    } else if stat
        .perm
        .map(|mode| mode & 0o170000 == 0o120000)
        .unwrap_or(false)
    {
        "symlink"
    } else {
        "other"
    };
    Properties {
        path: path.into(),
        kind: kind.into(),
        mode: stat.perm.map(|mode| mode & 0o7777),
        uid: stat.uid,
        gid: stat.gid,
        size: stat.size,
        modified: stat.mtime,
        editable: (stat.is_file() || stat.is_dir()) && stat.perm.is_some(),
    }
}

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    changed: usize,
    skipped: usize,
    unchanged: usize,
    checked: usize,
    cancelled: bool,
    errors: Vec<String>,
    engine: String,
    fallback_reason: Option<String>,
    workers: usize,
}

/// 进度只发送计数，避免反复复制整个错误列表或泄露当前路径到日志。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgress {
    checked: usize,
    changed: usize,
    unchanged: usize,
    skipped: usize,
    failed: usize,
    elapsed_ms: u64,
    phase: &'static str,
    fallback_reason: Option<String>,
    workers: usize,
}

fn validate_paths(paths: &[String], recursive: bool) -> Result<()> {
    if paths.is_empty() || paths.len() > 100_000 {
        return Err(AppError::new("请选择项目，最多 100000 项"));
    }
    if paths.iter().any(|path| {
        !path.starts_with('/')
            || path.contains(['\\', '\0'])
            || path.len() > 16384
            || path.split('/').any(|part| part == "." || part == "..")
            || (recursive && path.trim_matches('/').is_empty())
    }) {
        return Err(AppError::new("路径无效或尝试递归修改根目录"));
    }
    Ok(())
}

/// 属性读取只发一次 LSTAT；修改保留原有的乐观并发检查与结果核验。
pub async fn inspect_async(remote: &crate::remote_async::Remote, path: &str) -> Result<Properties> {
    if !path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(AppError::new("远程路径必须是绝对路径"));
    }
    let cancel = AtomicBool::new(false);
    Ok(describe(
        path,
        crate::remote_async::attrs(&remote.stat(path, &cancel).await?),
    ))
}
pub async fn update_async(
    remote: &crate::remote_async::Remote,
    path: &str,
    mode: u32,
    expected: u32,
) -> Result<Properties> {
    if mode > 0o777 || expected > 0o7777 {
        return Err(AppError::new("权限值无效"));
    }
    let cancel = AtomicBool::new(false);
    crate::remote_delete::parents(remote, path, &cancel).await?;
    let previous = inspect_async(remote, path).await?;
    if !previous.editable || previous.mode != Some(expected) {
        return Err(AppError::new(
            "项目不支持修改或远程权限已变化，请重新打开属性",
        ));
    }
    let permissions = (expected & 0o7000) | mode;
    remote
        .checked(
            remote.native.sftp.setstat(
                path.to_owned(),
                russh_sftp::protocol::FileAttributes {
                    permissions: Some(permissions),
                    ..Default::default()
                },
            ),
            &cancel,
        )
        .await?;
    let current = inspect_async(remote, path).await?;
    if current.mode != Some(permissions) {
        return Err(AppError::new("修改已发送，但服务器实际权限与请求不一致"));
    }
    Ok(current)
}
pub async fn update_owner_async(
    remote: &crate::remote_async::Remote,
    path: &str,
    owner: (u32, u32),
    expected: (u32, u32),
) -> Result<Properties> {
    let cancel = AtomicBool::new(false);
    crate::remote_delete::parents(remote, path, &cancel).await?;
    let previous = inspect_async(remote, path).await?;
    if !previous.editable || previous.uid != Some(expected.0) || previous.gid != Some(expected.1) {
        return Err(AppError::new(
            "项目不支持修改或所有者已变化，请重新打开属性",
        ));
    }
    remote
        .checked(
            remote.native.sftp.setstat(
                path.to_owned(),
                russh_sftp::protocol::FileAttributes {
                    uid: Some(owner.0),
                    gid: Some(owner.1),
                    ..Default::default()
                },
            ),
            &cancel,
        )
        .await?;
    let current = inspect_async(remote, path).await?;
    if current.uid != Some(owner.0) || current.gid != Some(owner.1) {
        return Err(AppError::new("修改已发送，但服务器实际所有者与请求不一致"));
    }
    Ok(current)
}

/// 先枚举后逐深度修改，确保降低目录权限不会挡住尚未处理的子项。
pub async fn batch_async(
    remote: &crate::remote_async::Remote,
    paths: Vec<String>,
    mode: Option<u32>,
    owner: Option<(u32, u32)>,
    recursive: bool,
    cancel: &AtomicBool,
    progress: &(dyn Fn(BatchProgress) + Sync),
) -> Result<BatchResult> {
    use futures::{stream::FuturesUnordered, StreamExt};
    validate_paths(&paths, recursive)?;
    if mode.is_some_and(|value| value > 0o777) || mode.is_some() == owner.is_some() {
        return Err(AppError::new("权限请求无效"));
    }
    let started = Instant::now();
    let mut result = BatchResult {
        engine: "async-sftp".into(),
        workers: 16,
        ..Default::default()
    };
    let mut pending: std::collections::VecDeque<_> = paths
        .into_iter()
        .map(|path| {
            (
                if path == "/" {
                    path
                } else {
                    path.trim_end_matches('/').to_owned()
                },
                0usize,
            )
        })
        .collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut nodes = std::collections::BTreeMap::<usize, Vec<(String, Properties)>>::new();
    let mut scans = FuturesUnordered::new();
    let mut scan_error = None;
    while !pending.is_empty() || !scans.is_empty() {
        while scans.len() < 16
            && scan_error.is_none()
            && !cancel.load(Ordering::Relaxed)
            && !remote.closed.load(Ordering::Relaxed)
        {
            let Some((path, depth)) = pending.pop_front() else {
                break;
            };
            if depth > 128 || visited.len() >= 100_000 {
                scan_error = Some(AppError::new("权限操作超过层级或项目限制"));
                break;
            }
            if !visited.insert(path.clone()) {
                continue;
            }
            scans.push(async move {
                crate::remote_delete::parents(remote, &path, cancel).await?;
                let previous = inspect_async(remote, &path).await?;
                let children = if recursive && previous.kind == "directory" {
                    crate::remote_delete::children(remote, &path, cancel).await?
                } else {
                    Vec::new()
                };
                Ok::<_, AppError>((path, depth, previous, children))
            });
        }
        let Some(outcome) = scans.next().await else {
            break;
        };
        match outcome {
            Ok((path, depth, previous, children)) => {
                pending.extend(children.into_iter().map(|path| (path, depth + 1)));
                nodes.entry(depth).or_default().push((path, previous));
            }
            Err(error) => {
                scan_error.get_or_insert(error);
            }
        }
    }
    if cancel.load(Ordering::Relaxed) || remote.closed.load(Ordering::Relaxed) {
        result.cancelled = true;
    }
    if !result.cancelled {
        if let Some(error) = scan_error {
            return Err(error);
        }
    }
    if result.cancelled {
        return Ok(result);
    }
    for (_, entries) in nodes.into_iter().rev() {
        let mut pending = entries.into_iter();
        let mut active = FuturesUnordered::new();
        loop {
            while active.len() < 16
                && !cancel.load(Ordering::Relaxed)
                && !remote.closed.load(Ordering::Relaxed)
            {
                let Some((path, previous)) = pending.next() else {
                    break;
                };
                active.push(async move {
                    if !previous.editable {
                        return Ok::<_, AppError>(0u8);
                    }
                    if let Some(mode) = mode {
                        let expected =
                            previous.mode.ok_or_else(|| AppError::new("权限信息缺失"))?;
                        if expected & 0o777 == mode {
                            return Ok(1);
                        }
                        update_async(remote, &path, mode, expected).await?;
                    } else if let Some(owner) = owner {
                        let expected = (
                            previous.uid.ok_or_else(|| AppError::new("UID 缺失"))?,
                            previous.gid.ok_or_else(|| AppError::new("GID 缺失"))?,
                        );
                        if expected == owner {
                            return Ok(1);
                        }
                        update_owner_async(remote, &path, owner, expected).await?;
                    }
                    Ok(2)
                });
            }
            let Some(outcome) = active.next().await else {
                break;
            };
            result.checked += 1;
            match outcome {
                Ok(0) => result.skipped += 1,
                Ok(1) => result.unchanged += 1,
                Ok(_) => result.changed += 1,
                Err(error) => result.errors.push(error.message),
            }
            progress(BatchProgress {
                checked: result.checked,
                changed: result.changed,
                unchanged: result.unchanged,
                skipped: result.skipped,
                failed: result.errors.len(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                phase: "异步并发修改属性",
                fallback_reason: None,
                workers: 16,
            });
        }
        if cancel.load(Ordering::Relaxed) || remote.closed.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
    }
    Ok(result)
}
