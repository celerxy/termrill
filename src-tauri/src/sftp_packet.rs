//! 标准 SFTP v3 有界包读写；只使用公开 SSH Channel，支持短读写和断开取消。
use crate::model::*;
#[cfg(test)]
use ssh2::{Channel, Session};
#[cfg(test)]
use std::{
    io::{Read, Write},
    sync::atomic::AtomicBool,
};
const MAX_PACKET: usize = 256 * 1024;
#[cfg(test)]
pub(crate) fn number(data: &mut &[u8]) -> Result<u32> {
    let bytes = data
        .get(..4)
        .ok_or_else(|| AppError::new("SFTP 响应被截断"))?;
    let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    *data = &data[4..];
    Ok(value)
}
#[cfg(test)]
pub(crate) fn string<'a>(data: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = number(data)? as usize;
    let value = data
        .get(..len)
        .ok_or_else(|| AppError::new("SFTP 字符串响应被截断"))?;
    *data = &data[len..];
    Ok(value)
}
pub(crate) fn push_string(packet: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    if value.len() > MAX_PACKET || packet.len() + 4 + value.len() > MAX_PACKET {
        return Err(AppError::new("SFTP 扩展请求过长"));
    }
    packet.extend_from_slice(&(value.len() as u32).to_be_bytes());
    packet.extend_from_slice(value);
    Ok(())
}
#[cfg(test)]
pub(crate) fn write_packet(
    session: &Session,
    closed: &AtomicBool,
    channel: &mut Channel,
    packet: &[u8],
) -> Result<()> {
    let mut framed = Vec::with_capacity(4 + packet.len());
    framed.extend_from_slice(&(packet.len() as u32).to_be_bytes());
    framed.extend_from_slice(packet);
    let mut offset = 0;
    while offset < framed.len() {
        match channel.write(&framed[offset..]) {
            Ok(0) => wait(session, closed, "atomic_write")?,
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait(session, closed, "atomic_write")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn read_packet(
    session: &Session,
    closed: &AtomicBool,
    channel: &mut Channel,
) -> Result<Vec<u8>> {
    let mut length = [0; 4];
    read_exact(session, closed, channel, &mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_PACKET {
        return Err(AppError::new("SFTP 扩展响应长度无效"));
    }
    let mut packet = vec![0; length];
    read_exact(session, closed, channel, &mut packet)?;
    Ok(packet)
}

#[cfg(test)]
fn read_exact(
    session: &Session,
    closed: &AtomicBool,
    channel: &mut Channel,
    buffer: &mut [u8],
) -> Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        match channel.read(&mut buffer[offset..]) {
            Ok(0) => return Err(AppError::new("SFTP 通道意外结束")),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait(session, closed, "atomic_read")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn session_step<T>(
    session: &Session,
    closed: &AtomicBool,
    phase: &str,
    mut operation: impl FnMut() -> std::result::Result<T, ssh2::Error>,
) -> Result<T> {
    loop {
        if closed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(AppError::new(format!("连接已关闭，{phase}已停止")));
        }
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if error.code() == ssh2::ErrorCode::Session(-37) => {
                wait(session, closed, phase)?
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
fn wait(session: &Session, closed: &AtomicBool, phase: &str) -> Result<()> {
    if closed.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(AppError::new(format!("连接已关闭，{phase}已停止")));
    }
    crate::transfer_io::wait_network_ready(session).map(|_| ())
}
