//! 删除专用标准 SFTP v3 请求流水线；窗口内的独立目录与文件请求可乱序确认。
//! 仅探测失败可回退；开始遍历后任何失败都停止派发，不重放可能成功的删除。
use super::*;
use crate::sftp_packet::{number, push_string, read_packet, session_step, string, write_packet};
use ssh2::{Channel, Session};
use std::collections::VecDeque;
const WINDOW: usize = 16;
const HANDLES: usize = 16;

#[derive(Debug)]
enum Operation {
    Stat(String, usize),
    Open(String, usize),
    Read(String, usize, Vec<u8>),
    Close(String, usize, Vec<u8>),
    Remove(String),
}
impl Operation {
    fn packet(&self, id: u32) -> Result<Vec<u8>> {
        let (kind, value) = match self {
            Self::Stat(path, _) => (7, path.as_bytes()),
            Self::Open(path, _) => (11, path.as_bytes()),
            Self::Read(_, _, handle) => (12, handle.as_slice()),
            Self::Close(_, _, handle) => (4, handle.as_slice()),
            Self::Remove(path) => (13, path.as_bytes()),
        };
        let mut packet = vec![kind];
        packet.extend_from_slice(&id.to_be_bytes());
        push_string(&mut packet, value)?;
        Ok(packet)
    }
}

pub(super) struct Client<'a> {
    session: &'a Session,
    closed: &'a AtomicBool,
    channel: Channel,
}
impl<'a> Client<'a> {
    pub fn prepare(session: &'a Session, closed: &'a AtomicBool) -> Result<Self> {
        let _mode = crate::transfer_io::Nonblocking::new(session);
        let mut channel = session_step(session, closed, "delete_open_channel", || {
            session.channel_session()
        })?;
        session_step(session, closed, "delete_start_sftp", || {
            channel.subsystem("sftp")
        })?;
        write_packet(session, closed, &mut channel, &[1, 0, 0, 0, 3])?;
        let packet = read_packet(session, closed, &mut channel)?;
        let mut data = packet.as_slice();
        if data.first() != Some(&2) {
            return Err(AppError::new("SFTP 版本响应无效"));
        }
        data = &data[1..];
        if number(&mut data)? != 3 {
            return Err(AppError::new("删除流水线需要 SFTP v3"));
        }
        while !data.is_empty() {
            string(&mut data)?;
            string(&mut data)?;
        }
        Ok(Self {
            session,
            closed,
            channel,
        })
    }
    pub fn run(&mut self, entries: Vec<Entry>) -> Result<BTreeMap<usize, Vec<String>>> {
        // 会话锁由调用者持有，退出时恢复模式；此窗口只有本线程读写专用通道。
        let _mode = crate::transfer_io::Nonblocking::new(self.session);
        let mut state = State::new(entries)?;
        let started = Instant::now();
        let mut next_id = 0u32;
        while !state.queued.is_empty() || !state.pending.is_empty() {
            if self.closed.load(Ordering::Relaxed) {
                return Err(AppError::new("连接已关闭，部分项目可能已删除"));
            }
            state.fill(|operation| {
                next_id = next_id
                    .checked_add(1)
                    .ok_or_else(|| AppError::new("删除请求数量超限"))?;
                write_packet(
                    self.session,
                    self.closed,
                    &mut self.channel,
                    &operation.packet(next_id)?,
                )?;
                Ok(next_id)
            })?;
            if state.pending.is_empty() {
                return Err(AppError::new("删除请求队列无可执行项目"));
            }
            let response = read_packet(self.session, self.closed, &mut self.channel)?;
            state.response(&response)?;
        }
        crate::performance_log!("[TermRill delete] request_pipeline=true window={} requests={} items={} directories={} elapsed_ms={}", WINDOW, next_id, state.count, state.directories.values().map(Vec::len).sum::<usize>(), started.elapsed().as_millis());
        Ok(state.directories)
    }
}

