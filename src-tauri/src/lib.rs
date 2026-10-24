//! Tauri 命令入口：将阻塞工作移到工作线程，管理配置与会话资源。

mod archive_buffer;
mod archive_io;
mod archives;
mod auth;
mod credentials;
#[cfg(test)]
mod delete_parallel;
mod directory;
mod directory_sync;
mod directory_sync_options;
mod directory_sync_rules;
mod epub;
mod files;
#[cfg(all(test, windows))]
mod kvm_read_perf;
mod links;
mod local_files;
mod log_follow;
mod model;
mod performance;
mod permissions;
mod preview;
#[cfg(test)]
mod reliability_tests;
mod remote_async;
mod remote_delete;
mod sftp_packet;
#[cfg(not(test))]
mod ssh;
#[cfg(test)]
#[path = "ssh_legacy.rs"]
mod ssh;
mod terminal;
#[cfg(test)]
mod tests;
mod text_chunks;
mod text_encoding;
mod transfer;
mod transfer_cleanup;
#[cfg(test)]
mod transfer_io;
mod transfer_journal;
mod transfer_rate;
mod workbench_remote;
mod workbench_store;

#[tauri::command]
fn delete_timing(total_ms: u64, refresh_ms: u64, success: bool) {
    crate::performance_log!("[TermRill delete] phase=ui_complete total_ms={total_ms} refresh_ms={refresh_ms} success={success}");
}

use model::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Manager, State};
type Shared<'a> = State<'a, Arc<Backend>>;

/// 开发模式启动计时只记录耗时，不包含连接、路径或认证数据。
struct StartupClock(std::time::Instant);

#[tauri::command]
fn startup_ready(navigation_ms: u64, diagnostics: Option<String>, clock: State<'_, StartupClock>) {
    if cfg!(debug_assertions) {
        eprintln!(
            "[TermRill startup] first frame: process={} ms, navigation={} ms",
            clock.0.elapsed().as_millis(),
            navigation_ms
        );
        if let Some(details) = diagnostics.filter(|value| value.len() < 8192) {
            eprintln!("[TermRill startup] resource timings: {details}");
        }
    }
}

#[tauri::command]
async fn sync_rules(
    scope: String,
    rule: Option<directory_sync_rules::Rule>,
    remove: Option<String>,
    state: Shared<'_>,
) -> Result<Vec<directory_sync_rules::Rule>> {
    let path = state.config_path.with_file_name("sync-rules.json");
    blocking(move || directory_sync_rules::access(&path, &scope, rule, remove)).await
}

#[tauri::command]
async fn compare_directories(
    id: String,
    connection_id: String,
    local: String,
    remote: String,
    direction: String,
    options: Option<directory_sync_options::Options>,
    state: Shared<'_>,
) -> Result<Vec<directory_sync::Difference>> {
    let conn = connection(&state, &connection_id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut transfers = lock(&state.transfers)?;
        if transfers.contains_key(&id) || transfers.len() >= 32 {
            return Err(AppError::new("比较任务已存在或任务数超过限制"));
        }
        transfers.insert(id.clone(), cancel.clone());
    }
    let backend = state.inner().clone();
    let result = async {
        let session = remote_async::get(&conn, &backend).await?;
        directory_sync::compare_with_options(
            &session,
            Path::new(&local),
            Path::new(&remote),
            &direction,
            &cancel,
            options.unwrap_or_default(),
        )
        .await
    }
    .await;
    lock(&backend.transfers)?.remove(&id);
    result
}

#[tauri::command]
async fn remote_properties(
    id: String,
    path: String,
    state: Shared<'_>,
) -> Result<permissions::Properties> {
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    permissions::inspect_async(&remote, &path).await
}
#[tauri::command]
async fn set_remote_permissions(
    id: String,
    path: String,
    mode: u32,
    expected_mode: u32,
    state: Shared<'_>,
) -> Result<permissions::Properties> {
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    permissions::update_async(&remote, &path, mode, expected_mode).await
}

#[tauri::command]
fn credential_support() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

#[tauri::command]
async fn set_remote_owner(
    id: String,
    path: String,
    uid: u32,
    gid: u32,
    expected_uid: u32,
    expected_gid: u32,
    state: Shared<'_>,
) -> Result<permissions::Properties> {
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    permissions::update_owner_async(&remote, &path, (uid, gid), (expected_uid, expected_gid)).await
}

#[tauri::command]
async fn set_remote_permissions_batch(
    id: String,
    paths: Vec<String>,
    mode: u32,
    recursive: bool,
    task_id: String,
    on_progress: tauri::ipc::Channel<permissions::BatchProgress>,
    state: Shared<'_>,
) -> Result<permissions::BatchResult> {
    let conn = connection(&state, &id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut tasks = lock(&state.transfers)?;
        if task_id.is_empty()
            || task_id.len() > 64
            || tasks.contains_key(&task_id)
            || tasks.len() >= 32
        {
            return Err(AppError::new("权限任务标识无效或任务数量超限"));
        }
        tasks.insert(task_id.clone(), cancel.clone());
    }
    let backend = state.inner().clone();
    let result = async {
        let remote = remote_async::get(&conn, &backend).await?;
        permissions::batch_async(
            &remote,
            paths,
            Some(mode),
            None,
            recursive,
            &cancel,
            &|value| {
                let _ = on_progress.send(value);
            },
        )
        .await
    }
    .await;
    lock(&backend.transfers)?.remove(&task_id);
    result
}

#[derive(serde::Deserialize)]
struct BatchOwner {
    uid: u32,
    gid: u32,
}

