//! RAR/7Z 受限转换为 Stored ZIP，复用现有预览、校验和安全安装流程。
use super::*;
use std::io::Write;

#[path = "archive_rar.rs"]
mod rar;

const MEMBER_LIMIT: u64 = 256 * 1024 * 1024;
type Writer = ZipWriter<ArchiveBuffer>;

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
}

fn check_entries(entries: &[Entry]) -> Result<()> {
    if entries.len() > ENTRY_LIMIT {
        return Err(AppError::new("压缩包超过 10000 项"));
    }
    validate(entries)?;
    if entries.iter().map(|entry| entry.size).sum::<u64>() > ARCHIVE_LIMIT {
        return Err(AppError::new("RAR/7Z 展开内容超过 512 MiB"));
    }
    Ok(())
}

fn start(writer: &mut Writer, entry: &Entry) -> Result<()> {
    if entry.directory {
        writer.add_directory(format!("{}/", entry.name.trim_end_matches('/')), options())
    } else {
        writer.start_file(&entry.name, options())
    }
    .map_err(zip_error)
}

fn copy_member(
    reader: &mut dyn Read,
    writer: &mut Writer,
    size: u64,
    io: &ArchiveIo<'_>,
) -> Result<()> {
    let mut copied = 0u64;
    let mut chunk = [0; 64 * 1024];
    loop {
        io.check()?;
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > size || copied > MEMBER_LIMIT {
            return Err(AppError::new("包内文件实际大小超过声明或 256 MiB 限制"));
        }
        writer.write_all(&chunk[..count])?;
    }
    if copied != size {
        return Err(AppError::new("包内文件内容不完整"));
    }
    Ok(())
}

fn seven_error(_: sevenz_rust2::Error) -> AppError {
    // 不透传第三方错误上下文，密码不进入诊断信息。
    AppError::new("7Z 解析或校验失败，请检查密码、压缩方法及文件完整性")
}

/// 解码器获取压缩字节时也检查取消，覆盖目录头和固实块的读取阶段。
struct CheckedSource<'a, 'b> {
    cursor: Cursor<Vec<u8>>,
    io: &'a ArchiveIo<'b>,
}
impl Read for CheckedSource<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.io
            .check()
            .map_err(|error| std::io::Error::other(error.message))?;
        self.cursor.read(bytes)
    }
}
impl std::io::Seek for CheckedSource<'_, '_> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

fn check_seven_blocks(archive: &sevenz_rust2::Archive) -> Result<()> {
    for block in &archive.blocks {
        if block.get_unpack_size() > ARCHIVE_LIMIT {
            return Err(AppError::new("7Z 固实块展开量超过 512 MiB"));
        }
        for coder in &block.coders {
            let properties = coder.properties();
            let memory = match coder.encoder_method_id() {
                [0x03, 0x01, 0x01] if properties.len() >= 5 => u64::from(u32::from_le_bytes(
                    properties[1..5]
                        .try_into()
                        .map_err(|_| AppError::new("7Z 字典属性无效"))?,
                )),
                [0x21] if !properties.is_empty() && properties[0] <= 40 => {
                    let bits = u32::from(properties[0]);
                    (2 | (bits & 1)) as u64 * (1u64 << (bits / 2 + 11))
                }
                [0x03, 0x04, 0x01] if properties.len() >= 5 => u64::from(u32::from_le_bytes(
                    properties[1..5]
                        .try_into()
                        .map_err(|_| AppError::new("7Z PPMd 属性无效"))?,
                )),
                _ => 0,
            };
            if memory > MEMBER_LIMIT {
                return Err(AppError::new("7Z 内容解码字典超过 256 MiB"));
            }
        }
    }
    Ok(())
}

fn seven(bytes: Vec<u8>, password: Option<&str>, io: &ArchiveIo<'_>) -> Result<Vec<u8>> {
    let mut archive = sevenz_rust2::ArchiveReader::new(
        CheckedSource {
            cursor: Cursor::new(bytes),
            io,
        },
        sevenz_rust2::Password::from(password.unwrap_or("")),
    )
    .map_err(seven_error)?;
    archive.set_thread_count(1);
    check_seven_blocks(archive.archive())?;
    let entries: Vec<_> = archive
        .archive()
        .files
        .iter()
        .map(|entry| {
            let name = entry.name.replace('\\', "/");
            let mode = (entry.windows_attributes >> 16) & 0o170000;
            let regular = !entry.has_windows_attributes
                || (entry.windows_attributes & 0x400 == 0
                    && matches!(mode, 0 | 0o100000 | 0o040000));
            Entry {
                safe: safe_name(&name).is_ok() && regular && !entry.is_anti_item,
                name,
                size: entry.size,
                compressed: entry.compressed_size,
                directory: entry.is_directory,
                encrypted: false,
            }
        })
        .collect();
    check_entries(&entries)?;
    let mut writer = Writer::new(ArchiveBuffer::default());
    let mut failure = None;
    let outcome = archive.for_each_entries(|entry, reader| {
        let result = (|| {
            io.check()?;
            let name = safe_name(&entry.name.replace('\\', "/"))?;
            let metadata = Entry {
                name,
                size: entry.size,
                compressed: entry.compressed_size,
                directory: entry.is_directory,
                encrypted: false,
                safe: true,
            };
            start(&mut writer, &metadata)?;
            if !metadata.directory {
                copy_member(reader, &mut writer, metadata.size, io)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            failure = Some(error);
            return Err(sevenz_rust2::Error::Other("归档读取已停止".into()));
        }
        Ok(true)
    });
    if let Some(error) = failure {
        return Err(error);
    }
    outcome.map_err(seven_error)?;
    io.check()?;
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}

pub(super) fn decode(
    path: &str,
    bytes: Vec<u8>,
    password: Option<&str>,
    io: &ArchiveIo<'_>,
) -> Result<Vec<u8>> {
    io.check()?;
    io.phase("正在解析压缩内容");
    if path.ends_with(".7z") {
        seven(bytes, password, io)
    } else {
        rar::decode(bytes, password, io)
    }
}

#[cfg(test)]
#[path = "archive_extra_tests.rs"]
mod tests;
