//! EPUB 容器只在内存中展开；限制声明与实际大小，不写磁盘、不加载外部资源。
use crate::model::*;
use base64::Engine;
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

const FILE_LIMIT: u64 = 8 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 64 * 1024 * 1024;

/// ZIP 资源只映射到内存；移除等价的当前目录/空分段，并拒绝父目录和绝对路径。
fn resource_path(name: &str) -> Result<String> {
    if name.starts_with('/')
        || name.contains(['\\', ':'])
        || name.chars().any(char::is_control)
        || name.split('/').any(|part| part == "..")
    {
        return Err(AppError::new("EPUB 包含无法安全读取的资源路径"));
    }
    let normalized = name
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        return Err(AppError::new("EPUB 资源路径为空"));
    }
    Ok(normalized)
}

pub(crate) fn is_epub(bytes: &[u8]) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    if archive.len() > 4096 {
        return false;
    }
    let Ok(mut file) = archive.by_name("mimetype") else {
        return false;
    };
    if file.size() != 20 {
        return false;
    }
    let mut mime = [0; 20];
    file.read_exact(&mut mime).is_ok() && mime == *b"application/epub+zip"
}

/// 返回包内资源的 Base64；最多 4096 项、每项 8 MiB、总展开 64 MiB。
pub(crate) fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    if bytes.len() > 32 * 1024 * 1024 || !is_epub(bytes) {
        return Err(AppError::new(
            "不是有效 EPUB，或文件超过 32 MiB / 4096 项限制",
        ));
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| AppError::new("EPUB 容器损坏"))?;
    let mut resources = BTreeMap::new();
    let mut total = 0;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|_| AppError::new("EPUB 条目损坏或已加密"))?;
        if file.is_dir() {
            continue;
        }
        let name = resource_path(file.name())?;
        if name == "META-INF/encryption.xml" {
            return Err(AppError::new(
                "暂不支持加密或字体混淆的 EPUB，请使用外部阅读器",
            ));
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(AppError::new("EPUB 包含不支持的符号链接资源"));
        }
        total += file.size();
        if file.size() > FILE_LIMIT || total > TOTAL_LIMIT {
            return Err(AppError::new("EPUB 展开超过限制：单项 8 MiB，总计 64 MiB"));
        }
        // 即使未渲染的资源也计入总预算，避免压缩炸弹绕过筛选。
        let mut data = Vec::new();
        (&mut file).take(FILE_LIMIT + 1).read_to_end(&mut data)?;
        if data.len() as u64 != file.size() {
            return Err(AppError::new("EPUB 条目大小不符或超过限制"));
        }
        if resources
            .insert(name, base64::engine::general_purpose::STANDARD.encode(data))
            .is_some()
        {
            return Err(AppError::new("EPUB 包含重复资源路径"));
        }
    }
    Ok(resources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn book(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn epub_preview_container_and_resources_are_validated() {
        let bytes = book(&[
            ("mimetype", b"application/epub+zip"),
            ("EPUB/chapter.xhtml", b"<p>hello</p>"),
        ]);
        assert!(is_epub(&bytes));
        assert!(crate::preview::validate(&bytes).is_ok());
        assert_eq!(
            unpack(&bytes).unwrap()["EPUB/chapter.xhtml"],
            "PHA+aGVsbG88L3A+"
        );
        assert!(!is_epub(&book(&[("mimetype", b"application/zip")])));
        assert!(unpack(&bytes[..bytes.len() - 10]).is_err());
    }
    #[test]
    fn epub_preview_encrypted_unsafe_and_oversized_books_are_rejected() {
        for name in [
            "../escape",
            "/absolute",
            "EPUB/../escape",
            "EPUB\\escape",
            "META-INF/encryption.xml",
        ] {
            let bytes = book(&[("mimetype", b"application/epub+zip"), (name, b"x")]);
            assert!(unpack(&bytes).is_err(), "{name}");
        }
        let large = vec![0; FILE_LIMIT as usize + 1];
        assert!(unpack(&book(&[
            ("mimetype", b"application/epub+zip"),
            ("large", &large)
        ]))
        .is_err());
    }

    #[test]
    fn epub_preview_normalizes_current_directory_without_alias_collisions() {
        let bytes = book(&[
            ("mimetype", b"application/epub+zip"),
            ("./OEBPS/override_v1.css", b"p {}"),
            ("OEBPS/./text//chapter.xhtml", b"<p>test</p>"),
        ]);
        let resources = unpack(&bytes).unwrap();
        assert!(resources.contains_key("OEBPS/override_v1.css"));
        assert!(resources.contains_key("OEBPS/text/chapter.xhtml"));
        let collision = book(&[
            ("mimetype", b"application/epub+zip"),
            ("OEBPS/chapter.xhtml", b"first"),
            ("./OEBPS/chapter.xhtml", b"second"),
        ]);
        assert!(unpack(&collision).is_err());
        let encrypted = book(&[
            ("mimetype", b"application/epub+zip"),
            ("./META-INF/encryption.xml", b"<encryption/>"),
        ]);
        assert!(unpack(&encrypted).is_err());
        for name in [
            "../escape",
            "./../escape",
            "/root",
            "a\\b",
            "a:b",
            "./",
            "a\0b",
        ] {
            assert!(resource_path(name).is_err());
        }
    }
}