#[tauri::command]
async fn set_remote_owner_batch(
    id: String,
    paths: Vec<String>,
    owner: BatchOwner,
    recursive: bool,
    task_id: String,
    on_progress: tauri::ipc::Channel<permissions::BatchProgress>,
    state: Shared<'_>,
) -> Result<permissions::BatchResult> {
    let conn = connection(&state, &id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut tasks = lock(&state.transfers)?;
        if task_id.is_empty()
            || task_id.len() > 64
            || tasks.contains_key(&task_id)
            || tasks.len() >= 32
        {
            return Err(AppError::new("权限任务标识无效或任务数量超限"));
        }
        tasks.insert(task_id.clone(), cancel.clone());
    }
    let backend = state.inner().clone();
    let result = async {
        let remote = remote_async::get(&conn, &backend).await?;
        permissions::batch_async(
            &remote,
            paths,
            None,
            Some((owner.uid, owner.gid)),
            recursive,
            &cancel,
            &|value| {
                let _ = on_progress.send(value);
            },
        )
        .await
    }
    .await;
    lock(&backend.transfers)?.remove(&task_id);
    result
}

#[tauri::command]
fn answer_auth(id: String, answers: Option<Vec<String>>, state: Shared<'_>) -> Result<()> {
    let sender = lock(&state.auth_answers)?
        .remove(&id)
        .ok_or_else(|| AppError::new("认证提示已过期"))?;
    sender
        .try_send(answers)
        .map_err(|_| AppError::new("认证提示已结束"))
}

#[tauri::command]
async fn save_credentials(profile: Profile, secrets: credentials::SavedSecrets) -> Result<()> {
    blocking(move || credentials::store(&profile, &secrets)).await
}
#[tauri::command]
async fn load_credentials(profile: Profile) -> Result<Option<credentials::SavedSecrets>> {
    blocking(move || credentials::load(&profile)).await
}
#[tauri::command]
async fn delete_credentials(profile: Profile) -> Result<()> {
    blocking(move || credentials::remove(&profile)).await
}

// 本地磁盘和归档解码 API 会阻塞；避免占用 Tauri 异步执行线程。
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(e.to_string()))?
}
fn persist(backend: &Backend, config: &Config) -> Result<()> {
    let temporary = backend.config_path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(config)?)?;
    fs::rename(temporary, &backend.config_path)?;
    Ok(())
}
fn connection(backend: &Backend, id: &str) -> Result<Arc<Mutex<ssh::Connection>>> {
    lock(&backend.connections)?
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::new("连接已关闭，请重新连接"))
}
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
#[tauri::command]
fn load_profiles(state: Shared<'_>) -> Result<Vec<Profile>> {
    Ok(lock(&state.config)?.profiles.clone())
}
#[tauri::command]
async fn workbench_preferences(
    update: Option<workbench_store::Preferences>,
    state: Shared<'_>,
) -> Result<workbench_store::Preferences> {
    let path = state.config_path.with_file_name("workbench.json");
    blocking(move || workbench_store::access(&path, update)).await
}

