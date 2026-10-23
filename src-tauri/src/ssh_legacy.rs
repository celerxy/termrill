//! SSH 握手、指纹确认、认证及标准 direct-tcpip 跳板。

use crate::model::*;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use ssh2::{Channel, KeyboardInteractivePrompt, MethodType, Prompt, Session};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub struct Tunnel {
    stop: Arc<AtomicBool>,
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
/// 持有 SSH 会话和跳板生命周期；移除注册项后，正在执行的任务仍可能持有 Arc。
pub struct Connection {
    pub remote_async: crate::remote_async::Cache,
    pub session: Session,
    /// 保存实际验证过的主机密钥算法，异步上传使用同一算法以复用已确认指纹。
    pub host_algorithms: std::collections::BTreeMap<String, String>,
    directory_sftp: Option<Arc<ssh2::Sftp>>,
    file_sftp: Option<Arc<ssh2::Sftp>>,
    pub request: ConnectRequest,
    pub closed: Arc<AtomicBool>,
    pub first_listing: Arc<tokio::sync::Notify>,
    pub _tunnel: Option<Tunnel>,
}
impl Connection {
    /// 浏览通道随连接缓存；不同目录句柄共享该子系统，断开时随最后一个持有者释放。
    pub fn directory_sftp(&mut self) -> Result<Arc<ssh2::Sftp>> {
        self.read_subsystem(false)
    }
    fn read_subsystem(&mut self, file: bool) -> Result<Arc<ssh2::Sftp>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(AppError::new("连接已关闭"));
        }
        let cached = if file {
            &mut self.file_sftp
        } else {
            &mut self.directory_sftp
        };
        if let Some(sftp) = cached {
            return Ok(sftp.clone());
        }
        // libssh2 在 Session 内保留 sftp_init 状态；EAGAIN 后继续同一次初始化，
        // 不能切换回阻塞等待或当作失败反复申请通道。调用者持连接锁保护模式。
        let started = std::time::Instant::now();
        let _mode = crate::transfer_io::Nonblocking::new(&self.session);
        let result =
            crate::files::network_step(Some((&self.session, &self.closed)), "sftp_init", || {
                self.session.sftp().map_err(Into::into)
            });
        crate::performance_log!(
            "[TermRill SFTP] phase=init file={} elapsed_ms={} success={}",
            file,
            started.elapsed().as_millis(),
            result.is_ok()
        );
        let sftp = Arc::new(result?);
        *cached = Some(sftp.clone());
        Ok(sftp)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

fn socket(endpoint: &Endpoint) -> Result<TcpStream> {
    let addresses = (endpoint.host.as_str(), endpoint.port).to_socket_addrs()?;
    for address in addresses {
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(12)) {
            stream.set_nodelay(true)?;
            return Ok(stream);
        }
    }
    Err(AppError::new(format!(
        "无法连接 {}:{}，请检查地址、网络或端口",
        endpoint.host, endpoint.port
    )))
}
fn handshake(stream: TcpStream, endpoint: &Endpoint, role: &str) -> Result<Session> {
    let mut session = Session::new()?;
    session.set_timeout(15_000);
    session.set_tcp_stream(stream);
    if let Err(error) = session.handshake() {
        // 握手尚未发送认证凭据；不能把算法协商错误误报为密码错误。
        // 只列出本客户端实际编译支持的算法，不猜测服务器提供了哪些算法。
        let detail = if error.code() == ssh2::ErrorCode::Session(-5) {
            let algorithms = |kind| {
                session
                    .supported_algs(kind)
                    .map(|names| names.join(", "))
                    .unwrap_or_else(|_| "无法读取".into())
            };
            format!(
                "密钥交换失败，尚未进行密码认证。可能是双方算法不兼容或服务器中断握手。\n客户端密钥交换算法：{}\n客户端主机密钥算法：{}\n请使用系统 ssh -vvv 检查服务器协商信息。",
                algorithms(MethodType::Kex),
                algorithms(MethodType::HostKey),
            )
        } else {
            "SSH 握手失败，尚未进行密码认证。请检查服务器 SSH 服务与网络连接。".into()
        };
        return Err(AppError::new(format!(
            "{role} {}:{}：{detail}\n原始错误：{error}",
            endpoint.host, endpoint.port
        )));
    }
    Ok(session)
}
// 必须在发送密码或签名之前调用；未知或变化的指纹作为挑战交给界面确认。
fn verify(session: &Session, endpoint: &Endpoint, backend: &Backend) -> Result<()> {
    let (key, _) = session
        .host_key()
        .ok_or_else(|| AppError::new("服务器未提供主机密钥"))?;
    let fingerprint = format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(key)));
    let address = format!("[{}]:{}", endpoint.host, endpoint.port);
    let previous = lock(&backend.config)?.known_hosts.get(&address).cloned();
    if previous.as_ref() == Some(&fingerprint) {
        return Ok(());
    }
    let challenge = HostKey {
        address: address.clone(),
        fingerprint,
        previous,
    };
    lock(&backend.pending_hosts)?.insert(address, challenge.clone());
    Err(AppError {
        message: if challenge.previous.is_some() {
            "服务器主机指纹已变化，请核实后确认".into()
        } else {
            "首次连接，请确认服务器主机指纹".into()
        },
        host_key: Some(challenge),
        timeout: false,
    })
}
/// 仅自动回答单个隐藏的密码提示；OTP、多提示及再次挑战需要独立交互界面。
struct PasswordPrompt<'a> {
    password: &'a str,
    answered: bool,
    unsupported: bool,
}
impl KeyboardInteractivePrompt for PasswordPrompt<'_> {
    fn prompt(&mut self, _: &str, _: &str, prompts: &[Prompt<'_>]) -> Vec<String> {
        if prompts.is_empty() {
            return Vec::new();
        }
        let password_prompt = prompts.len() == 1
            && !prompts[0].echo
            && (prompts[0].text.to_lowercase().contains("password")
                || prompts[0].text.contains("密码"));
        if self.answered || !password_prompt {
            self.unsupported = true;
            return vec![String::new(); prompts.len()];
        }
        self.answered = true;
        vec![self.password.to_owned()]
    }
}