struct State {
    queued: VecDeque<Operation>,
    pending: BTreeMap<u32, Operation>,
    directories: BTreeMap<usize, Vec<String>>,
    handles: usize,
    count: usize,
    bytes: usize,
}
impl State {
    fn new(entries: Vec<Entry>) -> Result<Self> {
        let mut state = Self {
            queued: VecDeque::new(),
            pending: BTreeMap::new(),
            directories: BTreeMap::new(),
            handles: 0,
            count: 0,
            bytes: 0,
        };
        for (path, stat) in entries {
            state.entry(path, stat.perm, 1)?;
        }
        Ok(state)
    }
    fn entry(&mut self, path: String, permissions: Option<u32>, depth: usize) -> Result<()> {
        if depth > 128 || self.count >= 100_000 || self.bytes + path.len() > 32 * 1024 * 1024 {
            return Err(AppError::new("目录遍历超限，部分项目可能已删除"));
        }
        self.count += 1;
        self.bytes += path.len();
        let directory = permissions.is_none_or(|value| value & 0o170000 == 0o040000);
        self.queued.push_back(if directory {
            Operation::Stat(path, depth)
        } else {
            Operation::Remove(path)
        });
        Ok(())
    }
    fn fill(&mut self, mut send: impl FnMut(&Operation) -> Result<u32>) -> Result<()> {
        // 已打开目录的 READ/CLOSE 排在前面；限制句柄数，避免服务端资源无界增长。
        let mut deferred = 0;
        while self.pending.len() < WINDOW && !self.queued.is_empty() {
            let Some(operation) = self.queued.pop_front() else {
                break;
            };
            if matches!(operation, Operation::Open(..)) && self.handles >= HANDLES {
                self.queued.push_back(operation);
                deferred += 1;
                if deferred >= self.queued.len() {
                    break;
                }
                continue;
            }
            let id = send(&operation)?;
            if matches!(operation, Operation::Open(..)) {
                self.handles += 1;
            }
            self.pending.insert(id, operation);
            deferred = 0;
        }
        Ok(())
    }
    fn response(&mut self, packet: &[u8]) -> Result<()> {
        let (&kind, mut data) = packet
            .split_first()
            .ok_or_else(|| AppError::new("SFTP 响应为空"))?;
        let id = number(&mut data)?;
        let operation = self
            .pending
            .remove(&id)
            .ok_or_else(|| AppError::new("SFTP 响应编号未知或重复"))?;
        if kind == 101 {
            let status = number(&mut data)?;
            string(&mut data)?;
            string(&mut data)?;
            if !data.is_empty() {
                return Err(AppError::new("SFTP 状态响应含多余内容"));
            }
            return match (operation, status) {
                (Operation::Read(path, depth, handle), 1) => {
                    self.queued
                        .push_front(Operation::Close(path, depth, handle));
                    Ok(())
                }
                (Operation::Close(path, depth, _), 0) => {
                    self.handles -= 1;
                    self.directories.entry(depth).or_default().push(path);
                    Ok(())
                }
                (Operation::Remove(_), 0) => Ok(()),
                (_, status) => Err(AppError::new(format!(
                    "SFTP 删除请求失败（状态 {status}），部分项目可能已删除"
                ))),
            };
        }
        match (operation, kind) {
            (Operation::Stat(path, depth), 105) => {
                let permissions = attributes(&mut data)?;
                if !data.is_empty() {
                    return Err(AppError::new("SFTP 属性响应含多余内容"));
                }
                let permissions =
                    permissions.ok_or_else(|| AppError::new("目录属性缺少类型，删除已停止"))?;
                self.queued
                    .push_front(if permissions & 0o170000 == 0o040000 {
                        Operation::Open(path, depth)
                    } else {
                        Operation::Remove(path)
                    });
            }
            (Operation::Open(path, depth), 102) => {
                let handle = string(&mut data)?.to_vec();
                if handle.is_empty() || handle.len() > 256 || !data.is_empty() {
                    return Err(AppError::new("SFTP 目录句柄无效"));
                }
                self.queued.push_front(Operation::Read(path, depth, handle));
            }
            (Operation::Read(path, depth, handle), 104) => {
                let count = number(&mut data)?;
                if count == 0 || count > 100_000 {
                    return Err(AppError::new("SFTP 目录批次数量无效"));
                }
                for _ in 0..count {
                    let name = string(&mut data)?;
                    string(&mut data)?; // longname 不用于路径或类型判断。
                    let permissions = attributes(&mut data)?;
                    if name == b"." || name == b".." {
                        continue;
                    }
                    let name = safe_name(name)?;
                    self.entry(format!("{path}/{name}"), permissions, depth + 1)?;
                }
                if !data.is_empty() {
                    return Err(AppError::new("SFTP 目录响应含多余内容"));
                }
                self.queued.push_front(Operation::Read(path, depth, handle));
            }
            _ => return Err(AppError::new("SFTP 删除响应类型无效，部分项目可能已删除")),
        }
        Ok(())
    }
}
fn safe_name(name: &[u8]) -> Result<&str> {
    let name = std::str::from_utf8(name).map_err(|_| AppError::new("服务器返回无效名称"))?;
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(AppError::new("服务器返回无效名称，删除已停止"));
    }
    Ok(name)
}
fn attributes(data: &mut &[u8]) -> Result<Option<u32>> {
    let flags = number(data)?;
    if flags & !0x8000000f != 0 {
        return Err(AppError::new("SFTP 属性标志无效"));
    }
    if flags & 1 != 0 {
        number(data)?;
        number(data)?;
    }
    if flags & 2 != 0 {
        number(data)?;
        number(data)?;
    }
    let permissions = if flags & 4 != 0 {
        Some(number(data)?)
    } else {
        None
    };
    if flags & 8 != 0 {
        number(data)?;
        number(data)?;
    }
    if flags & 0x80000000 != 0 {
        let count = number(data)?;
        if count as usize > data.len() / 8 {
            return Err(AppError::new("SFTP 扩展属性数量无效"));
        }
        for _ in 0..count {
            string(data)?;
            string(data)?;
        }
    }
    Ok(permissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status(id: u32, code: u32) -> Vec<u8> {
        let mut value = vec![101];
        value.extend_from_slice(&id.to_be_bytes());
        value.extend_from_slice(&code.to_be_bytes());
        push_string(&mut value, b"").unwrap();
        push_string(&mut value, b"").unwrap();
        value
    }
    #[test]
    fn reordered_confirmations_never_complete_parent_before_close() {
        let mut state = State::new(vec![]).unwrap();
        state.handles = 1;
        state
            .pending
            .insert(1, Operation::Remove("/root/file".into()));
        state.pending.insert(
            2,
            Operation::Read("/root/dir".into(), 1, b"handle".to_vec()),
        );
        state.response(&status(2, 1)).unwrap();
        assert!(state.directories.is_empty());
        assert!(matches!(state.queued.front(), Some(Operation::Close(..))));
        state.fill(|_| Ok(3)).unwrap();
        state.response(&status(1, 0)).unwrap();
        assert!(state.directories.is_empty());
        state.response(&status(3, 0)).unwrap();
        assert_eq!(state.directories[&1], vec!["/root/dir"]);
        assert_eq!(state.handles, 0);
        assert!(state.response(&status(3, 0)).is_err());
    }
    #[test]
    fn unsafe_names_and_failed_removals_are_rejected() {
        for name in [
            b"../outside".as_slice(),
            b"a/b",
            b"a\\b",
            b"a\0b",
            b"",
            b"..",
        ] {
            assert!(safe_name(name).is_err());
        }
        let mut state = State::new(vec![]).unwrap();
        state
            .pending
            .insert(1, Operation::Remove("/root/file".into()));
        assert!(state.response(&status(1, 4)).is_err());
        assert!(state.pending.is_empty());
    }
    #[test]
    fn request_window_and_directory_handles_are_bounded() {
        let mut state = State::new(vec![]).unwrap();
        for i in 0..40 {
            state
                .queued
                .push_back(Operation::Open(format!("/root/{i}"), 1));
        }
        let mut id = 0;
        state
            .fill(|_| {
                id += 1;
                Ok(id)
            })
            .unwrap();
        assert_eq!(state.pending.len(), WINDOW);
        assert_eq!(state.handles, HANDLES);
        state.pending.clear();
        state.fill(|_| panic!("句柄耗尽时不能继续 OPEN")).unwrap();
        assert_eq!(state.queued.len(), 24);
    }
}