// 只在用户打开工作台并选择采集/执行后调用；拒绝并发堆积，未开启时不创建通道。
static WORKBENCH_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);
#[tauri::command]
async fn workbench_command(
    connection_id: String,
    command: String,
    state: Shared<'_>,
) -> Result<workbench_remote::CommandOutput> {
    workbench_store::validate_command(&command)?;
    let _slot = WORKBENCH_SLOTS
        .try_acquire()
        .map_err(|_| AppError::new("工作台请求繁忙，请稍后再试"))?;
    let conn = connection(&state, &connection_id)?;
    let remote = remote_async::get(&conn, &state).await?;
    remote
        .checked(
            remote.native.workbench_command(&command),
            &AtomicBool::new(false),
        )
        .await
}
#[tauri::command]
async fn server_metrics(
    connection_id: String,
    state: Shared<'_>,
) -> Result<workbench_remote::Metrics> {
    workbench_remote::admit_sample(&connection_id)?;
    let _slot = WORKBENCH_SLOTS
        .try_acquire()
        .map_err(|_| AppError::new("状态采集繁忙"))?;
    let conn = connection(&state, &connection_id)?;
    let remote = remote_async::get(&conn, &state).await?;
    let output = remote
        .checked(
            remote
                .native
                .workbench_command(workbench_remote::METRICS_SCRIPT),
            &AtomicBool::new(false),
        )
        .await?;
    workbench_remote::parse(&output)
}
#[tauri::command]
fn load_preferences(state: Shared<'_>) -> Result<bool> {
    Ok(lock(&state.config)?.show_hidden)
}
#[tauri::command]
fn load_transfer_settings(state: Shared<'_>) -> Result<TransferSettings> {
    Ok(lock(&state.config)?.transfer_settings.clone())
}
#[tauri::command]
fn save_transfer_settings(settings: TransferSettings, state: Shared<'_>) -> Result<()> {
    settings.validate()?;
    let mut config = lock(&state.config)?;
    let previous = config.transfer_settings.clone();
    config.transfer_settings = settings;
    if let Err(error) = persist(&state, &config) {
        config.transfer_settings = previous;
        return Err(error);
    }
    Ok(())
}
#[tauri::command]
fn save_preferences(show_hidden: bool, state: Shared<'_>) -> Result<()> {
    let mut config = lock(&state.config)?;
    let previous = config.show_hidden;
    config.show_hidden = show_hidden;
    if let Err(e) = persist(&state, &config) {
        config.show_hidden = previous;
        return Err(e);
    }
    Ok(())
}
#[tauri::command]
fn save_profiles(profiles: Vec<Profile>, state: Shared<'_>) -> Result<()> {
    if profiles.len() > 200 {
        return Err(AppError::new("最多保存 200 个连接"));
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in &profiles {
        p.target.validate()?;
        if let Some(jump) = &p.jump {
            jump.validate()?;
        }
        if p.name.trim().is_empty() || !ids.insert(&p.id) {
            return Err(AppError::new("连接名称不能为空，连接标识不能重复"));
        }
    }
    let mut config = lock(&state.config)?;
    let previous = std::mem::replace(&mut config.profiles, profiles);
    if let Err(e) = persist(&state, &config) {
        config.profiles = previous;
        return Err(e);
    }
    Ok(())
}
#[tauri::command]
fn trust_host(host_key: HostKey, state: Shared<'_>) -> Result<()> {
    let mut pending = lock(&state.pending_hosts)?;
    let challenge = pending
        .get(&host_key.address)
        .ok_or_else(|| AppError::new("主机确认已过期，请重新连接"))?;
    if challenge.fingerprint != host_key.fingerprint || challenge.previous != host_key.previous {
        return Err(AppError::new("主机指纹已变化，请重新连接"));
    }
    let mut config = lock(&state.config)?;
    let old = config
        .known_hosts
        .insert(host_key.address.clone(), host_key.fingerprint);
    if let Err(e) = persist(&state, &config) {
        if let Some(old) = old {
            config.known_hosts.insert(host_key.address, old);
        } else {
            config.known_hosts.remove(&host_key.address);
        }
        return Err(e);
    }
    pending.remove(&host_key.address);
    Ok(())
}
#[tauri::command]
fn known_hosts(state: Shared<'_>) -> Result<BTreeMap<String, String>> {
    Ok(lock(&state.config)?.known_hosts.clone())
}
#[tauri::command]
fn forget_host(address: String, state: Shared<'_>) -> Result<()> {
    let mut config = lock(&state.config)?;
    let previous = config.known_hosts.remove(&address);
    if let Err(e) = persist(&state, &config) {
        if let Some(previous) = previous {
            config.known_hosts.insert(address, previous);
        }
        return Err(e);
    }
    Ok(())
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Connected {
    id: String,
}
#[tauri::command]
async fn connect(request: ConnectRequest, state: Shared<'_>) -> Result<Connected> {
    let backend = state.inner().clone();
    connect_session(request, backend).await
}

async fn connect_session(request: ConnectRequest, backend: Arc<Backend>) -> Result<Connected> {
    let started = std::time::Instant::now();
    let conn = ssh::connect_async(request, &backend).await?;
    let id = identifier();
    lock(&backend.connection_profiles)?.insert(id.clone(), conn.request.profile.clone());
    lock(&backend.closed)?.insert(id.clone(), conn.closed.clone());
    let conn = Arc::new(Mutex::new(conn));
    lock(&backend.connections)?.insert(id.clone(), conn.clone());
    transfer::warm_after_first_listing(conn, backend)?;
    // 认证及 SFTP 已可用即可交还会话；首屏枚举走独立分页入口，失败不丢弃已认证连接。
    crate::performance_log!(
        "[TermRill connection] phase=ready elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(Connected { id })
}
#[tauri::command]
// 先发停止信号再移除句柄，传输无需等待连接锁即可在下一次复制检查时退出。
fn disconnect(id: String, state: Shared<'_>) -> Result<()> {
    if let Some(closed) = lock(&state.closed)?.remove(&id) {
        closed.store(true, Ordering::Relaxed);
    }
    lock(&state.connections)?.remove(&id);
    lock(&state.connection_profiles)?.remove(&id);
    for control in lock(&state.terminals)?.values() {
        if control.connection_id == id {
            control.stop.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}
#[tauri::command]
async fn list_remote_page(
    id: String,
    path: String,
    offset: usize,
    query: String,
    show_hidden: bool,
    state: Shared<'_>,
) -> Result<directory::PageListing> {
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    let listing = remote
        .list(
            &path,
            directory::Filter::browser(offset, query, show_hidden)?,
            &AtomicBool::new(false),
        )
        .await;
    // 成功或失败均让后台预热继续；目录错误不能让连接池永远不启动。
    lock(&primary)?.first_listing.notify_one();
    listing
}

#[tauri::command]
async fn list_local_drives() -> Result<Vec<String>> {
    blocking(local_files::drives).await
}

#[tauri::command]
async fn list_local_page(
    path: Option<String>,
    offset: usize,
    query: String,
    show_hidden: bool,
) -> Result<directory::PageListing> {
    blocking(move || {
        let path = path.map(PathBuf::from).unwrap_or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });
        directory::local(&path, directory::Filter::new(offset, query, show_hidden)?)
    })
    .await
}

#[tauri::command]
async fn list_remote(
    id: String,
    path: String,
    on_progress: tauri::ipc::Channel<ssh::ListingBatch>,
    state: Shared<'_>,
) -> Result<Listing> {
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    let listing = remote
        .list(
            &path,
            directory::Filter::with_size(0, String::new(), true, 100_000)?,
            &AtomicBool::new(false),
        )
        .await?;
    if listing.page.has_more {
        return Err(AppError::new("目录超过 100000 项，请使用分页浏览"));
    }
    let _ = on_progress.send(ssh::ListingBatch {
        path: listing.listing.path.clone(),
        parent: listing.listing.parent.clone(),
        count: listing.listing.entries.len(),
        entries: listing.listing.entries.clone(),
        reset: true,
    });
    Ok(listing.listing)
}
#[tauri::command]
async fn list_local(
    path: Option<String>,
    on_progress: tauri::ipc::Channel<ssh::ListingBatch>,
) -> Result<Listing> {
    blocking(move || {
        let path = path.map(PathBuf::from).unwrap_or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });
        let path = fs::canonicalize(path)?;
        let listing_path = display_path(&path);
        let parent = path.parent().map(display_path);
        let mut entries = vec![];
        let mut delivered = 0;
        let mut reported = std::time::Instant::now();
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            let stat = match fs::symlink_metadata(entry.path()) {
                Ok(s) => s,
                Err(_) => continue,
            };
            entries.push(FileEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: display_path(&entry.path()),
                directory: stat.is_dir(),
                symlink: local_link(&stat),
                size: stat.len(),
                modified: stat
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|t| t.as_secs()),
            });
            if entries.len() >= 256
                && (entries.len() - delivered >= 2048
                    || reported.elapsed() >= std::time::Duration::from_millis(200))
            {
                let _ = on_progress.send(ssh::ListingBatch {
                    path: listing_path.clone(),
                    parent: parent.clone(),
                    entries: entries[delivered..].to_vec(),
                    count: entries.len(),
                    reset: delivered == 0,
                });
                delivered = entries.len();
                reported = std::time::Instant::now();
            }
        }
        if delivered > 0 && delivered < entries.len() {
            let _ = on_progress.send(ssh::ListingBatch {
                path: listing_path.clone(),
                parent: parent.clone(),
                entries: entries[delivered..].to_vec(),
                count: entries.len(),
                reset: false,
            });
        }
        Ok(Listing {
            path: listing_path,
            parent,
            entries: sorted(entries),
        })
    })
    .await
}
fn display_path(path: &Path) -> String {
    let p = path.to_string_lossy();
    if let Some(unc) = p.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        p.strip_prefix("\\\\?\\").unwrap_or(&p).to_string()
    }
}
#[tauri::command]
async fn pick_path(kind: String) -> Result<Vec<String>> {
    blocking(move || {
        let dialog = rfd::FileDialog::new();
        let paths: Vec<PathBuf> = match kind.as_str() {
            "folder" => dialog
                .set_title("选择文件夹")
                .pick_folder()
                .into_iter()
                .collect(),
            "files" => dialog
                .set_title("选择上传文件")
                .pick_files()
                .unwrap_or_default(),
            "key" => dialog
                .set_title("选择 SSH 私钥")
                .pick_file()
                .into_iter()
                .collect(),
            _ => return Err(AppError::new("无效的文件选择类型")),
        };
        Ok(paths.iter().map(|p| display_path(p)).collect())
    })
    .await
}
#[tauri::command]
async fn create_directory(id: Option<String>, path: String, state: Shared<'_>) -> Result<()> {
    if let Some(id) = id {
        let primary = connection(&state, &id)?;
        let remote = remote_async::get(&primary, &state).await?;
        let cancel = AtomicBool::new(false);
        remote_delete::parents(&remote, &path, &cancel).await?;
        remote
            .checked(
                remote.native.sftp.mkdir(
                    path,
                    russh_sftp::protocol::FileAttributes {
                        permissions: Some(0o755),
                        ..Default::default()
                    },
                ),
                &cancel,
            )
            .await
            .map(|_| ())
    } else {
        blocking(move || {
            fs::create_dir(path)?;
            Ok(())
        })
        .await
    }
}
#[tauri::command]
async fn rename_entry(
    id: Option<String>,
    source: String,
    destination: String,
    state: Shared<'_>,
) -> Result<()> {
    if let Some(id) = id {
        let primary = connection(&state, &id)?;
        let remote = remote_async::get(&primary, &state).await?;
        let cancel = AtomicBool::new(false);
        remote_delete::parents(&remote, &source, &cancel).await?;
        remote_delete::parents(&remote, &destination, &cancel).await?;
        // SFTP v3 RENAME 保留目标已存在时的失败语义，不使用覆盖扩展。
        remote
            .checked(remote.native.sftp.rename(source, destination), &cancel)
            .await
            .map(|_| ())
    } else {
        blocking(move || {
            if Path::new(&destination).exists() {
                return Err(AppError::new("目标已存在"));
            }
            fs::rename(source, destination)?;
            Ok(())
        })
        .await
    }
}
#[tauri::command]
async fn delete_entry(
    id: Option<String>,
    path: String,
    recursive: Option<bool>,
    file_only: Option<bool>,
    on_progress: tauri::ipc::Channel<String>,
    state: Shared<'_>,
) -> Result<()> {
    if let Some(id) = id {
        let started = std::time::Instant::now();
        let primary = connection(&state, &id)?;
        let remote = remote_async::get(&primary, &state).await?;
        performance_log!(
            "[TermRill delete] phase=acquire elapsed_ms={}",
            started.elapsed().as_millis()
        );
        remote_delete::run(
            &remote,
            &path,
            recursive.unwrap_or(false),
            file_only.unwrap_or(false),
            &|phase| {
                let _ = on_progress.send(phase.into());
            },
        )
        .await
    } else {
        blocking(move || {
            files::delete_local_observed(Path::new(&path), recursive.unwrap_or(false), &|phase| {
                let _ = on_progress.send(phase.into());
            })
        })
        .await
    }
}
#[tauri::command]
fn release_archive_snapshot(id: Option<String>, snapshot: String, path: String) {
    archives::release(id.as_deref().unwrap_or("local"), &snapshot, &path);
}

