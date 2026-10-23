//! 将受限的 TAR/压缩流转换为内存 Stored ZIP，复用已有路径预检与安装保护。
//! 转换最多 512 MiB；不执行外部程序，不解引用 TAR 链接。
use super::*;
use std::io::Write;

fn read_bounded(reader: &mut dyn Read, io: &ArchiveIo<'_>, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        io.check()?;
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > limit {
            return Err(AppError::new(
                "展开内容超过当前格式的 512 MiB 总量或 256 MiB 单文件限制",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

pub(super) fn decode(
    path: &str,
    bytes: Vec<u8>,
    password: Option<&str>,
    io: &ArchiveIo<'_>,
) -> Result<Vec<u8>> {
    let lower = path.to_lowercase();
    if lower.ends_with(".7z") || lower.ends_with(".rar") {
        return super::extra::decode(&lower, bytes, password, io);
    }
    let tar_format = lower.ends_with(".tar")
        || [
            ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tbz", ".tar.xz", ".txz",
        ]
        .iter()
        .any(|suffix| lower.ends_with(suffix));
    let mut reader: Box<dyn Read> = if lower.ends_with(".gz") || lower.ends_with(".tgz") {
        Box::new(flate2::read::MultiGzDecoder::new(Cursor::new(bytes)))
    } else if lower.ends_with(".bz2") || lower.ends_with(".tbz2") || lower.ends_with(".tbz") {
        Box::new(bzip2::read::MultiBzDecoder::new(Cursor::new(bytes)))
    } else if lower.ends_with(".xz") || lower.ends_with(".txz") {
        Box::new(xz2::read::XzDecoder::new_multi_decoder(Cursor::new(bytes)))
    } else if lower.ends_with(".tar") {
        Box::new(Cursor::new(bytes))
    } else {
        return Err(AppError::new(
            "支持 ZIP、RAR、7Z、TAR、GZ、BZ2、XZ 及 TAR 压缩变体",
        ));
    };
    io.phase("正在解析压缩内容");
    let mut writer = ZipWriter::new(ArchiveBuffer::default());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    if tar_format {
        // 解压流本身也有界，包含头部和填充，避免巨大空目录包无限扫描。
        let expanded = read_bounded(reader.as_mut(), io, ARCHIVE_LIMIT)?;
        let mut archive = tar::Archive::new(Cursor::new(expanded));
        let mut total = 0u64;
        for (index, entry) in archive.entries()?.enumerate() {
            io.check()?;
            if index >= ENTRY_LIMIT {
                return Err(AppError::new("压缩包超过 10000 项"));
            }
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            if !kind.is_file() && !kind.is_dir() {
                return Err(AppError::new("TAR 包包含链接或特殊文件，已拒绝操作"));
            }
            let name = entry
                .path()?
                .to_str()
                .ok_or_else(|| AppError::new("包内路径不是 UTF-8"))?
                .trim_start_matches("./")
                .to_string();
            if (name.is_empty() || name == ".") && kind.is_dir() {
                continue;
            }
            let name = safe_name(&name)?;
            if kind.is_dir() {
                writer
                    .add_directory(format!("{name}/"), options)
                    .map_err(zip_error)?;
            } else {
                let content = read_bounded(&mut entry, io, 256 * 1024 * 1024)?;
                total += content.len() as u64;
                if total > ARCHIVE_LIMIT {
                    return Err(AppError::new("TAR 展开内容超过 512 MiB"));
                }
                writer.start_file(name, options).map_err(zip_error)?;
                writer.write_all(&content)?;
            }
        }
    } else {
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::new("压缩文件名称无效"))?;
        let name = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
        let name = safe_name(name)?;
        let content = read_bounded(reader.as_mut(), io, 256 * 1024 * 1024)?;
        writer.start_file(name, options).map_err(zip_error)?;
        writer.write_all(&content)?;
    }
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}
