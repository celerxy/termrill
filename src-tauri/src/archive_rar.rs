//! 内置 UnRAR 只读解码：TEST 回调写入有界内存，不让原生库解压到文件路径。
//! UnRAR source code may be used in any software to handle RAR archives without
//! limitations free of charge, but cannot be used to develop RAR (WinRAR)
//! compatible archiver and to re-create RAR compression algorithm, which is
//! proprietary. Distribution of modified UnRAR source code in separate form or
//! as a part of other software is permitted, provided that full text of this
//! paragraph, starting from "UnRAR source code" words, is included in license,
//! or in documentation if license is not available, and in source code comments
//! of resulting package.
use super::*;
use std::{
    ffi::CString,
    fs::OpenOptions,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    ptr,
};
use unrar_sys as native;

struct Source(PathBuf);
impl Drop for Source {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
impl Source {
    fn create(bytes: &[u8], io: &ArchiveIo<'_>) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("termrill-rar-{}.rar", identifier()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // create_new 失败时不能删除可能由其他进程持有的同名文件。
        let mut file = options.open(&path)?;
        let source = Self(path);
        let outcome = (|| -> Result<()> {
            for chunk in bytes.chunks(64 * 1024) {
                io.check()?;
                file.write_all(chunk)?;
            }
            Ok(())
        })();
        // Windows 不能删除仍被 file 持有的文件；错误返回前先关闭写句柄。
        drop(file);
        outcome?;
        Ok(source)
    }
}

struct Handle(*const native::Handle);
impl Drop for Handle {
    fn drop(&mut self) {
        // 句柄总在回调上下文之前释放；关闭时移除回调，避免悬垂 userdata。
        unsafe {
            native::RARSetCallback(self.0, None, 0);
            native::RARCloseArchive(self.0);
        }
    }
}

struct Context<'a, 'b> {
    io: &'a ArchiveIo<'b>,
    writer: &'a mut Writer,
    password: &'a str,
    copied: u64,
    expected: u64,
    failure: Option<AppError>,
}

extern "C" fn callback(
    message: native::UINT,
    user: native::LPARAM,
    first: native::LPARAM,
    second: native::LPARAM,
) -> i32 {
    // 原生调用同步执行；userdata 指向本次 decode 的 Context，不跨线程或调用生命周期。
    let context = unsafe { &mut *(user as *mut Context<'_, '_>) };
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        context.io.check()?;
        match message {
            native::UCM_PROCESSDATA => {
                let size =
                    usize::try_from(second).map_err(|_| AppError::new("RAR 数据块大小无效"))?;
                if first == 0 || size as u64 > context.expected.saturating_sub(context.copied) {
                    return Err(AppError::new("RAR 实际内容超过声明大小"));
                }
                // PROCESSDATA 缓冲区由 UnRAR 提供，长度已经校验，仅读取且不留存指针。
                let bytes = unsafe { std::slice::from_raw_parts(first as *const u8, size) };
                context.writer.write_all(bytes)?;
                context.copied += size as u64;
            }
            native::UCM_NEEDPASSWORD | native::UCM_NEEDPASSWORDW => {
                if context.password.is_empty() || first == 0 || second <= 0 {
                    return Err(AppError::new("RAR 需要密码，请在解压设置中填写密码后重试"));
                }
                if message == native::UCM_NEEDPASSWORDW {
                    let password = wide(context.password);
                    if password.len() > second as usize {
                        return Err(AppError::new("RAR 密码过长"));
                    }
                    unsafe {
                        ptr::copy_nonoverlapping(
                            password.as_ptr(),
                            first as *mut native::WCHAR,
                            password.len(),
                        );
                    }
                } else {
                    let password = CString::new(context.password)
                        .map_err(|_| AppError::new("密码不能包含空字符"))?;
                    if password.as_bytes_with_nul().len() > second as usize {
                        return Err(AppError::new("RAR 密码过长"));
                    }
                    unsafe {
                        ptr::copy_nonoverlapping(
                            password.as_ptr(),
                            first as *mut std::ffi::c_char,
                            password.as_bytes_with_nul().len(),
                        );
                    }
                }
            }
            native::UCM_CHANGEVOLUME | native::UCM_CHANGEVOLUMEW => {
                return Err(AppError::new("暂不支持 RAR 分卷压缩包"));
            }
            _ => {}
        }
        Ok(())
    }));
    match outcome {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            context.failure = Some(error);
            -1
        }
        Err(_) => {
            context.failure = Some(AppError::new("RAR 解码回调失败"));
            -1
        }
    }
}

fn wide(text: &str) -> Vec<native::WCHAR> {
    #[cfg(windows)]
    let mut values: Vec<_> = text
        .encode_utf16()
        .map(|value| value as native::WCHAR)
        .collect();
    #[cfg(not(windows))]
    let mut values: Vec<_> = text.chars().map(|value| value as native::WCHAR).collect();
    values.push(0);
    values
}

fn filename(header: &native::HeaderDataEx) -> Result<String> {
    let length = header
        .filename_w
        .iter()
        .position(|value| *value == 0)
        .ok_or_else(|| AppError::new("RAR 包内路径过长"))?;
    #[cfg(windows)]
    let name = String::from_utf16(&header.filename_w[..length])
        .map_err(|_| AppError::new("RAR 文件名编码无效"))?;
    #[cfg(not(windows))]
    let name = header.filename_w[..length]
        .iter()
        .map(|value| {
            char::from_u32(*value as u32).ok_or_else(|| AppError::new("RAR 文件名编码无效"))
        })
        .collect::<Result<String>>()?;
    safe_name(&name.replace('\\', "/"))
}

