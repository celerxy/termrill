//! 递归传输、同名保护及临时文件安装；取消保留已经完成的文件和目录。

use crate::model::*;
use sha2::{Digest, Sha256};
#[cfg(test)]
use ssh2::{RenameFlags, Sftp};
#[cfg(test)]
use std::io::{Seek, SeekFrom};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
use tauri::Emitter;

#[path = "transfer_atomic_replace.rs"]
mod atomic_replace;

#[path = "transfer_async.rs"]
pub(crate) mod native_async;
// 上传优化使用独立模块和连接池，不能改变下载共享实现或后台预热。
#[path = "transfer_upload_async.rs"]
pub(crate) mod native_upload;
#[path = "transfer_upload_journal.rs"]
mod upload_journal;
#[allow(clippy::too_many_arguments)]
async fn upload_execute(
    job: &Job,
    items: &mut [(usize, Item)],
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    backend: Option<&Backend>,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
    limiter: Option<&crate::transfer_rate::RateLimiter>,
) -> Result<()> {
    if items.len() > 1 || items.iter().any(|(_, item)| item.size >= 4 * 1024 * 1024) {
        native_upload::execute(
            job, items, connection, backend, event, cancel, pause, send, checkpoint, limiter,
        )
        .await
    } else {
        native_async::execute(
            job, items, connection, backend, event, cancel, pause, send, checkpoint, limiter,
        )
        .await
    }
}

#[cfg(test)]
#[path = "transfer_parallel.rs"]
mod parallel;

pub(crate) fn maintain_upload_pool(
    connection: Arc<std::sync::Mutex<crate::ssh::Connection>>,
    backend: Arc<Backend>,
) {
    native_upload::pool::warm(connection.clone(), backend.clone());
    native_async::maintain_pool(connection, backend);
}

/// 首屏请求优先；未发起目录读取的客户端最多等待三秒，再按原策略预热。
pub(crate) fn warm_after_first_listing(
    connection: Arc<std::sync::Mutex<crate::ssh::Connection>>,
    backend: Arc<Backend>,
) -> Result<()> {
    let (ready, closed) = {
        let primary = lock(&connection)?;
        (primary.first_listing.clone(), primary.closed.clone())
    };
    tauri::async_runtime::spawn(async move {
        let listed = tokio::time::timeout(std::time::Duration::from_secs(3), ready.notified())
            .await
            .is_ok();
        if closed.load(Ordering::Relaxed) {
            return;
        }
        crate::performance_log!(
            "[TermRill connection] phase=warmup_start first_listing_finished={}",
            listed
        );
        maintain_upload_pool(connection, backend);
    });
    Ok(())
}

/// 大文件预览最多借用三条额外连接；建连失败退回已有会话，不重放已派发读取。
pub(crate) async fn preview_connections(
    primary: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    cancel: &AtomicBool,
) -> Result<Vec<native_async::ssh::Connection>> {
    lease_data_connections(primary, backend, cancel, false).await
}
async fn lease_data_connections(
    primary: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    cancel: &AtomicBool,
    whole_group: bool,
) -> Result<Vec<native_async::ssh::Connection>> {
    use futures::StreamExt;
    let (request, closed, algorithms) = {
        let connection = lock(primary)?;
        (
            connection.request.clone(),
            connection.closed.clone(),
            connection.host_algorithms.clone(),
        )
    };
    let (trusted, count) = {
        let config = lock(&backend.config)?;
        (
            native_async::pool::session_trust(&request, &config.known_hosts),
            config.transfer_settings.concurrency.clamp(1, 4) - usize::from(!whole_group),
        )
    };
    cancelled(cancel, &closed)?;
    let mut pending = futures::stream::iter(0..count)
        .map(|_| async {
            if let Some(connection) = native_async::pool::take(&closed, &trusted)? {
                return Ok(connection);
            }
            native_async::controlled(
                async {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        native_async::pool::tracked(
                            native_async::ssh::Connection::connect(
                                &request,
                                &trusted,
                                &algorithms,
                                Some(backend),
                            ),
                            &closed,
                            &trusted,
                        ),
                    )
                    .await
                    .map_err(|_| AppError::new("预览辅助连接不可用"))?
                },
                cancel,
                &closed,
            )
            .await
        })
        .buffer_unordered(count.max(1));
    let mut peers = Vec::new();
    while let Some(result) = pending.next().await {
        if let Ok(connection) = result {
            peers.push(connection);
        }
    }
    if let Err(error) = cancelled(cancel, &closed) {
        for peer in peers {
            peer.close().await;
        }
        return Err(error);
    }
    Ok(peers)
}

#[path = "archive_session.rs"]
mod archive_session;
pub(crate) use archive_session::{acquire_archive, release_archive};

/// 辅助连接成功收齐正文和 CLOSE 后回池，失败则并行关闭，避免重复握手及串行清理。
pub(crate) async fn release_preview_connections(
    peers: Vec<native_async::ssh::Connection>,
    reusable: bool,
) {
    futures::future::join_all(peers.into_iter().map(|peer| async move {
        if reusable {
            native_async::pool::recycle_preview(peer).await;
        } else {
            peer.close().await;
        }
    }))
    .await;
}