fn authenticate(
    session: &Session,
    endpoint: &Endpoint,
    secrets: &Secrets,
    backend: &Backend,
) -> Result<()> {
    match endpoint.auth.as_str() {
        "password" => {
            let methods = session.auth_methods(&endpoint.username)?.to_owned();
            if session.authenticated() {
                return Ok(());
            }
            // 普通密码被拒绝时不自动再次尝试，避免重复消耗服务器登录次数。
            if methods.split(',').any(|method| method == "password") {
                session.userauth_password(&endpoint.username, &secrets.password)?;
            } else if methods
                .split(',')
                .any(|method| method == "keyboard-interactive")
            {
                let mut responder = PasswordPrompt {
                    password: &secrets.password,
                    answered: false,
                    unsupported: false,
                };
                let result =
                    session.userauth_keyboard_interactive(&endpoint.username, &mut responder);
                if responder.unsupported {
                    return Err(AppError::new(
                        "服务器要求验证码、多步骤或非密码交互认证，当前尚未支持",
                    ));
                }
                result?;
            } else {
                return Err(AppError::new(
                    "服务器未提供密码或交互式密码认证，请选择私钥或 SSH Agent",
                ));
            }
        }
        "key" => session.userauth_pubkey_file(
            &endpoint.username,
            None,
            std::path::Path::new(&endpoint.key_path),
            if secrets.passphrase.is_empty() {
                None
            } else {
                Some(&secrets.passphrase)
            },
        )?,
        "interactive" => {
            let mut responder = crate::auth::Interactive {
                backend,
                endpoint,
                error: None,
                rounds: 0,
            };
            let result = session.userauth_keyboard_interactive(&endpoint.username, &mut responder);
            if let Some(error) = responder.error {
                return Err(AppError::new(error));
            }
            result?;
        }
        "agent" => session.userauth_agent(&endpoint.username)?,
        _ => return Err(AppError::new("不支持的认证方式")),
    }
    if !session.authenticated() {
        return Err(AppError::new("SSH 认证失败"));
    }
    Ok(())
}

pub fn connect(request: ConnectRequest, backend: &Backend) -> Result<Connection> {
    request.profile.target.validate()?;
    let mut host_algorithms = std::collections::BTreeMap::new();
    let (stream, tunnel) = if let Some(jump) = &request.profile.jump {
        jump.validate()?;
        let session = handshake(socket(jump)?, jump, "堡垒机")?;
        verify(&session, jump, backend)?;
        if let Some(algorithm) = session.methods(MethodType::HostKey) {
            host_algorithms.insert(format!("[{}]:{}", jump.host, jump.port), algorithm.into());
        }
        authenticate(&session, jump, &request.jump_secrets, backend)?;
        let channel = session.channel_direct_tcpip(
            &request.profile.target.host,
            request.profile.target.port,
            None,
        )?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let client = TcpStream::connect(listener.local_addr()?)?;
        let (server, _) = listener.accept()?;
        // SSH/SFTP 小请求需要立即转发，避免回环 TCP 的 Nagle/延迟 ACK 叠加。
        client.set_nodelay(true)?;
        server.set_nodelay(true)?;
        drop(listener);
        server.set_nonblocking(true)?;
        session.set_blocking(false);
        let stop = Arc::new(AtomicBool::new(false));
        let control = stop.clone();
        std::thread::spawn(move || relay(server, channel, session, control));
        (client, Some(Tunnel { stop }))
    } else {
        (socket(&request.profile.target)?, None)
    };
    let session = handshake(stream, &request.profile.target, "目标主机")?;
    verify(&session, &request.profile.target, backend)?;
    if let Some(algorithm) = session.methods(MethodType::HostKey) {
        host_algorithms.insert(
            format!(
                "[{}]:{}",
                request.profile.target.host, request.profile.target.port
            ),
            algorithm.into(),
        );
    }
    authenticate(
        &session,
        &request.profile.target,
        &request.target_secrets,
        backend,
    )?;
    // 握手/认证保留 15 秒；已认证会话的阻塞操作允许 60 秒，容纳服务器写盘与网络抖动。
    session.set_timeout(60_000);
    session.set_keepalive(true, 20);
    Ok(Connection {
        remote_async: Default::default(),
        session,
        host_algorithms,
        directory_sftp: None,
        file_sftp: None,
        request,
        closed: Arc::new(AtomicBool::new(false)),
        first_listing: Arc::new(tokio::sync::Notify::new()),
        _tunnel: tunnel,
    })
}

