//! 注册项只保存认证请求、信任快照和原生异步会话，不拥有同步 SSH 传输。
use crate::model::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
pub struct Connection {
    pub remote_async: crate::remote_async::Cache,
    pub request: ConnectRequest,
    pub closed: Arc<AtomicBool>,
    pub first_listing: Arc<tokio::sync::Notify>,
    pub host_algorithms: std::collections::BTreeMap<String, String>,
    pub trusted: std::collections::BTreeMap<String, String>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}
pub async fn connect_async(request: ConnectRequest, backend: &Backend) -> Result<Connection> {
    request.profile.target.validate()?;
    if let Some(jump) = &request.profile.jump {
        jump.validate()?;
    }
    let trusted = lock(&backend.config)?.known_hosts.clone();
    let closed = Arc::new(AtomicBool::new(false));
    let algorithms = Default::default();
    let native = crate::transfer::native_async::ssh::Connection::connect(
        &request,
        &trusted,
        &algorithms,
        Some(backend),
    )
    .await?;
    let remote = crate::remote_async::Remote::new(native, closed.clone());
    Ok(Connection {
        remote_async: Arc::new(tokio::sync::Mutex::new(Some(remote))),
        request,
        closed,
        first_listing: Arc::new(tokio::sync::Notify::new()),
        host_algorithms: algorithms,
        trusted,
    })
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingBatch {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FileEntry>,
    pub count: usize,
    pub reset: bool,
}