/// 预览独占数据连接：取消/失败销毁会话，完整读取成功才回池，防止旧 DATA 阻塞下次请求。
pub(crate) async fn read_preview(
    primary: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    original: Option<&crate::remote_async::Remote>,
    path: &str,
    cancel: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<Vec<u8>> {
    let (request, closed, algorithms) = {
        let connection = lock(primary)?;
        (
            connection.request.clone(),
            connection.closed.clone(),
            connection.host_algorithms.clone(),
        )
    };
    let trusted = native_async::pool::session_trust(&request, &lock(&backend.config)?.known_hosts);
    cancelled(cancel, &closed)?;
    let started = Instant::now();
    let pooled = native_async::pool::take(&closed, &trusted)?;
    let reused = pooled.is_some();
    let native = match pooled {
        Some(connection) => connection,
        None => {
            native_async::controlled(
                native_async::pool::tracked(
                    native_async::ssh::Connection::connect(
                        &request,
                        &trusted,
                        &algorithms,
                        Some(backend),
                    ),
                    &closed,
                    &trusted,
                ),
                cancel,
                &closed,
            )
            .await?
        }
    };
    crate::performance_log!(
        "[TermRill preview] phase=lease reused={} elapsed_ms={}",
        reused,
        started.elapsed().as_millis()
    );
    let mut remote = crate::remote_async::Remote::owned(native, closed.clone());
    if let Some(original) = original {
        remote.share_preview_cache(original);
    }
    let result = remote
        .read_preview_prepared(path, 32 * 1024 * 1024, cancel, progress, || {
            preview_connections(primary, backend, cancel)
        })
        .await;
    crate::performance_log!(
        "[TermRill preview] phase=complete success={} elapsed_ms={}",
        result.is_ok(),
        started.elapsed().as_millis()
    );
    // 取消后的在途 READ 可能仍在服务端发送；这种连接绝不能回池复用。
    if result.is_ok() && !cancel.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed) {
        native_async::pool::put(remote.native, closed, trusted).await?;
    } else {
        remote.native.close().await;
    }
    result
}

#[path = "transfer_download_async.rs"]
mod download_async;
#[cfg(test)]
pub(crate) use download_async::destination::file as measure_download_file;
#[path = "transfer_download_session.rs"]
mod download_session;
#[path = "transfer_schedule.rs"]
mod schedule;

#[derive(Clone)]
pub(crate) struct Item {
    pub(crate) local: PathBuf,
    pub(crate) remote: PathBuf,
    pub(crate) directory: bool,
    pub(crate) size: u64,
    pub(crate) link: Option<PathBuf>,
    pub(crate) link_directory: bool,
    pub(crate) remote_target: Option<crate::model::FileStat>,
}

fn link_digest(target: &Path) -> String {
    let text = target.to_string_lossy();
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    format!("link:{:x}", Sha256::digest(text.as_bytes()))
}

fn create_local_link(target: &Path, destination: &Path, directory: bool) -> Result<()> {
    #[cfg(windows)]
    let result = if directory {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    };
    #[cfg(unix)]
    let result = {
        let _ = directory;
        std::os::unix::fs::symlink(target, destination)
    };
    result.map_err(|error| {
        AppError::new(format!(
            "创建符号链接失败（Windows 可能需要开发者模式或链接权限）：{error}"
        ))
    })
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub id: String,
    pub connection_id: String,
    pub local: String,
    pub remote: String,
    pub direction: String,
    pub overwrite: bool,
    #[serde(default)]
    pub completed: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub selection: Option<std::collections::BTreeSet<String>>,
}

impl Job {
    pub(crate) fn temporary_name(&self, index: usize) -> String {
        format!(".termrill-{}-{index}.part", self.id)
    }
    pub(crate) fn temporary_prefix(&self) -> String {
        format!(".termrill-{}-", self.id)
    }
}

/// 只补充固定提交阶段，保留指纹挑战与超时标志；日志不记录路径或服务端错误正文。
fn install_stage_error(stage: &str, error: impl Into<AppError>) -> AppError {
    crate::performance_log!("[TermRill transfer] phase=install_failed stage={stage}");
    let mut error = error.into();
    error.message = format!("远程文件提交失败（{stage}）：{}", error.message);
    error
}

