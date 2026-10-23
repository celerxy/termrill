//! 原生异步上传连接；目标与跳板独立校验指纹，认证前拒绝未知或变化的主机。
use super::*;
// 上传独立编译底层适配器，保持下载模块及其可见性不变。
#[allow(clippy::duplicate_mod)]
#[path = "sftp_receive_trace.rs"]
mod receive_trace;
// 上传独立编译底层适配器，保持下载模块及其可见性不变。
#[allow(clippy::duplicate_mod)]
#[path = "transfer_tcp.rs"]
mod tcp;
#[path = "transfer_upload_transport.rs"]
mod upload_transport;
use russh::{
    client,
    keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};

struct Verify {
    address: String,
    expected: Option<String>,
    rejected: Arc<std::sync::Mutex<Option<HostKey>>>,
}
impl client::Handler for Verify {
    type Error = AppError;
    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> Result<()> {
        // 只记录协商算法，不能记录回调中的共享密钥或认证信息。
        crate::performance_log!(
            "[TermRill connection] cipher={:?} compression_receive={:?} compression_send={:?}",
            names.cipher,
            names.server_compression,
            names.client_compression
        );
        Ok(())
    }
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                key.fingerprint(HashAlg::Sha256).to_string()
            }
            PublicKeyOrCertificate::Certificate(certificate) => {
                use base64::Engine;
                let bytes = certificate
                    .to_bytes()
                    .map_err(|_| AppError::new("无法编码主机证书"))?;
                format!(
                    "SHA256:{}",
                    base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(bytes))
                )
            }
        };
        if self.expected.as_ref() == Some(&fingerprint) {
            return Ok(true);
        }
        *lock(&self.rejected)? = Some(HostKey {
            address: self.address.clone(),
            fingerprint,
            previous: self.expected.clone(),
        });
        Ok(false)
    }
}

pub(crate) struct Connection {
    pub(crate) upload_uncompressed: bool,
    pub sftp: RawSftpSession,
    pub posix: bool,
    pub write_size: usize,
    pub download_read_size: usize,
    pub max_files: usize,
    // 只随该已认证上传连接复用；新连接重新探测，不写入用户配置。
    pub upload_window: std::sync::atomic::AtomicUsize,
    // 调度提示只在此连接内短期复用；不是服务器的带宽承诺。
    pub upload_rate: std::sync::Mutex<Option<(std::time::Instant, f64)>>,
    pub sent_transport: Arc<std::sync::atomic::AtomicU64>,
    pub received_transport: Arc<std::sync::atomic::AtomicU64>,
    pub received_data: Arc<std::sync::atomic::AtomicU64>,
    target: client::Handle<Verify>,
    _jump: Option<client::Handle<Verify>>,
}

fn verifier(endpoint: &Endpoint, trusted: &std::collections::BTreeMap<String, String>) -> Verify {
    let address = format!("[{}]:{}", endpoint.host, endpoint.port);
    Verify {
        expected: trusted.get(&address).cloned(),
        address,
        rejected: Arc::new(std::sync::Mutex::new(None)),
    }
}

fn connection_config(
    endpoint: &Endpoint,
    algorithms: &std::collections::BTreeMap<String, String>,
) -> Result<Arc<client::Config>> {
    let mut config = client::Config {
        nodelay: true,
        keepalive_interval: Some(Duration::from_secs(15)),
        window_size: 8 * 1024 * 1024,
        maximum_packet_size: 65_535,
        ..Default::default()
    };
    // 只协商认证后启用的 OpenSSH 压缩。日志/文本传输可减少线上字节；
    // 不支持此算法的服务器使用 none，不启用认证前的传统 zlib。
    config.preferred.compression =
        std::borrow::Cow::Borrowed(&[russh::compression::ZLIB_LEGACY, russh::compression::NONE]);
    // 只读真实主机对照可恢复旧窗口或关闭压缩；正式程序不读取测量环境变量。
    #[cfg(test)]
    {
        if std::env::var("TERMRILL_KVM_BASE_WINDOW").as_deref() == Ok("1") {
            config.window_size = 2 * 1024 * 1024;
            config.maximum_packet_size = 32_768;
        }
        if std::env::var("TERMRILL_KVM_NO_COMPRESSION").as_deref() == Ok("1") {
            config.preferred.compression = std::borrow::Cow::Borrowed(&[russh::compression::NONE]);
        }
    }
    crate::performance_log!(
        "[TermRill connection] receive_window={} channel_packet={}",
        config.window_size,
        config.maximum_packet_size
    );
    if let Some(name) = algorithms.get(&format!("[{}]:{}", endpoint.host, endpoint.port)) {
        let certificate = name.ends_with("-cert-v01@openssh.com");
        let plain_name = name.strip_suffix("-cert-v01@openssh.com").unwrap_or(name);
        let algorithm = russh::keys::Algorithm::new(plain_name)
            .map_err(|_| AppError::new("已验证的主机密钥算法不被异步库支持"))?;
        if certificate {
            config.preferred.host_key_certificates = std::borrow::Cow::Owned(vec![algorithm]);
            config.preferred.key = std::borrow::Cow::Owned(vec![]);
        } else {
            config.preferred.key = std::borrow::Cow::Owned(vec![algorithm]);
        }
    }
    Ok(Arc::new(config))
}

