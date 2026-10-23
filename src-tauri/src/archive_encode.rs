//! 归档写入能力与编码；经校验的 Stored ZIP 作为有界中间表示，不执行外部程序。
use super::*;
use std::io::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Zip,
    Seven,
    Tar,
    Gzip,
    Bzip,
    Xz,
}
pub(super) fn format(path: &str) -> Result<Format> {
    let lower = path.to_lowercase();
    let result = if lower.ends_with(".zip") {
        Format::Zip
    } else if lower.ends_with(".7z") {
        Format::Seven
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Format::Gzip
    } else if [".tar.bz2", ".tbz", ".tbz2"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
    {
        Format::Bzip
    } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        Format::Xz
    } else if lower.ends_with(".tar") {
        Format::Tar
    } else {
        return Err(AppError::new(
            "该格式仅支持读取；创建和修改支持 ZIP、7Z、TAR 及 TAR.GZ/BZ2/XZ",
        ));
    };
    Ok(result)
}
pub(super) fn level(request: &Request) -> Result<u32> {
    match request.compression.as_deref().unwrap_or("normal") {
        "store" => Ok(0),
        "fast" => Ok(1),
        "normal" => Ok(6),
        "best" => Ok(9),
        _ => Err(AppError::new("不支持的压缩等级")),
    }
}
pub(super) fn validate_options(request: &Request) -> Result<Format> {
    let format = format(&request.path)?;
    level(request)?;
    if let Some(password) = &request.password {
        if password.len() > 1024 || password.contains('\0') {
            return Err(AppError::new("归档密码不得含 NUL 或超过 1024 字节"));
        }
        if !password.is_empty() && !matches!(format, Format::Zip | Format::Seven) {
            return Err(AppError::new("TAR 系列不支持加密，请清空密码或选择 ZIP/7Z"));
        }
    }
    Ok(format)
}
pub(super) fn zip_options(request: &Request) -> Result<zip::write::FileOptions<'_, ()>> {
    // 非 ZIP 写入只用 Stored 中间表示，避免对远程来源重复压缩。
    if format(&request.path)? != Format::Zip {
        return Ok(SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644));
    }
    let level = level(request)?;
    let options = SimpleFileOptions::default()
        .compression_method(if level == 0 {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        })
        .compression_level(if level == 0 { None } else { Some(level as i64) })
        .unix_permissions(0o644);
    Ok(
        match request
            .password
            .as_deref()
            .filter(|password| !password.is_empty())
        {
            Some(password) => options.with_aes_encryption(zip::AesMode::Aes256, password),
            None => options,
        },
    )
}

pub(super) fn encode(request: &Request, bytes: Vec<u8>, io: &ArchiveIo<'_>) -> Result<Vec<u8>> {
    let format = validate_options(request)?;
    if format == Format::Zip {
        return Ok(bytes);
    }
    io.check()?;
    io.phase("正在编码目标归档格式");
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    let listed = entries(&mut archive)?;
    validate(&listed)?;
    let level = level(request)?;
    if format == Format::Seven {
        return seven(request, &mut archive, &listed, level, io);
    }
    let mut builder = tar::Builder::new(ArchiveBuffer::default());
    for (index, entry) in listed.iter().enumerate() {
        io.check()?;
        let mut header = tar::Header::new_gnu();
        header.set_mode(if entry.directory { 0o755 } else { 0o644 });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_entry_type(if entry.directory {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        header.set_size(if entry.directory { 0 } else { entry.size });
        header.set_cksum();
        let content = if entry.directory {
            Vec::new()
        } else {
            read_entry(&mut archive, index, None, 256 * 1024 * 1024, io)?
        };
        builder.append_data(
            &mut header,
            &entry.name,
            CheckedReader {
                cursor: Cursor::new(content),
                io,
            },
        )?;
    }
    let tar = builder.into_inner()?.into_inner();
    drop(archive);
    io.check()?;
    match format {
        Format::Tar => Ok(tar),
        Format::Gzip => {
            let mut output = flate2::write::GzEncoder::new(
                ArchiveBuffer::default(),
                flate2::Compression::new(level),
            );
            copy_chunks(&tar, &mut output, io)?;
            Ok(output.finish()?.into_inner())
        }
        Format::Bzip => {
            let mut output = bzip2::write::BzEncoder::new(
                ArchiveBuffer::default(),
                bzip2::Compression::new(level.max(1)),
            );
            copy_chunks(&tar, &mut output, io)?;
            Ok(output.finish()?.into_inner())
        }
        Format::Xz => {
            let mut output = xz2::write::XzEncoder::new(ArchiveBuffer::default(), level.min(6));
            copy_chunks(&tar, &mut output, io)?;
            Ok(output.finish()?.into_inner())
        }
        _ => Err(AppError::new("归档编码格式无效")),
    }
}
fn copy_chunks(bytes: &[u8], output: &mut impl Write, io: &ArchiveIo<'_>) -> Result<()> {
    for chunk in bytes.chunks(64 * 1024) {
        io.check()?;
        output.write_all(chunk)?;
    }
    io.check()
}
struct CheckedReader<'a, 'b> {
    cursor: Cursor<Vec<u8>>,
    io: &'a ArchiveIo<'b>,
}
impl Read for CheckedReader<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.io
            .check()
            .map_err(|error| std::io::Error::other(error.message))?;
        let count = bytes.len().min(64 * 1024);
        self.cursor.read(&mut bytes[..count])
    }
}
fn seven(
    request: &Request,
    archive: &mut ZipArchive<Cursor<Vec<u8>>>,
    listed: &[Entry],
    level: u32,
    io: &ArchiveIo<'_>,
) -> Result<Vec<u8>> {
    use sevenz_rust2::{
        encoder_options::{AesEncoderOptions, Lzma2Options},
        ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod, Password,
    };
    let error = |_| AppError::new("7Z 写入失败，请检查来源、密码设置及容量限制");
    let mut writer = ArchiveWriter::new(ArchiveBuffer::default()).map_err(error)?;
    let mut methods = Vec::new();
    if let Some(password) = request
        .password
        .as_deref()
        .filter(|password| !password.is_empty())
    {
        methods.push(AesEncoderOptions::new(Password::new(password)).into());
    }
    methods.push(if level == 0 {
        EncoderConfiguration::new(EncoderMethod::COPY)
    } else {
        let mut options = Lzma2Options::from_level(level.min(6));
        options.set_dictionary_size(8 * 1024 * 1024);
        options.into()
    });
    writer.set_content_methods(methods);
    writer.set_encrypt_header(
        request
            .password
            .as_ref()
            .is_some_and(|password| !password.is_empty()),
    );
    for (index, entry) in listed.iter().enumerate() {
        io.check()?;
        let mut metadata = if entry.directory {
            ArchiveEntry::new_directory(entry.name.trim_end_matches('/'))
        } else {
            ArchiveEntry::new_file(&entry.name)
        };
        metadata.size = entry.size;
        let content = if entry.directory {
            None
        } else {
            Some(CheckedReader {
                cursor: Cursor::new(read_entry(archive, index, None, 256 * 1024 * 1024, io)?),
                io,
            })
        };
        writer
            .push_archive_entry(metadata, content)
            .map_err(error)?;
    }
    io.check()?;
    Ok(writer
        .finish()
        .map_err(|_| AppError::new("7Z 收尾写入失败"))?
        .into_inner())
}