/// 只保存路径、目标配置与覆盖选择；认证凭据从不进入任务清单。
pub fn persist_jobs(backend: &Backend) -> Result<()> {
    let jobs = lock(&backend.jobs)?;
    let path = backend.config_path.with_file_name("transfers.json");
    let temporary = path.with_extension("tmp");
    let mut snapshot = fs::File::create(&temporary)?;
    snapshot.write_all(&serde_json::to_vec(&*jobs)?)?;
    snapshot.sync_all()?;
    drop(snapshot);
    fs::rename(temporary, path)?;
    crate::transfer_journal::compact(backend, &jobs)?;
    Ok(())
}
pub(crate) fn cancelled(cancel: &AtomicBool, closed: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) || closed.load(Ordering::Relaxed) {
        return Err(AppError::new("传输已取消"));
    }
    Ok(())
}
fn safe_remote_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| c.is_control())
    {
        return Err(AppError::new(format!("文件名不适用于本地系统：{name}")));
    }
    #[cfg(windows)]
    {
        let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&base.as_str())
            || name.contains(['<', '>', '"', '|', '?', '*'])
        {
            return Err(AppError::new(format!("Windows 不支持此文件名：{name}")));
        }
    }
    Ok(())
}
// 递归扫描显式传入累计结果、深度和取消状态，避免隐藏的全局扫描状态。
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn collect(
    local: &Path,
    remote: &Path,
    upload: bool,
    sftp: Option<&Sftp>,
    items: &mut Vec<Item>,
    depth: usize,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    listed_stat: Option<crate::model::FileStat>,
    selection: Option<&std::collections::BTreeSet<String>>,
) -> Result<()> {
    cancelled(cancel, closed)?;
    if let Some(selected) = selection {
        let key = remote.to_string_lossy().replace('\\', "/");
        let prefix = format!("{key}/");
        if !selected.contains(&key)
            && !selected
                .range(prefix.clone()..)
                .next()
                .is_some_and(|path| path.starts_with(&prefix))
        {
            return Ok(());
        }
    }
    if depth > 64 || items.len() > 100_000 {
        return Err(AppError::new("目录层级或文件数量超出本次传输限制"));
    }
    let (directory, size, symlink) = if upload {
        let stat = fs::symlink_metadata(local)?;
        if !stat.is_file() && !stat.is_dir() && !stat.file_type().is_symlink() {
            return Err(AppError::new("暂不支持传输特殊文件"));
        }
        (stat.is_dir(), stat.len(), local_link(&stat))
    } else {
        // READDIR 已返回属性；复用它，避免每个子项额外一次网络往返。
        // 实际复制时仍重新核对打开文件的属性，不把扫描当作快照。
        let stat = match listed_stat {
            Some(stat) => stat,
            None => sftp
                .ok_or_else(|| AppError::new("下载缺少文件协议连接"))?
                .lstat(remote)?,
        };
        safe_remote_name(
            remote
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| AppError::new("无法解码远程文件名"))?,
        )?;
        if !stat.is_file()
            && !stat.is_dir()
            && !stat.perm.map(|p| p & 0o170000 == 0o120000).unwrap_or(false)
        {
            return Err(AppError::new("暂不支持传输特殊文件"));
        }
        (
            stat.is_dir(),
            stat.size.unwrap_or(0),
            stat.perm.map(|p| p & 0o170000 == 0o120000).unwrap_or(false),
        )
    };
    let link = if symlink {
        if upload && !fs::symlink_metadata(local)?.file_type().is_symlink() {
            return Err(AppError::new("目录联接和其他 reparse point 不支持传输"));
        }
        let target = if upload {
            fs::read_link(local)?
        } else {
            sftp.ok_or_else(|| AppError::new("下载缺少文件协议连接"))?
                .readlink(remote)?
        };
        let text = target
            .to_str()
            .ok_or_else(|| AppError::new("链接目标不是有效 UTF-8"))?;
        #[cfg(windows)]
        if target.is_absolute() || text.starts_with('/') || text.contains(':') {
            return Err(AppError::new("Windows 与远程系统之间只支持相对符号链接"));
        }
        #[cfg(windows)]
        let text = if upload {
            text.replace('\\', "/")
        } else {
            text.to_owned()
        };
        Some(PathBuf::from(text))
    } else {
        None
    };
    #[cfg(windows)]
    let link_directory = if symlink && !upload {
        // Windows 必须区分文件/目录链接；断链无法可靠判断，明确拒绝而不猜测类型。
        sftp.ok_or_else(|| AppError::new("下载缺少文件协议连接"))?
            .stat(remote)
            .map_err(|_| {
                AppError::new("无法判断链接目标类型，Windows 暂不下载断链或不可访问目标的链接")
            })?
            .is_dir()
    } else {
        false
    };
    #[cfg(not(windows))]
    let link_directory = false;
    items.push(Item {
        local: local.into(),
        remote: remote.into(),
        directory: directory && !symlink,
        size: if directory || symlink { 0 } else { size },
        link,
        link_directory,
        remote_target: None,
    });
    if directory && !symlink {
        if upload {
            for entry in fs::read_dir(local)? {
                let entry = entry?;
                collect(
                    &entry.path(),
                    &remote.join(entry.file_name()),
                    upload,
                    sftp,
                    items,
                    depth + 1,
                    cancel,
                    closed,
                    None,
                    selection,
                )?;
            }
        } else {
            for (path, stat) in sftp
                .ok_or_else(|| AppError::new("下载缺少文件协议连接"))?
                .readdir(remote)?
            {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| AppError::new("无法解码远程文件名"))?;
                if [".", ".."].contains(&name) {
                    continue;
                }
                safe_remote_name(name)?;
                collect(
                    &local.join(name),
                    &path,
                    upload,
                    sftp,
                    items,
                    depth + 1,
                    cancel,
                    closed,
                    Some(stat),
                    selection,
                )?;
            }
        }
    }
    Ok(())
}

