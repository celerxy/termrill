//! 本地有界编辑及属性；拒绝链接入口，保存保留编码并检查原始字节摘要。
use crate::{files, model::*};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// 枚举 Windows 已分配的盘符，不逐盘访问，避免空光驱和断开的网络盘阻塞导航。
pub(crate) fn drives() -> Result<Vec<String>> {
    #[cfg(windows)]
    {
        // 无参数、无指针的系统查询；返回值的低 26 位依次对应 A 到 Z。
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        if mask == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok((0..26)
            .filter(|index| mask & (1 << index) != 0)
            .map(|index| format!("{}:\\", (b'A' + index) as char))
            .collect())
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

fn metadata(path: &Path) -> Result<fs::Metadata> {
    if !path.is_absolute() || path.to_string_lossy().contains('\0') {
        return Err(AppError::new("本地路径无效"));
    }
    for parent in path.ancestors().skip(1) {
        if local_link(&fs::symlink_metadata(parent)?) {
            return Err(AppError::new("不支持通过链接父级读取或编辑文件"));
        }
    }
    Ok(fs::symlink_metadata(path)?)
}
pub(crate) fn regular(path: &Path) -> Result<fs::Metadata> {
    let stat = metadata(path)?;
    if !stat.is_file() || local_link(&stat) {
        return Err(AppError::new("仅支持普通文件"));
    }
    Ok(stat)
}
fn read(path: &Path) -> Result<Vec<u8>> {
    if regular(path)?.len() > files::EDIT_LIMIT {
        return Err(AppError::new("仅支持编辑 2 MiB 以内的普通文本文件"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(files::EDIT_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > files::EDIT_LIMIT {
        return Err(AppError::new("文件超过 2 MiB"));
    }
    Ok(bytes)
}
pub fn open(path: &Path) -> Result<files::Document> {
    files::document(&read(path)?)
}

pub fn save(path: &Path, content: &str, revision: &str) -> Result<files::Document> {
    let unchanged = || -> Result<Vec<u8>> {
        let bytes = read(path)?;
        if format!("{:x}", Sha256::digest(&bytes)) != revision {
            return Err(AppError::new("本地文件已被修改，请重新打开后合并内容"));
        }
        Ok(bytes)
    };
    let original = unchanged()?;
    let permissions = regular(path)?.permissions();
    if permissions.readonly() {
        return Err(AppError::new("本地文件为只读，无法保存"));
    }
    let (_, encoding) = files::decode(&original)?;
    let bytes = files::encode(content, encoding)?;
    let temporary = path.with_file_name(format!(".termrill-edit-{}.part", identifier()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::set_permissions(&temporary, permissions)?;
        unchanged()?;
        // 检查与替换之间仍有竞态；不能把摘要检查宣称为 compare-and-swap。
        replace(&temporary, path)?;
        files::document(&bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
#[cfg(windows)]
pub(crate) fn replace(temporary: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let temporary: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    // 原生替换保留目标 ACL；不通过先删除旧文件来绕过共享锁或权限错误。
    let success = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            temporary.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(windows))]
pub(crate) fn replace(temporary: &Path, target: &Path) -> Result<()> {
    Ok(fs::rename(temporary, target)?)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    path: String,
    kind: &'static str,
    size: u64,
    readonly: bool,
    modified: Option<u64>,
    created: Option<u64>,
    accessed: Option<u64>,
}
pub fn properties(path: &Path) -> Result<Properties> {
    let stat = metadata(path)?;
    let timestamp = |time: std::io::Result<SystemTime>| {
        time.ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|time| time.as_secs())
    };
    Ok(Properties {
        path: path.to_string_lossy().into(),
        kind: if local_link(&stat) {
            "链接"
        } else if stat.is_dir() {
            "文件夹"
        } else if stat.is_file() {
            "文件"
        } else {
            "特殊文件"
        },
        size: stat.len(),
        readonly: stat.permissions().readonly(),
        modified: timestamp(stat.modified()),
        created: timestamp(stat.created()),
        accessed: timestamp(stat.accessed()),
    })
}