fn status(code: i32, context: &mut Context<'_, '_>) -> Result<()> {
    if let Some(error) = context.failure.take() {
        return Err(error);
    }
    if code == native::ERAR_SUCCESS {
        return Ok(());
    }
    let message = match code {
        native::ERAR_MISSING_PASSWORD | native::ERAR_BAD_PASSWORD => "RAR 密码缺失或不正确",
        native::ERAR_BAD_DATA => "RAR 内容损坏或校验失败",
        _ => "RAR 解析失败，请检查文件完整性、密码及压缩格式",
    };
    Err(AppError::new(message))
}

pub(super) fn decode(
    bytes: Vec<u8>,
    password: Option<&str>,
    io: &ArchiveIo<'_>,
) -> Result<Vec<u8>> {
    let source = Source::create(&bytes, io)?;
    drop(bytes);
    let path = source
        .0
        .to_str()
        .ok_or_else(|| AppError::new("临时目录路径编码无效"))?;
    let path_w = wide(path);
    #[cfg(any(target_os = "linux", target_os = "netbsd"))]
    let path_c = CString::new(path).map_err(|_| AppError::new("临时目录路径无效"))?;
    #[cfg(any(target_os = "linux", target_os = "netbsd"))]
    let mut open = native::OpenArchiveDataEx::new(path_c.as_ptr(), native::RAR_OM_EXTRACT);
    #[cfg(not(any(target_os = "linux", target_os = "netbsd")))]
    let mut open = native::OpenArchiveDataEx::new(path_w.as_ptr(), native::RAR_OM_EXTRACT);
    open.archive_name_w = path_w.as_ptr();
    let mut writer = Writer::new(ArchiveBuffer::default());
    let mut context = Context {
        io,
        writer: &mut writer,
        password: password.unwrap_or(""),
        copied: 0,
        expected: 0,
        failure: None,
    };
    if context.password.contains('\0') {
        return Err(AppError::new("密码不能包含空字符"));
    }
    open.callback = Some(callback);
    open.user_data = &mut context as *mut Context<'_, '_> as native::LPARAM;
    // UnRAR 的 API 使用 const 指针签名但实际填充返回字段；底层对象为可写且独占的栈值。
    let raw = unsafe { native::RAROpenArchiveEx(&raw mut open) };
    let handle = if raw.is_null() {
        status(open.open_result as i32, &mut context)?;
        return Err(AppError::new("RAR 无法打开"));
    } else {
        Handle(raw)
    };
    status(open.open_result as i32, &mut context)?;
    if open.flags & native::ROADF_VOLUME != 0 {
        return Err(AppError::new("暂不支持 RAR 分卷压缩包"));
    }
    let mut entries = Vec::new();
    let mut total = 0u64;
    loop {
        io.check()?;
        let mut header = native::HeaderDataEx::default();
        let code = unsafe { native::RARReadHeaderEx(handle.0, &raw mut header) };
        if code == native::ERAR_END_ARCHIVE {
            break;
        }
        status(code, &mut context)?;
        if entries.len() >= ENTRY_LIMIT {
            return Err(AppError::new("压缩包超过 10000 项"));
        }
        if header.flags & (native::RHDF_SPLITBEFORE | native::RHDF_SPLITAFTER) != 0 {
            return Err(AppError::new("暂不支持 RAR 分卷压缩包"));
        }
        let size = (u64::from(header.unp_size_high) << 32) | u64::from(header.unp_size);
        total = total
            .checked_add(size)
            .ok_or_else(|| AppError::new("RAR 解压大小无效"))?;
        if size > MEMBER_LIMIT
            || total > ARCHIVE_LIMIT
            || u64::from(header.dict_size) > MEMBER_LIMIT / 1024
        {
            return Err(AppError::new("RAR 展开内容或解码字典超过大小限制"));
        }
        let regular = header.redir_type == 0
            && if header.host_os == 3 {
                matches!(header.file_attr & 0o170000, 0 | 0o100000 | 0o040000)
            } else {
                header.file_attr & 0x400 == 0
            };
        let entry = Entry {
            name: filename(&header)?,
            size,
            compressed: (u64::from(header.pack_size_high) << 32) | u64::from(header.pack_size),
            directory: header.flags & native::RHDF_DIRECTORY != 0,
            encrypted: false,
            safe: regular,
        };
        validate(std::slice::from_ref(&entry))?;
        start(context.writer, &entry)?;
        context.copied = 0;
        context.expected = size;
        // TEST 会校验 CRC 并通过 PROCESSDATA 提供数据，不使用 EXTRACT 写入路径。
        let code = unsafe {
            native::RARProcessFileW(handle.0, native::RAR_TEST, ptr::null(), ptr::null())
        };
        status(code, &mut context)?;
        if context.copied != size {
            return Err(AppError::new("RAR 包内文件内容不完整"));
        }
        entries.push(entry);
    }
    check_entries(&entries)?;
    drop(handle);
    io.check()?;
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}