fn verified<T>(
    result: std::result::Result<T, AppError>,
    rejected: &std::sync::Mutex<Option<HostKey>>,
    backend: Option<&Backend>,
) -> Result<T> {
    if let Some(challenge) = lock(rejected)?.take() {
        if let Some(backend) = backend {
            lock(&backend.pending_hosts)?.insert(challenge.address.clone(), challenge.clone());
        }
        return Err(AppError {
            message: "上传连接的主机指纹未知或已变化，请确认后重试".into(),
            host_key: Some(challenge),
            timeout: false,
        });
    }
    result
}

impl Connection {
    pub(crate) fn is_closed(&self) -> bool {
        self.target.is_closed() || self._jump.as_ref().is_some_and(|jump| jump.is_closed())
    }
    #[cfg(test)]
    pub async fn connect(
        request: &ConnectRequest,
        trusted: &std::collections::BTreeMap<String, String>,
        algorithms: &std::collections::BTreeMap<String, String>,
        backend: Option<&Backend>,
    ) -> Result<Self> {
        Self::connect_mode(request, trusted, algorithms, backend, false).await
    }

    /// 上传专用连接在目标和跳板握手时都禁用压缩，不能由已有压缩连接替代。
    pub async fn connect_upload(
        request: &ConnectRequest,
        trusted: &std::collections::BTreeMap<String, String>,
        algorithms: &std::collections::BTreeMap<String, String>,
        backend: Option<&Backend>,
    ) -> Result<Self> {
        Self::connect_mode(request, trusted, algorithms, backend, true).await
    }

