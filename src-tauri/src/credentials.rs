//! 用户明确选择后才读写系统凭据库；连接配置文件不包含凭据。
use crate::model::*;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
use sha2::{Digest, Sha256};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSecrets {
    pub target_secrets: Secrets,
    pub jump_secrets: Secrets,
}

// 地址/用户/认证方式变化后使用不同条目，避免将旧密码提交给新的服务器。
#[cfg(windows)]
fn target(profile: &Profile) -> Result<Vec<u16>> {
    let identity = serde_json::to_vec(&(&profile.id, &profile.target, &profile.jump))?;
    let name = format!(
        "TermRill/{}",
        format_args!("{:x}", Sha256::digest(identity))
    );
    Ok(name.encode_utf16().chain(Some(0)).collect())
}

#[cfg(windows)]
pub fn store(profile: &Profile, secrets: &SavedSecrets) -> Result<()> {
    use windows_sys::Win32::Security::Credentials::*;
    let mut name = target(profile)?;
    let mut user: Vec<u16> = profile
        .target
        .username
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut blob = serde_json::to_vec(secrets)?;
    if blob.len() > 2560 {
        return Err(AppError::new("凭据过长，无法保存到 Windows 凭据库"));
    }
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        UserName: user.as_mut_ptr(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // 指针指向仍存活的缓冲区，CredWriteW 在返回前复制数据。
    let success = unsafe { CredWriteW(&credential, 0) };
    blob.fill(0);
    if success == 0 {
        return Err(AppError::new("Windows 凭据库保存失败"));
    }
    Ok(())
}

#[cfg(windows)]
pub fn load(profile: &Profile) -> Result<Option<SavedSecrets>> {
    use windows_sys::Win32::Security::Credentials::*;
    let name = target(profile)?;
    let mut pointer = std::ptr::null_mut();
    // Win32 分配的结果只在 CredFree 前使用；无论 JSON 解析成功与否都释放。
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } == 0 {
        if std::io::Error::last_os_error().raw_os_error() == Some(1168) {
            return Ok(None);
        }
        return Err(AppError::new("Windows 凭据库读取失败"));
    }
    let result = unsafe {
        let credential = &*pointer;
        if credential.CredentialBlobSize == 0 || credential.CredentialBlob.is_null() {
            Err(AppError::new("保存的凭据为空"))
        } else {
            serde_json::from_slice(std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            ))
            .map_err(AppError::from)
        }
    };
    unsafe {
        CredFree(pointer.cast());
    }
    result.map(Some)
}

#[cfg(windows)]
pub fn remove(profile: &Profile) -> Result<()> {
    use windows_sys::Win32::Security::Credentials::*;
    let name = target(profile)?;
    if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0
        && std::io::Error::last_os_error().raw_os_error() != Some(1168)
    {
        return Err(AppError::new("Windows 凭据库删除失败"));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
static CREDENTIAL_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn entry(profile: &Profile) -> Result<keyring::Entry> {
    let identity = serde_json::to_vec(&(&profile.id, &profile.target, &profile.jump))?;
    let account = format!("{:x}", Sha256::digest(identity));
    keyring::Entry::new("dev.termrill.sftpclient", &account)
        .map_err(|_| AppError::new("系统凭据库不可用，请检查钥匙串或 Secret Service"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn store(profile: &Profile, secrets: &SavedSecrets) -> Result<()> {
    // 系统服务不保证同一条目的并发访问顺序；在工作线程中串行访问。
    let _guard = lock(&CREDENTIAL_ACCESS)?;
    let entry = entry(profile)?;
    let mut bytes = serde_json::to_vec(secrets)?;
    let result = entry
        .set_secret(&bytes)
        .map_err(|_| AppError::new("系统凭据库保存失败，请检查是否已解锁"));
    bytes.fill(0);
    result
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn load(profile: &Profile) -> Result<Option<SavedSecrets>> {
    let _guard = lock(&CREDENTIAL_ACCESS)?;
    match entry(profile)?.get_secret() {
        Ok(mut bytes) => {
            let result = serde_json::from_slice(&bytes).map_err(AppError::from);
            bytes.fill(0);
            result.map(Some)
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        // 不直接格式化 keyring 错误：某些错误携带原始凭据字节。
        Err(_) => Err(AppError::new("系统凭据库读取失败，请检查是否已解锁")),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn remove(profile: &Profile) -> Result<()> {
    let _guard = lock(&CREDENTIAL_ACCESS)?;
    match entry(profile)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(AppError::new("系统凭据库删除失败")),
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn store(_: &Profile, _: &SavedSecrets) -> Result<()> {
    Err(AppError::new("当前平台尚未接入系统凭据库"))
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn load(_: &Profile) -> Result<Option<SavedSecrets>> {
    Err(AppError::new("当前平台尚未接入系统凭据库"))
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn remove(_: &Profile) -> Result<()> {
    // 未接入的平台没有保存过凭据，不阻止普通连接配置删除与修改。
    Ok(())
}
