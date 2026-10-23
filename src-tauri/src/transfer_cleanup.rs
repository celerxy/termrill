//! 移除任务前只清理任务拥有的 .part 文件，不删除正式文件、链接或备份。
use crate::{model::*, transfer::Job};
use std::{fs, path::Path};
fn belongs_to(name: &str, job: &Job) -> bool {
    name.strip_prefix(&job.temporary_prefix())
        .and_then(|suffix| suffix.strip_suffix(".part"))
        .is_some_and(|index| !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
}
/// 下载清理只访问本地磁盘，父级与临时项目都不能是链接或目录联接。
pub fn cleanup(job: &Job) -> Result<()> {
    if job.id.is_empty()
        || !job
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AppError::new("任务标识无效"));
    }
    let destination = Path::new(&job.local);
    if !destination.is_absolute()
        || destination.file_name().is_none()
        || destination
            .to_string_lossy()
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
    {
        return Err(AppError::new("清理路径无效"));
    }
    let recursive = match fs::symlink_metadata(destination) {
        Ok(stat) if crate::local_link(&stat) || (!stat.is_dir() && !stat.is_file()) => {
            return Err(AppError::new("任务目标类型已变化"))
        }
        Ok(stat) => stat.is_dir(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let root = if recursive {
        destination
    } else {
        destination
            .parent()
            .ok_or_else(|| AppError::new("清理目录无效"))?
    };
    for parent in root.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(stat) if crate::local_link(&stat) || !stat.is_dir() => {
                return Err(AppError::new("清理父级包含链接或非目录"))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    if !recursive {
        let temporary = root.join(job.temporary_name(0));
        return remove_local(&temporary);
    }
    let mut pending = std::collections::VecDeque::from([(root.to_owned(), 0usize)]);
    let mut count = 0;
    while let Some((path, depth)) = pending.pop_front() {
        if depth > 128 {
            return Err(AppError::new("清理目录层级超限"));
        }
        for child in fs::read_dir(path)? {
            let child = child?;
            count += 1;
            if count > 100_000 {
                return Err(AppError::new("清理项目数量超限"));
            }
            let stat = fs::symlink_metadata(child.path())?;
            if belongs_to(&child.file_name().to_string_lossy(), job) {
                remove_local(&child.path())?;
            } else if stat.is_dir() && !crate::local_link(&stat) {
                pending.push_back((child.path(), depth + 1));
            }
        }
    }
    Ok(())
}
fn remove_local(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(stat) if !crate::local_link(&stat) && stat.is_file() => {
            fs::remove_file(path).map_err(Into::into)
        }
        Ok(_) => Err(AppError::new("临时路径类型已变化")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
/// 原生异步清理只删除任务拥有的普通 .part 文件；正式目标、链接和备份保留。
pub async fn cleanup_async(job: &Job, remote: &crate::remote_async::Remote) -> Result<()> {
    use std::sync::atomic::AtomicBool;
    if job.id.is_empty()
        || !job
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AppError::new("任务标识无效"));
    }
    let cancel = AtomicBool::new(false);
    crate::remote_delete::parents(remote, &job.remote, &cancel).await?;
    let target = match remote
        .checked(
            async { Ok::<_, AppError>(remote.native.sftp.lstat(job.remote.clone()).await) },
            &cancel,
        )
        .await?
    {
        Ok(value) => Some(crate::remote_async::attrs(&value.attrs)),
        Err(error) if crate::remote_async::missing(&error) => None,
        Err(error) => return Err(error.into()),
    };
    if target
        .as_ref()
        .is_some_and(|stat| !stat.is_dir() && !stat.is_file())
    {
        return Err(AppError::new("清理目标类型已变化"));
    }
    let recursive = target.as_ref().is_some_and(|stat| stat.is_dir());
    let root = if recursive {
        job.remote.clone()
    } else {
        Path::new(&job.remote)
            .parent()
            .ok_or_else(|| AppError::new("目标目录无效"))?
            .to_string_lossy()
            .into_owned()
    };
    if !recursive {
        let temporary = format!("{}/{}", root.trim_end_matches('/'), job.temporary_name(0));
        match remote
            .checked(
                async { Ok::<_, AppError>(remote.native.sftp.lstat(temporary.clone()).await) },
                &cancel,
            )
            .await?
        {
            Ok(value) if crate::remote_async::attrs(&value.attrs).is_file() => {
                remote
                    .checked(remote.native.sftp.remove(temporary), &cancel)
                    .await?;
            }
            Err(error) if crate::remote_async::missing(&error) => {}
            Ok(_) => return Err(AppError::new("临时路径类型已变化")),
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }
    let mut pending = std::collections::VecDeque::from([(root, 0usize)]);
    let mut count = 0;
    while let Some((path, depth)) = pending.pop_front() {
        if depth > 128 {
            return Err(AppError::new("清理目录层级超限"));
        }
        let listing = remote
            .list(
                &path,
                crate::directory::Filter::with_size(0, String::new(), true, 100_000)?,
                &cancel,
            )
            .await?;
        if listing.page.has_more {
            return Err(AppError::new("清理项目数量超限"));
        }
        for entry in listing.listing.entries {
            count += 1;
            if count > 100_000 {
                return Err(AppError::new("清理项目数量超限"));
            }
            if entry.symlink {
                continue;
            }
            if entry.directory {
                pending.push_back((entry.path, depth + 1));
            } else if belongs_to(&entry.name, job) {
                // READDIR 只用于筛选；删除前重新核验自身类型与父级。
                crate::remote_delete::parents(remote, &entry.path, &cancel).await?;
                if !crate::remote_async::attrs(&remote.stat(&entry.path, &cancel).await?).is_file()
                {
                    return Err(AppError::new("临时路径类型已变化"));
                }
                remote
                    .checked(remote.native.sftp.remove(entry.path), &cancel)
                    .await?;
            }
        }
    }
    Ok(())
}