    async fn connect_mode(
        request: &ConnectRequest,
        trusted: &std::collections::BTreeMap<String, String>,
        algorithms: &std::collections::BTreeMap<String, String>,
        backend: Option<&Backend>,
        upload_uncompressed: bool,
    ) -> Result<Self> {
        let configure = |endpoint: &Endpoint| -> Result<Arc<client::Config>> {
            let mut config = connection_config(endpoint, algorithms)?;
            if upload_uncompressed {
                Arc::get_mut(&mut config)
                    .ok_or_else(|| AppError::new("上传连接配置已被共享"))?
                    .preferred
                    .compression = std::borrow::Cow::Borrowed(&[russh::compression::NONE]);
            }
            Ok(config)
        };
        let config = configure(&request.profile.target)?;
        let mut jump_handle = None;
        let verify = verifier(&request.profile.target, trusted);
        let rejected = verify.rejected.clone();
        let received_transport = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sent_transport = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let result = if let Some(jump) = &request.profile.jump {
            let jump_verify = verifier(jump, trusted);
            let jump_rejected = jump_verify.rejected.clone();
            let mut handle = verified(
                client::connect_stream(
                    configure(jump)?,
                    tcp::connect(&jump.host, jump.port).await?,
                    jump_verify,
                )
                .await,
                &jump_rejected,
                backend,
            )?;
            authenticate(&mut handle, jump, &request.jump_secrets, backend).await?;
            let channel = handle
                .channel_open_direct_tcpip(
                    request.profile.target.host.clone(),
                    request.profile.target.port as u32,
                    "127.0.0.1",
                    0,
                )
                .await?;
            let stream = upload_transport::UploadTransport::new(
                channel.into_stream(),
                received_transport.clone(),
                sent_transport.clone(),
            );
            let result = client::connect_stream(config, stream, verify).await;
            jump_handle = Some(handle);
            result
        } else {
            let socket =
                tcp::connect(&request.profile.target.host, request.profile.target.port).await?;
            #[cfg(all(test, windows))]
            crate::kvm_read_perf::register_socket(&socket);
            let stream = upload_transport::UploadTransport::new(
                socket,
                received_transport.clone(),
                sent_transport.clone(),
            );
            client::connect_stream(config, stream, verify).await
        };
        let mut target = verified(result, &rejected, backend)?;
        authenticate(
            &mut target,
            &request.profile.target,
            &request.target_secrets,
            backend,
        )
        .await?;
        let channel = target.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let stream = channel.into_stream();
        let received_data = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stream = receive_trace::ReceiveTrace::new(stream, received_data.clone());
        let mut sftp = RawSftpSession::new(stream);
        // 默认库只有 10 秒；上传请求允许较慢服务器，取消由外层 select 主动响应。
        sftp.set_timeout(300);
        let version = sftp.init().await?;
        if version.version != 3 {
            return Err(AppError::new("异步上传需要服务器提供 SFTP v3"));
        }
        let posix = version
            .extensions
            .get("posix-rename@openssh.com")
            .is_some_and(|v| v == "1");
        let mut write_size = BLOCK;
        // 未声明 limits 的服务端保留标准大小；明确支持时下载采用 128 KiB。
        // SFTP 帧可跨多个 SSH 数据包，下载分片不受单个 SSH 包长直接限制。
        let mut download_read_size = BLOCK;
        // ZIP 随机预览保持已有小范围策略；完整下载与打包来源使用独立的大分片。
        // SFTP DATA 帧可以跨 SSH 数据包，不能把通道包长当作读取上限。
        let mut archive_read_size = 30_000;
        let mut max_files = 32;
        if version
            .extensions
            .get("limits@openssh.com")
            .is_some_and(|v| v == "1")
        {
            let limits = sftp.limits().await?;
            if limits.max_read_len > 0 {
                download_read_size = (128 * 1024).min(limits.max_read_len as usize);
                archive_read_size = archive_read_size.min(limits.max_read_len as usize);
            }
            if limits.max_open_handles > 0 {
                max_files = max_files.min(limits.max_open_handles as usize);
            }
            if limits.max_write_len > 0 {
                write_size = if upload_uncompressed {
                    (128 * 1024).min(limits.max_write_len as usize)
                } else {
                    write_size.min(limits.max_write_len as usize)
                };
            }
            if limits.max_packet_len > 0 {
                write_size = write_size.min(limits.max_packet_len.saturating_sub(1024) as usize);
                archive_read_size =
                    archive_read_size.min(limits.max_packet_len.saturating_sub(1024) as usize);
                download_read_size =
                    download_read_size.min(limits.max_packet_len.saturating_sub(1024) as usize);
            }
            sftp.set_limits(limits.into());
        }
        crate::performance_log!(
            "[TermRill upload connection] download_read_size={} write_size={}",
            download_read_size,
            write_size
        );
        if archive_read_size == 0 || download_read_size == 0 {
            return Err(AppError::new("服务器 SFTP 读取长度限制无效"));
        }
        if write_size == 0 {
            return Err(AppError::new("服务器 SFTP 写入长度限制无效"));
        }
        Ok(Self {
            upload_uncompressed,
            sftp,
            posix,
            write_size,
            download_read_size,
            max_files,
            upload_window: std::sync::atomic::AtomicUsize::new(WRITES),
            upload_rate: std::sync::Mutex::new(None),
            sent_transport,
            received_transport,
            received_data,
            target,
            _jump: jump_handle,
        })
    }

    pub async fn close(self) {
        let _ = self.sftp.close_session();
        drop(self.sftp);
        let mut target = self.target;
        let _ = target
            .disconnect(russh::Disconnect::ByApplication, "transfer finished", "")
            .await;
        // disconnect 仅排入消息队列；等待网络任务退出，避免连续任务延后释放 socket。
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut target).await;
        drop(target);
        if let Some(mut jump) = self._jump {
            let _ = jump
                .disconnect(russh::Disconnect::ByApplication, "transfer finished", "")
                .await;
            let _ = tokio::time::timeout(Duration::from_secs(5), &mut jump).await;
        }
    }
}