#[tauri::command]
async fn archive_operation(
    id: Option<String>,
    request: archives::Request,
    operation_id: String,
    on_progress: tauri::ipc::Channel<String>,
    state: Shared<'_>,
) -> Result<archives::Reply> {
    let started = std::time::Instant::now();
    let operation = match request.operation.as_str() {
        "list" => "list",
        "preview" => "preview",
        "verify" => "verify",
        "extract" => "extract",
        "pack" => "pack",
        "add" => "add",
        "remove" => "remove",
        _ => "unknown",
    };
    let scope = id.clone().unwrap_or_else(|| "local".into());
    let flag = preview::flag(&operation_id)?;
    let dedicated = id.is_some() && matches!(operation, "pack" | "add");
    let remote = if let Some(id) = id {
        let primary = connection(&state, &id)?;
        let acquired = if dedicated {
            transfer::acquire_archive(&primary, &state, &flag).await
        } else {
            remote_async::get(&primary, &state).await
        };
        match acquired {
            Ok(remote) => Some(remote),
            Err(error) => {
                let _ = preview::cancel(&operation_id);
                return Err(error);
            }
        }
    } else {
        None
    };
    performance_log!(
        "[TermRill archive] operation={} phase=acquire elapsed_ms={}",
        operation,
        started.elapsed().as_millis()
    );
    let job_remote = remote.clone();
    let job_flag = flag.clone();
    // 解压与格式解析属于 CPU/本地磁盘任务。工作线程等待原生异步协议，
    // 不持有主连接锁；网络请求由 Tokio 驱动并按请求窗口并发。
    let result = blocking(move || {
        let progress_clock = std::cell::RefCell::new(None);
        let io = archive_io::ArchiveIo {
            native: job_remote.as_ref(),
            cancel: &job_flag,
            closed: job_remote.as_ref().map(|remote| remote.closed.as_ref()),
            progress: on_progress.clone(),
            last_progress: &progress_clock,
        };
        let local = archive_io::ArchiveIo {
            native: None,
            cancel: &job_flag,
            closed: io.closed,
            progress: on_progress,
            last_progress: &progress_clock,
        };
        io.check()?;
        archives::run(request, &io, &local, &scope)
    })
    .await;
    if dedicated {
        if let Some(remote) = remote {
            transfer::release_archive(remote, result.is_ok() && !flag.load(Ordering::Relaxed))
                .await;
        }
    }
    performance_log!(
        "[TermRill archive] operation={} phase=total elapsed_ms={} success={}",
        operation,
        started.elapsed().as_millis(),
        result.is_ok()
    );
    let _ = preview::cancel(&operation_id);
    result
}