// 非阻塞转发需要保留未写完的字节；EAGAIN 不能当作断开，也不能丢弃缓冲区。
fn relay(mut socket: TcpStream, mut channel: Channel, _session: Session, stop: Arc<AtomicBool>) {
    let mut to_remote = VecDeque::new();
    let mut to_local = VecDeque::new();
    let mut buffer = [0u8; 32768];
    let mut local_eof = false;
    let mut remote_eof = false;
    let mut sent_eof = false;
    while !stop.load(Ordering::Relaxed) {
        if !local_eof && to_remote.len() < 262144 {
            match socket.read(&mut buffer) {
                Ok(0) => local_eof = true,
                Ok(n) => to_remote.extend(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if !to_remote.is_empty() {
            match channel.write(to_remote.make_contiguous()) {
                Ok(n) => {
                    to_remote.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if local_eof && to_remote.is_empty() && !sent_eof && channel.send_eof().is_ok() {
            sent_eof = true;
        }
        if !remote_eof && to_local.len() < 262144 {
            match channel.read(&mut buffer) {
                Ok(0) if channel.eof() => remote_eof = true,
                Ok(n) => to_local.extend(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if !to_local.is_empty() {
            match socket.write(to_local.make_contiguous()) {
                Ok(n) => {
                    to_local.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if remote_eof && to_local.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = socket.shutdown(std::net::Shutdown::Both);
}

#[cfg(test)]
pub fn listing(session: &Session, path: &str) -> Result<Listing> {
    listing_progress(&session.sftp()?, path, &AtomicBool::new(false), |_| {})
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

pub fn listing_progress(
    sftp: &ssh2::Sftp,
    path: &str,
    closed: &AtomicBool,
    progress: impl Fn(ListingBatch),
) -> Result<Listing> {
    let canonical = sftp.realpath(std::path::Path::new(path))?;
    let path = canonical.to_string_lossy().replace('\\', "/");
    let parent = canonical
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"));
    // 逐项读取避免 readdir 的中间完整数组；定期回报数量，断开后停止扫描。
    let mut directory = sftp.opendir(&canonical)?;
    let mut entries = Vec::new();
    let mut reported = std::time::Instant::now();
    let mut delivered = 0;
    loop {
        if closed.load(Ordering::Relaxed) {
            return Err(AppError::new("连接已关闭，目录读取已停止"));
        }
        let (filename, stat) = match directory.readdir() {
            Ok(entry) => entry,
            // 与 ssh2::Sftp::readdir 一致：LIBSSH2_ERROR_FILE 表示目录枚举结束。
            Err(error) if error.code() == ssh2::ErrorCode::Session(-16) => break,
            Err(error) => return Err(error.into()),
        };
        if let Some(name) = filename.file_name() {
            let name = name.to_string_lossy().into_owned();
            if name == "." || name == ".." {
                continue;
            }
            let p = canonical.join(filename);
            entries.push(FileEntry {
                name,
                path: p.to_string_lossy().replace('\\', "/"),
                directory: stat.is_dir(),
                symlink: stat.perm.map(|p| p & 0o170000 == 0o120000).unwrap_or(false),
                size: stat.size.unwrap_or(0),
                modified: stat.mtime,
            });
        }
        // 小目录仅走最终响应；至少读到 256 项后才启用流式消息，避免多次清空和重绘。
        if entries.len() >= 256
            && entries.len() > delivered
            && (entries.len() - delivered >= 2048
                || reported.elapsed() >= std::time::Duration::from_millis(200))
        {
            progress(ListingBatch {
                path: path.clone(),
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
        progress(ListingBatch {
            path: path.clone(),
            parent: parent.clone(),
            entries: entries[delivered..].to_vec(),
            count: entries.len(),
            reset: false,
        });
    }
    Ok(Listing {
        path,
        parent,
        entries: sorted(entries),
    })
}

/// 测试用同步 fixture 会话只负责既有基线；命令实际访问原生异步缓存。
pub async fn connect_async(request: ConnectRequest, backend: &Backend) -> Result<Connection> {
    let primary = std::sync::Mutex::new(connect(request, backend)?);
    let _ = crate::remote_async::get(&primary, backend).await?;
    primary
        .into_inner()
        .map_err(|_| AppError::new("连接锁已损坏"))
}