pub fn run(
    app: tauri::AppHandle,
    backend: Arc<Backend>,
    job: Job,
    cancel: Arc<AtomicBool>,
    connection: Arc<std::sync::Mutex<crate::ssh::Connection>>,
) {
    let send = |event: &TransferEvent| {
        // 终态必须等注册表和任务清单清理后再发送，否则立即重试会撞上旧任务。
        if !matches!(event.status.as_str(), "completed" | "cancelled" | "failed") {
            let _ = app.emit("transfer-progress", event);
        }
    };
    let pause = backend
        .pauses
        .lock()
        .ok()
        .and_then(|pauses| pauses.get(&job.id).cloned())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let limit = backend
        .config
        .lock()
        .map(|config| config.transfer_settings.bytes_per_second)
        .unwrap_or(0);
    let limiter = crate::transfer_rate::RateLimiter::new(limit);
    let result = if job.direction == "upload" {
        let (mut event, persisted) =
            upload_journal::with_checkpoints(&backend, &job.id, |checkpoint| {
                execute_upload(
                    &job,
                    &cancel,
                    &pause,
                    &connection,
                    &send,
                    checkpoint,
                    &backend,
                    &limiter,
                )
            });
        if let Err(error) = persisted {
            event.status = "failed".into();
            event.error = Some(error.message);
        }
        event
    } else {
        let mut event = TransferEvent {
            id: job.id.clone(),
            connection_id: job.connection_id.clone(),
            name: Path::new(&job.remote)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            direction: "download".into(),
            status: "connecting".into(),
            bytes: 0,
            total: 0,
            speed: 0,
            remaining_seconds: None,
            error: None,
        };
        let result =
            crate::transfer_journal::with_download_checkpoints(&backend, &job.id, |checkpoint| {
                tauri::async_runtime::block_on(async {
                    download_session::execute(
                        &job,
                        &connection,
                        &backend,
                        &mut event,
                        &cancel,
                        &pause,
                        &send,
                        checkpoint,
                        &limiter,
                    )
                    .await
                })
            });
        event.status = match result {
            Ok(()) => "completed",
            Err(error) => {
                event.error = Some(error.message);
                if cancel.load(Ordering::Relaxed) {
                    "cancelled"
                } else {
                    "failed"
                }
            }
        }
        .into();
        event.speed = 0;
        event
    };
    if result.status != "failed" {
        if let Ok(mut jobs) = backend.jobs.lock() {
            jobs.remove(&job.id);
        }
    }
    if let Ok(mut pauses) = backend.pauses.lock() {
        pauses.remove(&job.id);
    }
    if let Ok(mut transfers) = backend.transfers.lock() {
        transfers.remove(&job.id);
    }
    if let Err(error) = persist_jobs(&backend) {
        let _ = app.emit("transfer-persistence-error", error.message);
    }
    let _ = app.emit("transfer-progress", &result);
}

#[cfg(test)]
pub(crate) fn execute(
    job: &Job,
    cancel: &AtomicBool,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    send: impl Fn(&TransferEvent),
) -> TransferEvent {
    execute_controlled(
        job,
        cancel,
        &AtomicBool::new(false),
        connection,
        send,
        |_, _| Ok(()),
    )
}

#[cfg(test)]
pub(crate) fn execute_controlled(
    job: &Job,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    send: impl Fn(&TransferEvent),
    checkpoint: impl Fn(&str, String) -> Result<()>,
) -> TransferEvent {
    execute_inner(
        job,
        cancel,
        pause,
        connection,
        &send,
        &checkpoint,
        None,
        None,
        None,
    )
}