#[tauri::command]
async fn read_text_chunk(
    id: Option<String>,
    request: String,
    path: String,
    offset: u64,
    stamp: Option<String>,
    state: Shared<'_>,
) -> Result<text_chunks::Chunk> {
    if let Some(id) = id {
        let primary = connection(&state, &id)?;
        let remote = remote_async::get(&primary, &state).await?;
        remote
            .chunk(&request, &path, offset, stamp.as_deref())
            .await
    } else {
        blocking(move || text_chunks::read_local(Path::new(&path), offset, stamp.as_deref())).await
    }
}
#[tauri::command]
async fn close_text_reader(id: String, request: String, state: Shared<'_>) -> Result<()> {
    let primary = connection(&state, &id)?;
    let cache = lock(&primary)?.remote_async.clone();
    let remote = cache.lock().await.clone();
    if let Some(remote) = remote {
        remote.close_reader(&request).await?;
    }
    Ok(())
}
#[tauri::command]
async fn open_local_text(path: String) -> Result<files::Document> {
    blocking(move || local_files::open(Path::new(&path))).await
}
#[tauri::command]
async fn save_local_text(
    path: String,
    content: String,
    revision: String,
) -> Result<files::Document> {
    blocking(move || local_files::save(Path::new(&path), &content, &revision)).await
}
#[tauri::command]
async fn local_properties(path: String) -> Result<local_files::Properties> {
    blocking(move || local_files::properties(Path::new(&path))).await
}
#[tauri::command]
async fn read_local_preview(
    path: String,
    request: String,
    on_progress: tauri::ipc::Channel<u64>,
) -> Result<tauri::ipc::Response> {
    blocking(move || {
        preview::read_local(Path::new(&path), &request, on_progress).map(tauri::ipc::Response::new)
    })
    .await
}

