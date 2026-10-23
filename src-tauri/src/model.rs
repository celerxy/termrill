//! 跨模块状态与 IPC 数据模型；序列化字段使用 camelCase，与前端 types.ts 保持一致。

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

pub type Result<T> = std::result::Result<T, AppError>;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_key: Option<HostKey>,
    /// 仅内部恢复使用，不通过错误字符串推断网络故障。
    #[serde(skip)]
    pub timeout: bool,
}
impl AppError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            host_key: None,
            timeout: false,
        }
    }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        let mut error = Self::new(e.to_string());
        error.timeout = e.kind() == std::io::ErrorKind::TimedOut;
        error
    }
}
#[cfg(test)]
impl From<ssh2::Error> for AppError {
    fn from(e: ssh2::Error) -> Self {
        // EAGAIN 应在持有请求状态的调用点续接。这里已经失去请求上下文，
        // 不能统一重放；仅记录代码调用栈定位遗漏入口，不记录错误正文或凭据。
        if e.code() == ssh2::ErrorCode::Session(-37) {
            crate::performance_log!(
                "[TermRill SSH] phase=unhandled_eagain stack={}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        let mut error = Self::new(e.to_string());
        error.timeout = e.code() == ssh2::ErrorCode::Session(-9);
        error
    }
}
impl From<russh::Error> for AppError {
    fn from(error: russh::Error) -> Self {
        // 会话因无响应结束时库只给出英文变体名；改为可直接展示的原因。
        // 不设置 timeout 标记，避免触发面向请求超时的内部恢复。
        match error {
            russh::Error::KeepaliveTimeout => {
                Self::new("服务器长时间无响应，连接已断开（keepalive 超时）")
            }
            russh::Error::InactivityTimeout => Self::new("连接长时间无活动，已断开"),
            error => Self::new(error.to_string()),
        }
    }
}
impl From<russh_sftp::client::error::Error> for AppError {
    fn from(error: russh_sftp::client::error::Error) -> Self {
        let timeout = matches!(error, russh_sftp::client::error::Error::Timeout);
        Self {
            message: error.to_string(),
            host_key: None,
            timeout,
        }
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(e.to_string())
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: String,
    pub key_path: String,
}
impl Endpoint {
    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() || self.username.trim().is_empty() || self.port == 0 {
            return Err(AppError::new("请填写有效的主机地址、端口和用户名"));
        }
        if !["password", "key", "agent", "interactive"].contains(&self.auth.as_str()) {
            return Err(AppError::new("不支持的认证方式"));
        }
        if self.auth == "key" && self.key_path.is_empty() {
            return Err(AppError::new("请选择私钥文件"));
        }
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// 仅供当前会话认证；Config/Profile 不包含此类型，避免凭据进入配置文件。
pub struct Secrets {
    pub password: String,
    pub passphrase: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub target: Endpoint,
    pub jump: Option<Endpoint>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    pub profile: Profile,
    pub target_secrets: Secrets,
    pub jump_secrets: Secrets,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Debug)]
pub struct HostKey {
    pub address: String,
    pub fingerprint: String,
    pub previous: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub profiles: Vec<Profile>,
    pub known_hosts: BTreeMap<String, String>,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default)]
    pub transfer_settings: TransferSettings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSettings {
    pub concurrency: usize,
    /// 单个任务的合计字节/秒；0 表示不限速，并发线程共享预算。
    pub bytes_per_second: u64,
}
impl Default for TransferSettings {
    fn default() -> Self {
        Self {
            concurrency: 3,
            bytes_per_second: 0,
        }
    }
}
impl TransferSettings {
    pub fn validate(&self) -> Result<()> {
        if !(1..=4).contains(&self.concurrency) || self.bytes_per_second > 1024 * 1024 * 1024 {
            return Err(AppError::new("并发需为 1～4，限速不得超过 1 GiB/s"));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub directory: bool,
    pub symlink: bool,
    pub size: u64,
    pub modified: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FileEntry>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferEvent {
    pub id: String,
    pub connection_id: String,
    pub name: String,
    pub direction: String,
    pub status: String,
    pub bytes: u64,
    pub total: u64,
    pub speed: u64,
    /// 上传综合文件安装完成率与字节速度估算；未取得有效样本时为空。
    pub remaining_seconds: Option<u64>,
    pub error: Option<String>,
}

/// 注册表锁只用于取出句柄；网络操作通过连接自己的 Mutex 串行执行。
pub struct Backend {
    pub auth_app: Option<tauri::AppHandle>,
    pub auth_answers: Mutex<BTreeMap<String, std::sync::mpsc::SyncSender<Option<Vec<String>>>>>,
    pub config_path: PathBuf,
    pub config: Mutex<Config>,
    pub connections: Mutex<BTreeMap<String, Arc<Mutex<crate::ssh::Connection>>>>,
    pub closed: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    pub connection_profiles: Mutex<BTreeMap<String, Profile>>,
    pub jobs: Mutex<BTreeMap<String, (crate::transfer::Job, Profile)>>,
    pub pauses: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    pub transfers: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    pub terminals: Mutex<BTreeMap<String, crate::terminal::TerminalControl>>,
    pub pending_hosts: Mutex<BTreeMap<String, HostKey>>,
}
pub fn lock<T>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    m.lock()
        .map_err(|_| AppError::new("内部状态锁失效，请重新启动应用"))
}
pub fn identifier() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
pub fn sorted(mut entries: Vec<FileEntry>) -> Vec<FileEntry> {
    // 每个名称只生成一次排序键，避免大目录每次比较都分配小写字符串。
    entries.sort_by_cached_key(|entry| (!entry.directory, entry.name.to_lowercase()));
    entries
}
// Windows reparse point 也可能是目录联接；仅检查 symlink 会遗漏它。
pub fn local_link(stat: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        stat.file_type().is_symlink() || stat.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        stat.file_type().is_symlink()
    }
}

/// 协议无关的元数据快照；生产构建无需 libssh2。
#[cfg(not(test))]
#[derive(Clone)]
pub struct FileStat {
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub perm: Option<u32>,
    pub mtime: Option<u64>,
}
#[cfg(not(test))]
impl FileStat {
    pub fn is_file(&self) -> bool {
        self.perm.is_some_and(|mode| mode & 0o170000 == 0o100000)
    }
    pub fn is_dir(&self) -> bool {
        self.perm.is_some_and(|mode| mode & 0o170000 == 0o040000)
    }
}
#[cfg(test)]
pub type FileStat = ssh2::FileStat;
