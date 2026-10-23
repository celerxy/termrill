//! 普通大文本只读分块读取；不沿用编辑大小限制，不一次下载整个文件。
use crate::{local_files, model::*};
use base64::Engine;
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub(crate) bytes: String,
    pub(crate) encoding: Option<&'static str>,
    pub(crate) next: u64,
    pub(crate) total: u64,
    pub(crate) stamp: String,
}
pub fn read_local(path: &Path, offset: u64, expected: Option<&str>) -> Result<Chunk> {
    let stat = local_files::regular(path)?;
    let total = stat.len();
    let stamp = format!("{}:{:?}", total, stat.modified()?);
    if expected.is_some_and(|value| value != stamp) || offset > total {
        return Err(AppError::new("文件已变化或读取位置无效"));
    }
    let mut reader = std::fs::File::open(path)?;
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    reader.take(256 * 1024).read_to_end(&mut bytes)?;
    if bytes.is_empty() && offset < total {
        return Err(AppError::new("文件提前结束"));
    }
    Ok(Chunk {
        encoding: (offset == 0)
            .then(|| crate::text_encoding::detect(&bytes, bytes.len() as u64 >= total)),
        next: offset + bytes.len() as u64,
        total,
        stamp,
        bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}