async fn authenticate(
    handle: &mut client::Handle<Verify>,
    endpoint: &Endpoint,
    secrets: &Secrets,
    backend: Option<&Backend>,
) -> Result<()> {
    let success = match endpoint.auth.as_str() {
        "password" => {
            let response = handle.authenticate_none(&endpoint.username).await?;
            if response.success() {
                true
            } else if matches!(&response, russh::client::AuthResult::Failure { remaining_methods, .. } if remaining_methods.contains(&russh::MethodKind::Password))
            {
                handle
                    .authenticate_password(&endpoint.username, &secrets.password)
                    .await?
                    .success()
            } else {
                interactive(handle, endpoint, secrets, None).await?
            }
        }
        "key" => {
            let path = endpoint.key_path.clone();
            let passphrase = secrets.passphrase.clone();
            // 私钥读取/解密是本地磁盘和 CPU 工作，不能占用 Tokio 网络任务线程。
            let key = tokio::task::spawn_blocking(move || {
                russh::keys::load_secret_key(
                    path,
                    (!passphrase.is_empty()).then_some(passphrase.as_str()),
                )
            })
            .await
            .map_err(|_| AppError::new("读取上传私钥任务异常"))?
            .map_err(|_| AppError::new("读取上传私钥失败，请检查路径、格式和口令"))?;
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            handle
                .authenticate_publickey(
                    &endpoint.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await?
                .success()
        }
        "interactive" => interactive(handle, endpoint, secrets, backend).await?,
        "agent" => authenticate_agent(handle, endpoint).await?,
        _ => return Err(AppError::new("不支持的上传认证方式")),
    };
    if !success {
        return Err(AppError::new("异步上传连接认证失败"));
    }
    Ok(())
}

async fn interactive(
    handle: &mut client::Handle<Verify>,
    endpoint: &Endpoint,
    secrets: &Secrets,
    backend: Option<&Backend>,
) -> Result<bool> {
    use russh::client::KeyboardInteractiveAuthResponse as Reply;
    let mut response = handle
        .authenticate_keyboard_interactive_start(&endpoint.username, None)
        .await?;
    let mut answered = false;
    for _ in 0..16 {
        match response {
            Reply::Success => return Ok(true),
            Reply::Failure { .. } => return Ok(false),
            Reply::InfoRequest {
                instructions,
                prompts,
                ..
            } => {
                let answers = if prompts.is_empty() {
                    Vec::new()
                } else if let Some(backend) = backend {
                    crate::auth::answer_async(
                        backend,
                        endpoint,
                        &instructions,
                        prompts.iter().map(|p| (p.prompt.clone(), p.echo)).collect(),
                    )
                    .await?
                } else if !answered
                    && prompts.len() == 1
                    && !prompts[0].echo
                    && (prompts[0].prompt.to_lowercase().contains("password")
                        || prompts[0].prompt.contains("密码"))
                {
                    answered = true;
                    vec![secrets.password.clone()]
                } else {
                    return Err(AppError::new(
                        "上传认证需要交互式验证码，请使用交互认证方式",
                    ));
                };
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Err(AppError::new("上传认证挑战次数超出限制"))
}

async fn authenticate_agent(
    handle: &mut client::Handle<Verify>,
    endpoint: &Endpoint,
) -> Result<bool> {
    use russh::keys::agent::client::AgentClient;
    #[cfg(windows)]
    let mut agent = match AgentClient::connect_named_pipe("\\\\.\\pipe\\openssh-ssh-agent").await {
        Ok(agent) => agent.dynamic(),
        Err(_) => AgentClient::connect_pageant()
            .await
            .map_err(|error| AppError::new(format!("连接 SSH Agent 失败：{error}")))?
            .dynamic(),
    };
    #[cfg(unix)]
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|error| AppError::new(format!("连接 SSH Agent 失败：{error}")))?;
    let keys = agent
        .request_identities()
        .await
        .map_err(|error| AppError::new(format!("读取 Agent 公钥失败：{error}")))?;
    let hash = handle.best_supported_rsa_hash().await?.flatten();
    for key in keys {
        let response = match key {
            russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                handle
                    .authenticate_publickey_with(&endpoint.username, key, hash, &mut agent)
                    .await
            }
            russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                handle
                    .authenticate_certificate_with(
                        &endpoint.username,
                        certificate,
                        hash,
                        &mut agent,
                    )
                    .await
            }
        }
        .map_err(|error| AppError::new(format!("上传 Agent 认证失败：{error}")))?;
        if response.success() {
            return Ok(true);
        }
    }
    Ok(false)
}