#[tauri::command]
async fn open_remote_text(id: String, path: String, state: Shared<'_>) -> Result<files::Document> {
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    let bytes = remote
        .read(&path, files::EDIT_LIMIT, &AtomicBool::new(false), |_| {})
        .await?;
    blocking(move || files::document(&bytes)).await
}
#[tauri::command]
fn begin_preview() -> Result<String> {
    preview::begin()
}
#[tauri::command]
async fn unpack_epub(
    request: tauri::ipc::Request<'_>,
) -> Result<std::collections::BTreeMap<String, String>> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::new("EPUB 需要二进制请求"));
    };
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(AppError::new("EPUB 超过 32 MiB"));
    }
    let bytes = bytes.clone();
    blocking(move || epub::unpack(&bytes)).await
}
#[tauri::command]
fn cancel_preview(request: String) -> Result<()> {
    preview::cancel(&request)
}
#[tauri::command]
async fn read_remote_preview(
    id: String,
    path: String,
    request: String,
    on_progress: tauri::ipc::Channel<u64>,
    state: Shared<'_>,
) -> Result<tauri::ipc::Response> {
    let result = async {
        let primary = connection(&state, &id)?;
        let flag = preview::flag(&request)?;
        let remote = remote_async::preview_owner(&primary).await?;
        let bytes =
            transfer::read_preview(&primary, &state, remote.as_deref(), &path, &flag, |bytes| {
                let _ = on_progress.send(bytes);
            })
            .await?;
        preview::validate(&bytes)?;
        Ok(bytes)
    }
    .await;
    let _ = preview::cancel(&request);
    result.map(tauri::ipc::Response::new)
}
#[tauri::command]
async fn save_remote_text(
    id: String,
    path: String,
    content: String,
    revision: String,
    state: Shared<'_>,
) -> Result<files::Document> {
    if content.len() > 2 * 1024 * 1024 || content.contains('\0') {
        return Err(AppError::new("内容过大或包含二进制字符"));
    }
    let primary = connection(&state, &id)?;
    let remote = remote_async::get(&primary, &state).await?;
    let cancel = AtomicBool::new(false);
    let original = remote.read(&path, 2 * 1024 * 1024, &cancel, |_| {}).await?;
    let prepared = blocking(move || {
        use sha2::{Digest, Sha256};
        if format!("{:x}", Sha256::digest(&original)) != revision {
            return Err(AppError::new("远程文件已被修改，请重新打开后合并内容"));
        }
        let (_, encoding) = files::decode(&original)?;
        let bytes = files::encode(&content, encoding)?;
        Ok((bytes, revision))
    })
    .await?;
    remote
        .install_bytes(&path, &prepared.0, true, &cancel, Some(&prepared.1))
        .await?;
    blocking(move || files::document(&prepared.0)).await
}
#[tauri::command]
// 参数直接对应稳定的前端 IPC 字段，保留命令边界而不引入第二套请求结构。
#[allow(clippy::too_many_arguments)]
fn start_transfer(
    id: String,
    connection_id: String,
    local: String,
    remote: String,
    direction: String,
    overwrite: bool,
    selection: Option<std::collections::BTreeSet<String>>,
    state: Shared<'_>,
    app: tauri::AppHandle,
) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        || !["upload", "download"].contains(&direction.as_str())
        || !Path::new(&local).is_absolute()
        || !remote.starts_with('/')
    {
        return Err(AppError::new("传输方向或路径无效"));
    }
    if let Some(selected) = &selection {
        let prefix = format!("{}/", remote.trim_end_matches('/'));
        if selected.is_empty()
            || selected.len() > 100_000
            || selected.iter().any(|path| {
                !path.starts_with(&prefix)
                    || path.contains('\\')
                    || path.split('/').any(|part| part == "." || part == "..")
            })
        {
            return Err(AppError::new("同步文件选择无效"));
        }
    }
    if lock(&state.jobs)?.len() >= 128 {
        return Err(AppError::new("待恢复任务过多，请清理失败记录"));
    }
    let conn = connection(&state, &connection_id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut transfers = lock(&state.transfers)?;
        if transfers.len() >= 32 || transfers.contains_key(&id) {
            return Err(AppError::new("传输队列已满或任务标识重复"));
        }
        transfers.insert(id.clone(), cancel.clone());
    }
    lock(&state.pauses)?.insert(id.clone(), Arc::new(AtomicBool::new(false)));
    let job = transfer::Job {
        completed: BTreeMap::new(),
        selection,
        id,
        connection_id,
        local,
        remote,
        direction,
        overwrite,
    };
    let profile = lock(&state.connection_profiles)?
        .get(&job.connection_id)
        .cloned()
        .ok_or_else(|| AppError::new("连接已断开"))?;
    lock(&state.jobs)?.insert(job.id.clone(), (job.clone(), profile));
    if let Err(error) = transfer::persist_jobs(&state) {
        lock(&state.jobs)?.remove(&job.id);
        lock(&state.transfers)?.remove(&job.id);
        lock(&state.pauses)?.remove(&job.id);
        return Err(error);
    }
    let backend = state.inner().clone();
    std::thread::spawn(move || transfer::run(app, backend, job, cancel, conn));
    Ok(())
}
#[tauri::command]
fn pending_transfers(profile: Profile, state: Shared<'_>) -> Result<Vec<TransferEvent>> {
    pending_transfers_for_profile(&state, &profile)
}

fn pending_transfers_for_profile(state: &Backend, profile: &Profile) -> Result<Vec<TransferEvent>> {
    // 页面重载并不停止 Rust 工作线程；只有未运行的持久化任务才属于中断恢复。
    // 先复制状态再释放锁，避免与任务登记/完成流程交叉持锁。
    let active = lock(&state.transfers)?
        .iter()
        .map(|(id, cancel)| (id.clone(), cancel.load(Ordering::Relaxed)))
        .collect::<BTreeMap<_, _>>();
    let paused = lock(&state.pauses)?
        .iter()
        .map(|(id, pause)| (id.clone(), pause.load(Ordering::Relaxed)))
        .collect::<BTreeMap<_, _>>();
    Ok(lock(&state.jobs)?
        .values()
        // 连接 ID 在重连后变化；配置名可重命名。按稳定配置 ID 与实际端点隔离。
        // 同 ID 配置若被编辑为另一服务器，也不能认领此前的历史任务。
        .filter(|(_, stored)| {
            stored.id == profile.id
                && stored.target == profile.target
                && stored.jump == profile.jump
        })
        .map(|(job, _)| TransferEvent {
            id: job.id.clone(),
            connection_id: job.connection_id.clone(),
            name: Path::new(if job.direction == "upload" {
                &job.local
            } else {
                &job.remote
            })
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
            direction: job.direction.clone(),
            status: match active.get(&job.id) {
                Some(true) => "cancelling",
                Some(false) if paused.get(&job.id) == Some(&true) => "paused",
                Some(false) => "running",
                None => "failed",
            }
            .into(),
            bytes: 0,
            total: 0,
            speed: 0,
            remaining_seconds: None,
            error: if active.contains_key(&job.id) {
                None
            } else {
                Some("任务中断，请连接原服务器后重试；已有临时内容会先校验再续传".into())
            },
        })
        .collect())
}
#[tauri::command]
async fn discard_transfer(
    id: String,
    connection_id: Option<String>,
    state: Shared<'_>,
) -> Result<()> {
    let backend = state.inner().clone();
    discard_transfer_async(&id, connection_id.as_deref(), &backend).await
}

