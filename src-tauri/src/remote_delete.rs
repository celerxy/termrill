//! 异步删除工作队列；链接只删除自身，失败停止派发并收齐已经发出的请求。
use crate::{
    model::*,
    remote_async::{attrs, Remote},
};
use futures::{stream::FuturesUnordered, StreamExt};
use std::{collections::VecDeque, sync::atomic::AtomicBool};
fn valid(path: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.contains(['\\', '\0'])
        || std::path::Path::new(path).file_name().is_none()
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(AppError::new("不能删除根目录或无效路径"));
    }
    Ok(())
}
pub async fn parents(remote: &Remote, path: &str, cancel: &AtomicBool) -> Result<()> {
    if path == "/" {
        return Ok(());
    }
    valid(path)?;
    for parent in std::path::Path::new(path).ancestors().skip(1) {
        if parent == std::path::Path::new("/") {
            continue;
        }
        if !attrs(&remote.stat(&parent.to_string_lossy(), cancel).await?).is_dir() {
            return Err(AppError::new("路径包含链接或非目录父级"));
        }
    }
    Ok(())
}
pub async fn children_attrs(
    remote: &Remote,
    path: &str,
    cancel: &AtomicBool,
) -> Result<Vec<(String, russh_sftp::protocol::FileAttributes)>> {
    let _permit = remote.permit(cancel).await?;
    let handle = remote
        .checked(remote.native.sftp.opendir(path.to_owned()), cancel)
        .await?
        .handle;
    let result = async {
        let mut names = Vec::new();
        loop {
            match remote
                .checked(
                    async { Ok::<_, AppError>(remote.native.sftp.readdir(handle.clone()).await) },
                    cancel,
                )
                .await?
            {
                Ok(batch) => {
                    if batch.files.is_empty() {
                        return Err(AppError::new("目录返回空批次"));
                    }
                    for entry in batch.files {
                        if [".", ".."].contains(&entry.filename.as_str()) {
                            continue;
                        }
                        if entry.filename.is_empty() || entry.filename.contains(['/', '\\', '\0']) {
                            return Err(AppError::new("服务器返回无效文件名"));
                        }
                        if names.len() >= 100_000 {
                            return Err(AppError::new("目录项目超过 100000 项限制"));
                        }
                        names.push((
                            format!("{}/{}", path.trim_end_matches('/'), entry.filename),
                            entry.attrs,
                        ));
                    }
                }
                Err(russh_sftp::client::error::Error::Status(status))
                    if status.status_code == russh_sftp::protocol::StatusCode::Eof =>
                {
                    break
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(names)
    }
    .await;
    let close = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        remote.native.sftp.close(handle),
    )
    .await;
    match result {
        Ok(names) => {
            close.map_err(|_| AppError::new("关闭目录句柄超时"))??;
            Ok(names)
        }
        Err(error) => Err(error),
    }
}
pub async fn children(remote: &Remote, path: &str, cancel: &AtomicBool) -> Result<Vec<String>> {
    Ok(children_attrs(remote, path, cancel)
        .await?
        .into_iter()
        .map(|(path, _)| path)
        .collect())
}
pub async fn run(
    remote: &Remote,
    path: &str,
    recursive: bool,
    file_only: bool,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    let cancel = AtomicBool::new(false);
    valid(path)?;
    let trace = state::Trace::new();
    progress("检查删除路径");
    if let Err(error) = trace
        .request("parents", parents(remote, path, &cancel))
        .await
    {
        trace.finish(false, 1, 0, 0);
        return Err(error);
    }
    if file_only {
        progress("正在删除文件");
        let result = trace
            .request(
                "unlink",
                remote.checked(remote.native.sftp.remove(path.to_owned()), &cancel),
            )
            .await
            .map(|_| ());
        trace.finish(result.is_ok(), 1, 0, 1);
        return result;
    }
    progress("边扫描边删除，完成子目录立即收尾");
    let result = pipeline::execute(remote, path, recursive, &cancel, &trace).await;
    if result.is_ok() {
        progress("完成");
    }
    result
}

#[path = "delete_async_pipeline.rs"]
mod pipeline;
#[path = "delete_async_state.rs"]
mod state;
