//! 下载开始前查询目标卷可用空间；仅是瞬时预检，不预留容量，写盘错误仍须原样处理。
use crate::model::{AppError, Result};
use std::path::Path;

fn require_capacity(available: u64, required: u64) -> Result<()> {
    if available < required {
        return Err(AppError::new(format!(
            "本地存储空间不足：尚需 {required} 字节，可用 {available} 字节；请释放空间或选择其他目录后续传"
        )));
    }
    Ok(())
}

/// 在阻塞线程调用；remaining 是未下载字节，不重复计算已存在的连续前缀或旧目标。
pub(crate) fn ensure(path: &Path, remaining: u64) -> Result<()> {
    if remaining == 0 {
        return Ok(());
    }
    let available =
        available(path).map_err(|error| AppError::new(format!("无法检查本地可用空间：{error}")))?;
    require_capacity(available, remaining)
}

#[cfg(windows)]
fn available(path: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    // UNC 卷根查询要求尾随分隔符；普通目录加分隔符也不会改变目标卷。
    if !matches!(wide.last(), Some(47 | 92)) {
        wide.push(92);
    }
    wide.push(0);
    let mut bytes = 0;
    // 缓冲以 NUL 结尾且指针在调用期间有效；查询当前用户可用容量，包含卷配额约束。
    let success = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(bytes)
}

#[cfg(unix)]
fn available(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs 成功才读取已初始化结构；bavail 是非特权用户可用块数。
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok((u128::from(info.f_bavail) * u128::from(info.f_frsize)).min(u128::from(u64::MAX)) as u64)
}

#[cfg(not(any(windows, unix)))]
fn available(_: &Path) -> std::io::Result<u64> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_boundary_and_resume_budget() {
        assert!(require_capacity(40, 100 - 60).is_ok());
        assert!(require_capacity(39, 100 - 60)
            .unwrap_err()
            .message
            .contains("空间不足"));
        assert!(require_capacity(0, 0).is_ok());
    }
    #[test]
    fn volume_query_and_missing_directory() {
        assert!(available(&std::env::temp_dir()).is_ok());
        assert!(
            available(&std::env::temp_dir().join("termrill-no-such-volume-query-directory"))
                .is_err()
        );
        assert!(ensure(Path::new(""), 0).is_ok());
    }
}
