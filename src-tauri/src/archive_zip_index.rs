//! 读取有界 ZIP 尾块与中央目录；元数据由 zip 库解析，不下载整个归档。
use super::*;
use std::io::{Seek, SeekFrom};
const INDEX_LIMIT: u64 = 16 * 1024 * 1024;
pub(super) struct Index {
    central: Vec<u8>,
    count: u16,
    offset: u64,
    prefix: u64,
    tail_start: u64,
    tail: Vec<u8>,
    extended: Option<(u64, Vec<u8>)>,
    pub revision: String,
}
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("已验证字段范围"),
    )
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("已验证字段范围"),
    )
}
fn range(reader: &mut (impl Read + Seek), offset: u64, length: usize) -> Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
impl Index {
    pub fn load(reader: &mut (impl Read + Seek)) -> Result<Self> {
        let size = reader.seek(SeekFrom::End(0))?;
        if !(22..=ARCHIVE_LIMIT).contains(&size) {
            return Err(AppError::new("ZIP 大小无效或超过读取限制"));
        }
        // 一个 SFTP DATA 块内同时带回尾记录与常见小中央目录，
        // 避免先读 22 字节、再串行请求目录造成额外 RTT；最长注释仍按需补齐。
        let mut tail_start = size.saturating_sub(30_000);
        let mut tail = range(reader, tail_start, (size - tail_start) as usize)?;
        let locate = |tail: &[u8]| {
            (0..=tail.len() - 22).rev().find(|&offset| {
                tail[offset..offset + 4] == *b"PK\x05\x06"
                    && offset + 22 + usize::from(u16_at(tail, offset + 20)) == tail.len()
            })
        };
        let end = if let Some(end) = locate(&tail) {
            end
        } else {
            tail_start = size.saturating_sub(65557);
            tail = range(reader, tail_start, (size - tail_start) as usize)?;
            locate(&tail).ok_or_else(|| AppError::new("ZIP 尾记录无效"))?
        };
        let record = &tail[end..];
        if u16_at(record, 4) != 0 || u16_at(record, 6) != 0 {
            return Err(AppError::new("不支持分卷 ZIP"));
        }
        let mut count = u64::from(u16_at(record, 10));
        let mut length = u64::from(u32_at(record, 12));
        let mut offset = u64::from(u32_at(record, 16));
        let mut boundary = tail_start + end as u64;
        let mut extended_record = None;
        if count == 65535 || length == u64::from(u32::MAX) || offset == u64::from(u32::MAX) {
            let locator_start = boundary
                .checked_sub(20)
                .ok_or_else(|| AppError::new("ZIP64 定位记录缺失"))?;
            let locator = range(reader, locator_start, 20)?;
            if locator[..4] != *b"PK\x06\x07"
                || u32_at(&locator, 4) != 0
                || u32_at(&locator, 16) != 1
            {
                return Err(AppError::new("ZIP64 定位记录无效或为分卷"));
            }
            boundary = u64_at(&locator, 8);
            if boundary
                .checked_add(56)
                .is_none_or(|end| end > locator_start)
            {
                return Err(AppError::new("ZIP64 尾记录越界"));
            }
            let extended = range(reader, boundary, 56)?;
            if extended[..4] != *b"PK\x06\x06"
                || u64_at(&extended, 4) < 44
                || u32_at(&extended, 16) != 0
                || u32_at(&extended, 20) != 0
                || u64_at(&extended, 24) != u64_at(&extended, 32)
            {
                return Err(AppError::new("ZIP64 尾记录无效"));
            }
            count = u64_at(&extended, 32);
            length = u64_at(&extended, 40);
            offset = u64_at(&extended, 48);
            extended_record = Some((boundary, extended));
        } else if u16_at(record, 8) != u16_at(record, 10) {
            return Err(AppError::new("ZIP 项目计数无效"));
        }
        if count > ENTRY_LIMIT as u64
            || length > INDEX_LIMIT
            || offset.checked_add(length).is_none_or(|end| end > boundary)
        {
            return Err(AppError::new("ZIP 目录越界或超过项目/索引限制"));
        }
        let declared = offset;
        let mut central = range(reader, offset, length as usize)?;
        // 自解压 ZIP 的目录偏移可能相对于 ZIP 起点；只在原偏移不含中央头时
        // 尝试尾记录前的实际目录位置，后续仍完整校验项目边界。
        if count != 0 && !central.starts_with(b"PK\x01\x02") {
            let physical = boundary
                .checked_sub(length)
                .ok_or_else(|| AppError::new("ZIP 目录位置无效"))?;
            central = range(reader, physical, length as usize)?;
            offset = physical;
        }
        let mut digest = Sha256::new();
        digest.update(size.to_le_bytes());
        digest.update(&central);
        let revision = format!("zip-index:{:x}", digest.finalize());
        Ok(Self {
            offset,
            prefix: offset.saturating_sub(declared),
            tail_start,
            tail,
            extended: extended_record,
            central,
            count: count as u16,
            revision,
        })
    }
    /// ZIP 库重复 Seek 时直接使用已读取的原始索引，不再访问网络。
    pub fn read_at(&self, position: u64, output: &mut [u8]) -> Option<usize> {
        let segment = std::iter::once((self.offset, self.central.as_slice()))
            .chain(std::iter::once((self.tail_start, self.tail.as_slice())))
            .chain(
                self.extended
                    .as_ref()
                    .map(|(offset, bytes)| (*offset, bytes.as_slice())),
            )
            .find(|(offset, bytes)| {
                position >= *offset && position < *offset + bytes.len() as u64
            })?;
        let start = (position - segment.0) as usize;
        let count = output.len().min(segment.1.len() - start);
        output[..count].copy_from_slice(&segment.1[start..start + count]);
        Some(count)
    }
    /// 返回所选本地头位置；一次预取头部和负载，减少小文件连续往返。
    pub fn start(&self, wanted: usize) -> Result<u64> {
        let mut position = 0;
        for index in 0..usize::from(self.count) {
            if position + 46 > self.central.len() {
                return Err(AppError::new("ZIP 索引越界"));
            }
            let record = &self.central[position..];
            let name = usize::from(u16_at(record, 28));
            let extra = usize::from(u16_at(record, 30));
            let length = 46 + name + extra + usize::from(u16_at(record, 32));
            if length > record.len() {
                return Err(AppError::new("ZIP 索引项目越界"));
            }
            if index == wanted {
                let mut offset = u64::from(u32_at(record, 42));
                if offset == u64::from(u32::MAX) {
                    let extras = &record[46 + name..46 + name + extra];
                    let mut cursor = 0;
                    let mut found = false;
                    while cursor + 4 <= extras.len() {
                        let tag = u16_at(extras, cursor);
                        let count = usize::from(u16_at(extras, cursor + 2));
                        cursor += 4;
                        if cursor + count > extras.len() {
                            return Err(AppError::new("ZIP64 扩展字段越界"));
                        }
                        if tag == 1 {
                            let skip = 8 * usize::from(u32_at(record, 24) == u32::MAX)
                                + 8 * usize::from(u32_at(record, 20) == u32::MAX);
                            if count < skip + 8 {
                                return Err(AppError::new("ZIP64 条目偏移缺失"));
                            }
                            offset = u64_at(extras, cursor + skip);
                            found = true;
                            break;
                        }
                        cursor += count;
                    }
                    if !found {
                        return Err(AppError::new("ZIP64 条目偏移缺失"));
                    }
                }
                return offset
                    .checked_add(self.prefix)
                    .ok_or_else(|| AppError::new("ZIP 条目偏移溢出"));
            }
            position += length;
        }
        Err(AppError::new("ZIP 条目索引不存在"))
    }
    pub fn covers(&self, start: u64, end: u64) -> bool {
        std::iter::once((self.offset, self.central.len()))
            .chain(std::iter::once((self.tail_start, self.tail.len())))
            .chain(
                self.extended
                    .as_ref()
                    .map(|(offset, bytes)| (*offset, bytes.len())),
            )
            .any(|(offset, length)| start >= offset && end <= offset + length as u64)
    }
    pub fn memory(&self) -> usize {
        self.central.len()
            + self.tail.len()
            + self.extended.as_ref().map_or(0, |(_, bytes)| bytes.len())
    }
    pub fn entries(&self) -> Result<Vec<Entry>> {
        // 构造仅供元数据读取的内存 ZIP：共用空本地头，禁止用它解码/校验负载。
        // 中央目录名称、编码、大小、加密和类型仍由同一 zip 库解释。
        let mut central = self.central.clone();
        let mut position = 0;
        for _ in 0..self.count {
            if position + 46 > central.len() || central[position..position + 4] != *b"PK\x01\x02" {
                return Err(AppError::new("ZIP 中央目录损坏"));
            }
            if u16_at(&central, position + 34) != 0 {
                return Err(AppError::new("不支持分卷 ZIP 项目"));
            }
            let length = 46
                + usize::from(u16_at(&central, position + 28))
                + usize::from(u16_at(&central, position + 30))
                + usize::from(u16_at(&central, position + 32));
            if position + length > central.len() {
                return Err(AppError::new("ZIP 中央目录项目越界"));
            }
            central[position + 42..position + 46].copy_from_slice(&0u32.to_le_bytes());
            position += length;
        }
        if position != central.len() {
            let remaining = &central[position..];
            if remaining.len() < 6
                || remaining[..4] != *b"PK\x05\x05"
                || 6 + usize::from(u16_at(remaining, 4)) != remaining.len()
            {
                return Err(AppError::new("ZIP 中央目录长度与项目数不符"));
            }
            // 数字签名记录不是文件条目；保留在真实索引摘要中，仅从元数据副本去除。
            central.truncate(position);
        }
        let mut bytes = vec![0; 30];
        bytes[..4].copy_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&central);
        bytes.extend_from_slice(b"PK\x05\x06");
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&self.count.to_le_bytes());
        bytes.extend_from_slice(&self.count.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&30u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 2]);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
        entries(&mut archive)
    }
}