#[cfg(test)]
pub(crate) fn execute_concurrent(
    job: &Job,
    cancel: &AtomicBool,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    send: impl Fn(&TransferEvent),
    checkpoint: impl Fn(&str, String) -> Result<()>,
) -> TransferEvent {
    execute_concurrent_controlled(
        job,
        cancel,
        &AtomicBool::new(false),
        connection,
        backend,
        send,
        checkpoint,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_concurrent_controlled(
    job: &Job,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    send: impl Fn(&TransferEvent),
    checkpoint: impl Fn(&str, String) -> Result<()>,
) -> TransferEvent {
    execute_inner(
        job,
        cancel,
        pause,
        connection,
        &send,
        &checkpoint,
        Some(backend),
        None,
        None,
    )
}

// 工作线程复用同一文件执行器；固定原扫描下标，保持断线重试的临时文件名稳定。
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn execute_inner(
    job: &Job,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
    backend: Option<&Backend>,
    plan: Option<&std::sync::Mutex<std::collections::VecDeque<(usize, Item)>>>,
    limiter: Option<&crate::transfer_rate::RateLimiter>,
) -> TransferEvent {
    let name = if job.direction == "upload" {
        Path::new(&job.local)
    } else {
        Path::new(&job.remote)
    }
    .file_name()
    .unwrap_or_default()
    .to_string_lossy()
    .into_owned();
    let mut event = TransferEvent {
        id: job.id.clone(),
        connection_id: job.connection_id.clone(),
        name,
        direction: job.direction.clone(),
        status: "queued".into(),
        bytes: 0,
        total: 0,
        speed: 0,
        remaining_seconds: None,
        error: None,
    };
    send(&event);
    let result = (|| -> Result<()> {
        // 不持有应用层连接锁等待暂停，让浏览和编辑仍能取得连接句柄。
        // 长任务独占 SFTP 子系统，不能与浏览复用同一子系统的请求状态。
        // 任务持有连接保持跳板存活，暂停期间不占用应用连接锁。
        let upload = job.direction == "upload";
        let (session, sftp, closed) = {
            let conn = lock(connection)?;
            (
                conn.session.clone(),
                if upload {
                    None
                } else {
                    Some(Arc::new(conn.session.sftp()?))
                },
                conn.closed.clone(),
            )
        };
        cancelled(cancel, &closed)?;
        event.status = "scanning".into();
        send(&event);
        let prepared = plan.is_some();
        let scan_started = Instant::now();
        let mut indexed_items = if plan.is_some() {
            Vec::new()
        } else {
            let mut items = vec![];
            collect(
                Path::new(&job.local),
                Path::new(&job.remote),
                upload,
                sftp.as_deref(),
                &mut items,
                0,
                cancel,
                &closed,
                None,
                job.selection.as_ref(),
            )?;
            items.into_iter().enumerate().collect()
        };
        event.total = indexed_items.iter().map(|(_, i)| i.size).sum();
        crate::performance_log!(
            "[TermRill transfer] phase=scan prepared={prepared} items={} bytes={} elapsed_ms={}",
            indexed_items.len(),
            event.total,
            scan_started.elapsed().as_millis()
        );
        if upload {
            return tauri::async_runtime::block_on(upload_execute(
                job,
                &mut indexed_items,
                connection,
                backend,
                &mut event,
                cancel,
                pause,
                send,
                checkpoint,
                limiter,
            ));
        }
        let sftp = sftp.ok_or_else(|| AppError::new("下载缺少文件协议连接"))?;
        let precheck_started = Instant::now();
        // Validate all conflicts before creating any destination files.
        for (_, item) in &mut indexed_items {
            // 并发工作计划已经过协调线程的完整预检；安装阶段仍检查实际目标，保留竞态保护。
            if prepared {
                continue;
            }
            if let Some(digest) = job
                .completed
                .get(&item.remote.to_string_lossy().to_string())
            {
                if let Some(target) = &item.link {
                    let local_target = fs::read_link(&item.local)?;
                    let remote_target = sftp.readlink(&item.remote)?;
                    if link_digest(target) != *digest
                        || link_digest(&local_target) != *digest
                        || link_digest(&remote_target) != *digest
                    {
                        return Err(AppError::new("已完成链接目标已变化，请重新创建任务"));
                    }
                    continue;
                }
                if item.directory {
                    return Err(AppError::new("任务清单中的文件类型已变化"));
                }
                let local_stat = fs::symlink_metadata(&item.local)?;
                let remote_stat = sftp.lstat(&item.remote)?;
                if local_link(&local_stat) || !local_stat.is_file() || !remote_stat.is_file() {
                    return Err(AppError::new("已完成文件被替换为其他类型，请重新创建任务"));
                }
                // 目录重试时核对两端摘要，不能仅凭文件名或大小跳过已完成内容。
                if hash_reader(fs::File::open(&item.local)?, cancel, &closed)? != *digest
                    || hash_reader(sftp.open(&item.remote)?, cancel, &closed)? != *digest
                {
                    return Err(AppError::new("已完成文件内容发生变化，请重新创建任务"));
                }
                event.bytes += item.size;
                continue;
            }
            for ancestor in item
                .local
                .ancestors()
                .skip(usize::from(item.link.is_some()))
            {
                if let Ok(stat) = fs::symlink_metadata(ancestor) {
                    if local_link(&stat) {
                        return Err(AppError::new(format!(
                            "目标路径包含符号链接或目录联接：{}",
                            ancestor.display()
                        )));
                    }
                }
            }
            match fs::symlink_metadata(&item.local) {
                Ok(stat) => {
                    if (item.link.is_some() && (!stat.file_type().is_symlink() || !job.overwrite))
                        || (item.link.is_none() && local_link(&stat))
                        || (item.link.is_none() && stat.is_dir() != item.directory)
                        || (!item.directory && !job.overwrite)
                    {
                        return Err(AppError::new(format!(
                            "目标已存在或类型不匹配：{}",
                            item.local.display()
                        )));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        crate::performance_log!(
            "[TermRill transfer] phase=precheck prepared={prepared} elapsed_ms={}",
            precheck_started.elapsed().as_millis()
        );
        send(&event);
        if !prepared {
            if let Some(backend) = backend {
                if let Some(result) = parallel::execute(
                    job,
                    cancel,
                    pause,
                    connection,
                    backend,
                    &indexed_items,
                    &mut event,
                    send,
                    limiter,
                )? {
                    return result;
                }
            }
        }
        // 并发执行器会先创建预检确认过的目标目录；这段时间保持“扫描目录”，
        // 等首批数据真正写出后再显示“传输中”。
        event.status = "running".into();
        send(&event);
        let start = Instant::now();
        let mut last = Instant::now();
        // 下载复用 1 MiB 堆缓冲，使 libssh2 可以预发送多个协议包；
        // 短写和 EAGAIN 重用同一缓冲，失败后仍校验实际临时前缀。
        let mut copy_buffer = vec![0u8; 1024 * 1024];
        let mut local_items = indexed_items.into_iter();
        let mut completed_files = 0usize;
        loop {
            // 共享队列仅在领取时加锁；整个文件的网络操作仍由工作连接独占。
            cancelled(cancel, &closed)?;
            let next = match plan {
                Some(queue) => lock(queue)?.pop_front(),
                None => local_items.next(),
            };
            let Some((index, item)) = next else {
                break;
            };
            if plan.is_some() {
                event.total = event.total.saturating_add(item.size);
            }
            completed_files += 1;
            cancelled(cancel, &closed)?;
            if let Some(target) = &item.link {
                if job
                    .completed
                    .contains_key(&item.remote.to_string_lossy().to_string())
                {
                    continue;
                }
                let current_target = sftp.readlink(&item.remote)?;
                if link_digest(&current_target) != link_digest(target) {
                    return Err(AppError::new("源链接目标在扫描后已变化，请重新创建任务"));
                }
                // 链接覆盖使用独立流程，不使用普通文件安装时的 chmod。
                if job.overwrite && fs::symlink_metadata(&item.local).is_ok() {
                    let temporary = item
                        .local
                        .with_file_name(format!(".termrill-link-{}", identifier()));
                    create_local_link(target, &temporary, item.link_directory)?;
                    let result = crate::links::install_local(&temporary, &item.local);
                    if result.is_err() {
                        let _ = crate::links::remove_local(&temporary);
                    }
                    result?;
                } else {
                    create_local_link(target, &item.local, item.link_directory)?;
                }
                checkpoint(&item.remote.to_string_lossy(), link_digest(target))?;
                continue;
            }
            if item.directory {
                fs::create_dir_all(&item.local)?;
                continue;
            }
            if job
                .completed
                .contains_key(&item.remote.to_string_lossy().to_string())
            {
                continue;
            }
            let mut digest = Sha256::new();
            let previous_bytes = event.bytes;
            let parent = item
                .local
                .parent()
                .ok_or_else(|| AppError::new("目标目录无效"))?;
            let temporary = parent.join(job.temporary_name(index));
            let mut stage = "打开下载文件及校验续传前缀";
            let result = (|| -> Result<()> {
                if let Ok(stat) = fs::symlink_metadata(&temporary) {
                    if !stat.is_file() || local_link(&stat) {
                        return Err(AppError::new("续传临时路径不是普通文件"));
                    }
                }
                let mut source = sftp.open(&item.remote)?;
                let original = source.stat()?;
                if !original.is_file() || original.size != Some(item.size) {
                    return Err(AppError::new("源文件在扫描后已变化，请重新创建任务"));
                }
                let mut dest = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&temporary)?;
                resume_prefix(
                    &mut source,
                    &mut dest,
                    item.size,
                    &mut event,
                    cancel,
                    &closed,
                    &mut digest,
                )?;
                stage = "下载文件数据";
                copy(
                    &session,
                    &mut source,
                    &mut dest,
                    &mut event,
                    cancel,
                    pause,
                    &closed,
                    &mut last,
                    start,
                    &send,
                    &mut digest,
                    &mut copy_buffer,
                    limiter,
                )?;
                stage = "保存本地文件并核对源文件";
                let finalize_start = Instant::now();
                dest.sync_all()?;
                drop(dest);
                let current = source.stat()?;
                if event.bytes - previous_bytes != item.size
                    || current.size != original.size
                    || current.mtime != original.mtime
                {
                    return Err(AppError::new("源文件在传输中已变化，未替换目标文件"));
                }
                cancelled(cancel, &closed)?;
                crate::performance_log!("[TermRill transfer] task={} direction=download phase=flush_and_source_check elapsed_ms={}", job.id, finalize_start.elapsed().as_millis());
                stage = "安装本地目标文件";
                let install_start = Instant::now();
                if job.overwrite {
                    fs::rename(&temporary, &item.local)?;
                } else {
                    fs::hard_link(&temporary, &item.local)?;
                    fs::remove_file(&temporary)?;
                }
                crate::performance_log!(
                    "[TermRill transfer] task={} direction=download phase=install elapsed_ms={}",
                    job.id,
                    install_start.elapsed().as_millis()
                );
                Ok(())
            })();
            if result.is_err() && cancel.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&temporary);
            }
            result.map_err(|error| transfer_error(&item.remote, stage, error))?;
            send(&event);
            let checkpoint_start = Instant::now();
            let digest = format!("{:x}", digest.finalize());
            checkpoint(&item.remote.to_string_lossy(), digest)?;
            crate::performance_log!(
                "[TermRill transfer] task={} direction={} phase={} elapsed_ms={}",
                job.id,
                job.direction,
                if plan.is_some() {
                    "checkpoint_queue"
                } else {
                    "checkpoint"
                },
                checkpoint_start.elapsed().as_millis()
            );
        }
        crate::performance_log!("[TermRill transfer] phase=worker_done dynamic={} files={completed_files} bytes={} elapsed_ms={}", plan.is_some(), event.bytes, start.elapsed().as_millis());
        Ok(())
    })();
    match result {
        Ok(()) => {
            event.status = "completed".into();
            event.speed = 0;
        }
        Err(e) => {
            event.status = if cancel.load(Ordering::Relaxed) {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            event.error = Some(e.message);
            event.speed = 0;
        }
    }
    send(&event);
    event
}
fn hash_reader(mut reader: impl Read, cancel: &AtomicBool, closed: &AtomicBool) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        cancelled(cancel, closed)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

// 显式传入源、目标、计数、取消和摘要状态，保持复制阶段的状态所有权清楚。
#[allow(clippy::too_many_arguments)]
/// 续传前逐字节核对临时文件前缀；源文件改变时拒绝续传，不拼接旧内容。
#[cfg(test)]
fn resume_prefix(
    source: &mut (impl Read + Seek),
    destination: &mut (impl Read + Write + Seek),
    size: u64,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    digest: &mut Sha256,
) -> Result<()> {
    let offset = destination.seek(SeekFrom::End(0))?;
    if offset > size {
        return Err(AppError::new("续传临时文件大于源文件，请取消并重新传输"));
    }
    destination.seek(SeekFrom::Start(0))?;
    verify_prefix(source, destination, offset, event, cancel, closed, digest)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn verify_prefix(
    source: &mut impl Read,
    destination: &mut impl Read,
    offset: u64,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    digest: &mut Sha256,
) -> Result<()> {
    let mut remaining = offset;
    let mut original = [0u8; 65536];
    let mut saved = [0u8; 65536];
    while remaining > 0 {
        cancelled(cancel, closed)?;
        let count = remaining.min(original.len() as u64) as usize;
        source.read_exact(&mut original[..count])?;
        destination.read_exact(&mut saved[..count])?;
        if original[..count] != saved[..count] {
            return Err(AppError::new(format!(
                "源文件内容已变化，请取消并重新传输（前缀偏移 {}）",
                offset - remaining
            )));
        }
        digest.update(&original[..count]);
        remaining -= count as u64;
    }
    event.bytes += offset;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// 每个块检查取消；进度最多每 100ms 推送一次，speed 为任务平均字节/秒。
#[cfg(test)]
fn copy(
    session: &ssh2::Session,
    source: &mut impl Read,
    dest: &mut impl Write,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    closed: &AtomicBool,
    last: &mut Instant,
    start: Instant,
    send: &impl Fn(&TransferEvent),
    digest: &mut Sha256,
    buffer: &mut [u8],
    limiter: Option<&crate::transfer_rate::RateLimiter>,
) -> Result<()> {
    // 独立会话在数据阶段使用非阻塞调用；EAGAIN 等待不会作为失败，也不占用浏览会话。
    let _mode = crate::transfer_io::Nonblocking::new(session);
    loop {
        cancelled(cancel, closed)?;
        if pause.load(Ordering::Relaxed) {
            event.status = "paused".into();
            event.speed = 0;
            send(event);
            // 保持文件偏移和临时文件；暂停期间仍检查取消/断开，不能永久等待。
            while pause.load(Ordering::Relaxed) {
                cancelled(cancel, closed)?;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            event.status = "running".into();
            send(event);
        }
        let block_size = limiter.map_or(buffer.len(), |limiter| limiter.block_size(buffer.len()));
        let n = crate::transfer_io::read_network(
            session,
            source,
            &mut buffer[..block_size],
            cancel,
            closed,
            pause,
        )?;
        if n == 0 {
            // 最后一块可能不到事件节流间隔；先发布真实字节数，再进入关闭和安装。
            send(event);
            break;
        }
        digest.update(&buffer[..n]);
        if let Some(limiter) = limiter {
            limiter.wait(n, cancel, closed, pause, || {
                if event.status != "paused" {
                    event.status = "paused".into();
                    event.speed = 0;
                    send(event);
                }
            })?;
            if event.status == "paused" {
                event.status = "running".into();
                send(event);
            }
        }
        crate::transfer_io::write_network(
            session,
            dest,
            &buffer[..n],
            cancel,
            closed,
            pause,
            |count| {
                // 按底层成功返回的字节更新，短写期间也能显示进展，不提前计算预发送数据。
                event.bytes += count as u64;
                if last.elapsed().as_millis() >= 100 {
                    event.speed =
                        (event.bytes as f64 / start.elapsed().as_secs_f64().max(0.001)) as u64;
                    send(event);
                    *last = Instant::now();
                }
            },
        )?;
    }
    Ok(())
}

// 安装阶段重新检查目标类型；预扫描后服务器目录仍可能被其他进程修改。
#[cfg(test)]
pub(crate) fn install_remote(
    sftp: &Sftp,
    temporary: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<()> {
    if !overwrite {
        sftp.rename(temporary, destination, Some(RenameFlags::empty()))?;
        return Ok(());
    }
    let previous = match sftp.lstat(destination) {
        Ok(stat) => Some(stat),
        Err(error) if error.code() == ssh2::ErrorCode::SFTP(2) => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(stat) = previous {
        if !stat.is_file() {
            return Err(AppError::new("覆盖目标的类型已变化，已停止保存"));
        }
        if let Some(mode) = stat.perm {
            sftp.setstat(
                temporary,
                crate::model::FileStat {
                    perm: Some(mode & 0o777),
                    size: None,
                    uid: None,
                    gid: None,
                    atime: None,
                    mtime: None,
                },
            )?;
        }
        let backup = destination.with_file_name(format!(".termrill-backup-{}", identifier()));
        sftp.rename(destination, &backup, Some(RenameFlags::empty()))?;
        if let Err(error) = sftp.rename(temporary, destination, Some(RenameFlags::empty())) {
            if sftp
                .rename(&backup, destination, Some(RenameFlags::empty()))
                .is_err()
            {
                return Err(AppError::new(format!(
                    "替换失败：{error}；原文件保存在 {}",
                    backup.display()
                )));
            }
            return Err(error.into());
        }
        sftp.unlink(&backup).map_err(|error| {
            AppError::new(format!(
                "文件已替换，但原文件备份 {} 未能清理：{error}",
                backup.display()
            ))
        })?;
    } else {
        sftp.rename(temporary, destination, Some(RenameFlags::empty()))?;
    }
    Ok(())
}

/// 错误保留路径和阶段；不自动重放可能已成功的远端写入。
#[cfg(test)]
fn transfer_error(path: &Path, stage: &str, error: AppError) -> AppError {
    let detail = if error.message.to_ascii_lowercase().contains("timeout") {
        format!(
            "SSH/SFTP 等待远端响应时连接中断。临时文件保留，请重试/续传；原始错误：{}",
            error.message
        )
    } else {
        error.message
    };
    AppError::new(format!("{stage}失败：{}；{detail}", path.display()))
}

#[allow(clippy::too_many_arguments)]
fn execute_upload(
    job: &Job,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
    backend: &Backend,
    limiter: &crate::transfer_rate::RateLimiter,
) -> TransferEvent {
    let mut event = TransferEvent {
        id: job.id.clone(),
        connection_id: job.connection_id.clone(),
        name: Path::new(&job.local)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        direction: "upload".into(),
        status: "scanning".into(),
        bytes: 0,
        total: 0,
        speed: 0,
        remaining_seconds: None,
        error: None,
    };
    send(&event);
    let result = (|| {
        let closed = lock(connection)?.closed.clone();
        let mut items = Vec::new();
        collect_local(
            Path::new(&job.local),
            Path::new(&job.remote),
            &mut items,
            0,
            cancel,
            &closed,
            job.selection.as_ref(),
        )?;
        let mut items = items.into_iter().enumerate().collect::<Vec<_>>();
        event.total = items.iter().map(|(_, item)| item.size).sum();
        tauri::async_runtime::block_on(upload_execute(
            job,
            &mut items,
            connection,
            Some(backend),
            &mut event,
            cancel,
            pause,
            send,
            checkpoint,
            Some(limiter),
        ))
    })();
    event.status = match result {
        Ok(()) => "completed",
        Err(error) => {
            event.error = Some(error.message);
            if cancel.load(Ordering::Relaxed) {
                "cancelled"
            } else {
                "failed"
            }
        }
    }
    .into();
    event.speed = 0;
    event
}

#[cfg(test)]
pub(crate) fn execute_native_download(
    job: &Job,
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
) -> TransferEvent {
    let mut event = TransferEvent {
        id: job.id.clone(),
        connection_id: job.connection_id.clone(),
        name: "fixture".into(),
        direction: "download".into(),
        status: "queued".into(),
        bytes: 0,
        total: 0,
        speed: 0,
        remaining_seconds: None,
        error: None,
    };
    let result = tauri::async_runtime::block_on(download_session::execute(
        job,
        connection,
        backend,
        &mut event,
        cancel,
        pause,
        send,
        checkpoint,
        &crate::transfer_rate::RateLimiter::new(0),
    ));
    event.status = match result {
        Ok(()) => "completed",
        Err(error) => {
            event.error = Some(error.message);
            if cancel.load(Ordering::Relaxed) {
                "cancelled"
            } else {
                "failed"
            }
        }
    }
    .into();
    event
}

#[allow(clippy::too_many_arguments)]
fn collect_local(
    local: &Path,
    remote: &Path,
    items: &mut Vec<Item>,
    depth: usize,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    selection: Option<&std::collections::BTreeSet<String>>,
) -> Result<()> {
    cancelled(cancel, closed)?;
    if depth > 64 || items.len() >= 100_000 {
        return Err(AppError::new("上传目录层级或项目数量超限"));
    }
    let key = remote.to_string_lossy().replace('\\', "/");
    if let Some(selection) = selection {
        let prefix = format!("{key}/");
        if !selection.contains(&key) && !selection.iter().any(|path| path.starts_with(&prefix)) {
            return Ok(());
        }
    }
    let stat = fs::symlink_metadata(local)?;
    let symlink = stat.file_type().is_symlink();
    if local_link(&stat) && !symlink {
        return Err(AppError::new("不支持上传目录联接或其他 reparse point"));
    }
    if !stat.is_dir() && !stat.is_file() && !symlink {
        return Err(AppError::new("不支持上传特殊文件"));
    }
    let link = if symlink {
        let target = fs::read_link(local)?;
        let text = target
            .to_str()
            .ok_or_else(|| AppError::new("链接目标不是 UTF-8"))?;
        #[cfg(windows)]
        if target.is_absolute() || text.starts_with('/') || text.contains(':') {
            return Err(AppError::new("只支持相对符号链接"));
        }
        Some(PathBuf::from(text.replace('\\', "/")))
    } else {
        None
    };
    items.push(Item {
        local: local.to_owned(),
        remote: remote.to_owned(),
        directory: stat.is_dir() && !symlink,
        size: if stat.is_dir() || symlink {
            0
        } else {
            stat.len()
        },
        link,
        link_directory: false,
        remote_target: None,
    });
    if stat.is_dir() && !symlink {
        for child in fs::read_dir(local)? {
            let child = child?;
            collect_local(
                &child.path(),
                &remote.join(child.file_name()),
                items,
                depth + 1,
                cancel,
                closed,
                selection,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_path_escape_and_windows_aliases() {
        for name in [
            "..",
            ".",
            "../file",
            "a\\b",
            "file:stream",
            "name.",
            "name ",
            "x\0y",
        ] {
            assert!(safe_remote_name(name).is_err(), "{name}");
        }
        #[cfg(windows)]
        for name in ["CON", "nul.txt", "LPT1.log", "a?b"] {
            assert!(safe_remote_name(name).is_err(), "{name}");
        }
        assert!(safe_remote_name("中文文件.txt").is_ok());
    }
}