async fn discard_transfer_async(
    id: &str,
    connection_id: Option<&str>,
    backend: &Backend,
) -> Result<()> {
    let saved = lock(&backend.jobs)?
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::new("任务记录不存在"))?;
    {
        let mut transfers = lock(&backend.transfers)?;
        if transfers.contains_key(id) {
            return Err(AppError::new("运行中的任务应先取消，或等待清理完成"));
        }
        // 清理期间占用任务名额，避免重试与删除临时文件并发执行。
        transfers.insert(id.into(), Arc::new(AtomicBool::new(false)));
    }
    let result = async {
        let (job, profile) = &saved;
        if job.direction == "upload" {
            let conn = connection(backend, connection_id.unwrap_or(&job.connection_id))
                .map_err(|_| AppError::new("请先连接原服务器，再移除记录；临时文件会自动清理"))?;
            let current = {
                let conn = lock(&conn)?;
                conn.request.profile.clone()
            };
            if serde_json::to_vec(&(&current.target, &current.jump))?
                != serde_json::to_vec(&(&profile.target, &profile.jump))?
            {
                return Err(AppError::new(
                    "当前连接与原任务服务器不匹配，未清理临时文件",
                ));
            }
            let remote = remote_async::get(&conn, backend).await?;
            transfer_cleanup::cleanup_async(job, &remote).await?;
        } else {
            blocking({
                let job = job.clone();
                move || transfer_cleanup::cleanup(&job)
            })
            .await?;
        }
        lock(&backend.jobs)?.remove(id);
        if let Err(error) = transfer::persist_jobs(backend) {
            lock(&backend.jobs)?.insert(id.into(), saved.clone());
            return Err(error);
        }
        Ok(())
    }
    .await;
    lock(&backend.transfers)?.remove(id);
    result
}
#[tauri::command]
fn retry_transfer(
    id: String,
    connection_id: String,
    state: Shared<'_>,
    app: tauri::AppHandle,
) -> Result<()> {
    let (mut job, original_profile) = lock(&state.jobs)?
        .get(&id)
        .cloned()
        .ok_or_else(|| AppError::new("没有可重试的任务"))?;
    let current = lock(&state.connection_profiles)?
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| AppError::new("请先重新连接原来的服务器"))?;
    if serde_json::to_vec(&(&current.target, &current.jump))?
        != serde_json::to_vec(&(&original_profile.target, &original_profile.jump))?
    {
        return Err(AppError::new("当前连接与原任务的服务器或认证配置不匹配"));
    }
    job.connection_id = connection_id;
    let conn = connection(&state, &job.connection_id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut transfers = lock(&state.transfers)?;
        if transfers.contains_key(&id) || transfers.len() >= 32 {
            return Err(AppError::new("任务仍在运行或队列已满"));
        }
        transfers.insert(id.clone(), cancel.clone());
    }
    lock(&state.pauses)?.insert(id.clone(), Arc::new(AtomicBool::new(false)));
    // 重连得到新连接 ID；先持久化，确保再次中断后恢复的是本次任务配置。
    let previous = lock(&state.jobs)?.insert(id.clone(), (job.clone(), original_profile));
    if let Err(error) = transfer::persist_jobs(&state) {
        if let Some(previous) = previous {
            lock(&state.jobs)?.insert(id.clone(), previous);
        }
        lock(&state.pauses)?.remove(&id);
        lock(&state.transfers)?.remove(&id);
        return Err(error);
    }
    let backend = state.inner().clone();
    std::thread::spawn(move || transfer::run(app, backend, job, cancel, conn));
    Ok(())
}
#[tauri::command]
fn pause_transfer(id: String, paused: bool, state: Shared<'_>) -> Result<()> {
    let pauses = lock(&state.pauses)?;
    let flag = pauses
        .get(&id)
        .ok_or_else(|| AppError::new("传输任务已结束"))?;
    flag.store(paused, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
fn cancel_transfer(id: String, state: Shared<'_>) -> Result<()> {
    if let Some(cancel) = lock(&state.transfers)?.get(&id) {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}
#[tauri::command]
async fn start_terminal(
    id: String,
    connection_id: String,
    cols: u32,
    rows: u32,
    state: Shared<'_>,
    app: tauri::AppHandle,
) -> Result<()> {
    let backend = state.inner().clone();
    let conn = connection(&backend, &connection_id)?;
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    let stop = Arc::new(AtomicBool::new(false));
    let acknowledged = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let accepted = {
        let mut terminals = lock(&backend.terminals)?;
        if terminals.contains_key(&id) || terminals.len() >= 8 {
            false
        } else {
            terminals.insert(
                id.clone(),
                terminal::TerminalControl {
                    connection_id,
                    input: sender,
                    stop: stop.clone(),
                    acknowledged: acknowledged.clone(),
                },
            );
            true
        }
    };
    if !accepted {
        return Err(AppError::new("终端已存在或超过数量限制"));
    }
    // 握手前占用名额，关闭请求也能取消尚未完成的启动。
    let setup = async {
        let remote = remote_async::get(&conn, &backend).await?;
        let (channel, initial) = remote
            .checked(
                remote
                    .native
                    .terminal_channel(cols.clamp(20, 500), rows.clamp(5, 300)),
                &stop,
            )
            .await?;
        Ok::<_, AppError>((remote, channel, initial))
    }
    .await;
    let (remote, channel, initial) = match setup {
        Ok(value) => value,
        Err(error) => {
            lock(&backend.terminals)?.remove(&id);
            return Err(error);
        }
    };
    tauri::async_runtime::spawn(terminal::run(
        app,
        backend,
        id,
        channel,
        initial,
        receiver,
        stop,
        remote.closed.clone(),
        acknowledged,
    ));
    Ok(())
}
#[tauri::command]
fn terminal_ack(id: String, bytes: usize, state: Shared<'_>) -> Result<()> {
    if bytes > 1024 * 1024 {
        return Err(AppError::new("终端确认字节数超限"));
    }
    if let Some(control) = lock(&state.terminals)?.get(&id) {
        control.acknowledged.fetch_add(bytes, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
async fn read_log(
    id: Option<String>,
    path: String,
    cursor: Option<log_follow::Cursor>,
    state: Shared<'_>,
) -> Result<log_follow::Batch> {
    if let Some(id) = id {
        let primary = connection(&state, &id)?;
        let remote = remote_async::get(&primary, &state).await?;
        log_follow::read_remote(&remote, &path, cursor.as_ref()).await
    } else {
        blocking(move || log_follow::read_local(Path::new(&path), cursor.as_ref())).await
    }
}

#[tauri::command]
fn terminal_input(id: String, data: String, state: Shared<'_>) -> Result<()> {
    if data.len() > 65536 {
        return Err(AppError::new("单次终端输入过大"));
    }
    let terminals = lock(&state.terminals)?;
    let control = terminals
        .get(&id)
        .ok_or_else(|| AppError::new("终端已关闭"))?;
    control
        .input
        .try_send(terminal::Input::Data(data))
        .map_err(|_| AppError::new("终端输入繁忙，请稍后重试"))
}
#[tauri::command]
fn resize_terminal(id: String, cols: u32, rows: u32, state: Shared<'_>) -> Result<()> {
    if let Some(control) = lock(&state.terminals)?.get(&id) {
        let _ = control.input.try_send(terminal::Input::Resize(
            cols.clamp(20, 500),
            rows.clamp(5, 300),
        ));
    }
    Ok(())
}
#[tauri::command]
fn close_terminal(id: String, state: Shared<'_>) -> Result<()> {
    if let Some(control) = lock(&state.terminals)?.get(&id) {
        control.stop.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(StartupClock(std::time::Instant::now()))
        .setup(|app| {
            if let Ok(path) = app.path().app_cache_dir() {
                archives::initialize_cache(path.join("zip-ranges"));
            }
            if performance::initialize(app.path().app_log_dir()?).is_err() {
                eprintln!("[TermRill performance] log_initialization_failed=true");
            }
            let dir = app.path().app_config_dir()?;
            fs::create_dir_all(&dir)?;
            let config_path = dir.join("connections.json");
            let config: Config = if config_path.exists() {
                serde_json::from_slice(&fs::read(&config_path)?)?
            } else {
                Config::default()
            };
            config
                .transfer_settings
                .validate()
                .map_err(|error| std::io::Error::other(error.message))?;
            app.manage(Arc::new(Backend {
                auth_app: Some(app.handle().clone()),
                auth_answers: Mutex::new(BTreeMap::new()),
                config_path: config_path.clone(),
                config: Mutex::new(config),
                connections: Mutex::new(BTreeMap::new()),
                closed: Mutex::new(BTreeMap::new()),
                transfers: Mutex::new(BTreeMap::new()),
                pauses: Mutex::new(BTreeMap::new()),
                jobs: Mutex::new({
                    let path = config_path.with_file_name("transfers.json");
                    if path.exists() {
                        let jobs: BTreeMap<String, (transfer::Job, Profile)> =
                            transfer_journal::load(&path)
                                .map_err(|error| std::io::Error::other(error.message))?;
                        if jobs.len() > 128
                            || jobs.iter().any(|(id, (job, _))| {
                                id != &job.id
                                    || id.is_empty()
                                    || id.len() > 64
                                    || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                                    || !Path::new(&job.local).is_absolute()
                                    || !job.remote.starts_with('/')
                                    || !["upload", "download"].contains(&job.direction.as_str())
                            })
                        {
                            return Err("传输任务清单无效".into());
                        }
                        jobs
                    } else {
                        BTreeMap::new()
                    }
                }),
                connection_profiles: Mutex::new(BTreeMap::new()),
                terminals: Mutex::new(BTreeMap::new()),
                pending_hosts: Mutex::new(BTreeMap::new()),
            }));
            if cfg!(debug_assertions) {
                eprintln!(
                    "[TermRill startup] backend ready: {} ms",
                    app.state::<StartupClock>().0.elapsed().as_millis()
                );
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.try_state::<Arc<Backend>>() {
                    if let Ok(mut answers) = state.auth_answers.lock() {
                        answers.clear();
                    }
                    if let Ok(closed) = state.closed.lock() {
                        for flag in closed.values() {
                            flag.store(true, Ordering::Relaxed);
                        }
                    }
                    // 退出使用连接关闭信号，保留任务清单和临时文件供重启续传。
                    if let Ok(terminals) = state.terminals.lock() {
                        for terminal in terminals.values() {
                            terminal.stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            delete_timing,
            remote_properties,
            set_remote_permissions,
            set_remote_permissions_batch,
            set_remote_owner,
            set_remote_owner_batch,
            credential_support,
            answer_auth,
            save_credentials,
            load_credentials,
            delete_credentials,
            delete_entry,
            archive_operation,
            release_archive_snapshot,
            open_local_text,
            read_text_chunk,
            close_text_reader,
            save_local_text,
            local_properties,
            read_local_preview,
            open_remote_text,
            begin_preview,
            unpack_epub,
            cancel_preview,
            read_remote_preview,
            save_remote_text,
            app_version,
            startup_ready,
            load_preferences,
            compare_directories,
            sync_rules,
            save_preferences,
            load_transfer_settings,
            save_transfer_settings,
            load_profiles,
            workbench_preferences,
            workbench_command,
            server_metrics,
            save_profiles,
            trust_host,
            known_hosts,
            forget_host,
            connect,
            disconnect,
            list_remote,
            list_remote_page,
            list_local_page,
            list_local_drives,
            list_local,
            pick_path,
            create_directory,
            rename_entry,
            start_transfer,
            cancel_transfer,
            pause_transfer,
            retry_transfer,
            pending_transfers,
            discard_transfer,
            start_terminal,
            terminal_input,
            terminal_ack,
            read_log,
            resize_terminal,
            close_terminal
        ])
        .run(tauri::generate_context!())
        .expect("failed to run TermRill");
}

#[cfg(test)]
fn discard_transfer_inner(id: &str, connection_id: Option<&str>, backend: &Backend) -> Result<()> {
    tauri::async_runtime::block_on(discard_transfer_async(id, connection_id, backend))
}
