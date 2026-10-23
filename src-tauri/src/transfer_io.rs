//! 数据阶段的非阻塞读写：按实际返回字节判断进展，暂停不计入停滞，短写保持原缓冲。
use crate::model::*;
use std::{
    io::{Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const IDLE: Duration = Duration::from_secs(300);
/// 等待专用传输连接就绪，数据到达立即唤醒；最多等待 50 ms 后重新检查取消。
pub(crate) fn wait_network(session: &ssh2::Session) -> Result<()> {
    wait_network_ready(session).map(|_| ())
}

/// 只报告 socket 是否实际就绪，不读取/记录 SSH 包和文件内容。
/// 用于区分客户端持续处理数据与网络长时间没有响应。
pub(crate) fn wait_network_ready(session: &ssh2::Session) -> Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            WSAGetLastError, WSAPoll, POLLRDNORM, POLLWRNORM, WSAPOLLFD,
        };
        let events = match session.block_directions() {
            ssh2::BlockDirections::Inbound => POLLRDNORM,
            ssh2::BlockDirections::Outbound => POLLWRNORM,
            ssh2::BlockDirections::Both => POLLRDNORM | POLLWRNORM,
            ssh2::BlockDirections::None => {
                std::thread::sleep(Duration::from_millis(1));
                return Ok(false);
            }
        };
        let mut socket = WSAPOLLFD {
            fd: session.as_raw_socket() as _,
            events,
            revents: 0,
        };
        // Session 在整个调用期间持有 socket；数组长度为 1，不跨线程关闭此句柄。
        let result = unsafe { WSAPoll(&mut socket, 1, 50) };
        if result < 0 {
            return Err(std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }).into());
        }
        Ok(result > 0)
    }
    #[cfg(not(windows))]
    {
        let _ = session;
        std::thread::sleep(Duration::from_millis(1));
        Ok(false)
    }
}
pub struct Nonblocking<'a>(&'a ssh2::Session, bool);
impl<'a> Nonblocking<'a> {
    pub fn new(session: &'a ssh2::Session) -> Self {
        let previous = session.is_blocking();
        session.set_blocking(false);
        Self(session, previous)
    }
}
impl Drop for Nonblocking<'_> {
    fn drop(&mut self) {
        self.0.set_blocking(self.1);
    }
}

fn ready(
    last: &mut Instant,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    idle: Duration,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) || closed.load(Ordering::Relaxed) {
        return Err(AppError::new("传输已停止"));
    }
    while pause.load(Ordering::Relaxed) {
        if cancel.load(Ordering::Relaxed) || closed.load(Ordering::Relaxed) {
            return Err(AppError::new("传输已停止"));
        }
        std::thread::sleep(Duration::from_millis(20));
        *last = Instant::now();
    }
    if last.elapsed() >= idle {
        return Err(AppError::new(
            "数据读写连续 300 秒没有进展，临时文件保留供续传",
        ));
    }
    Ok(())
}
#[cfg(test)]
pub fn read(
    source: &mut impl Read,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
) -> Result<usize> {
    read_wait(source, buffer, cancel, closed, pause, || {
        std::thread::sleep(Duration::from_millis(10));
        Ok(())
    })
}

pub fn read_network(
    session: &ssh2::Session,
    source: &mut impl Read,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
) -> Result<usize> {
    read_wait(source, buffer, cancel, closed, pause, || {
        wait_network(session)
    })
}

fn read_wait(
    source: &mut impl Read,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    wait: impl Fn() -> Result<()>,
) -> Result<usize> {
    let mut last = Instant::now();
    loop {
        ready(&mut last, cancel, closed, pause, IDLE)?;
        match source.read(buffer) {
            Ok(count) => return Ok(count),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => wait()?,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}
#[cfg(test)]
pub fn write(
    dest: &mut impl Write,
    buffer: &[u8],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    progress: impl FnMut(usize),
) -> Result<()> {
    write_with_budget(dest, buffer, cancel, closed, pause, IDLE, progress, None)
}

pub fn write_network(
    session: &ssh2::Session,
    dest: &mut impl Write,
    buffer: &[u8],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    progress: impl FnMut(usize),
) -> Result<()> {
    write_with_budget(
        dest,
        buffer,
        cancel,
        closed,
        pause,
        IDLE,
        progress,
        Some(session),
    )
}

#[allow(clippy::too_many_arguments)]
fn write_with_budget(
    dest: &mut impl Write,
    buffer: &[u8],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    idle: Duration,
    mut progress: impl FnMut(usize),
    session: Option<&ssh2::Session>,
) -> Result<()> {
    let mut last = Instant::now();
    let mut offset = 0;
    let no_pause = AtomicBool::new(false);
    while offset < buffer.len() {
        // 先收完已预发送缓冲的确认，再由外层停止发送新缓冲；不能在暂停确认后继续写盘。
        if pause.load(Ordering::Relaxed) {
            last = Instant::now();
        }
        ready(&mut last, cancel, closed, &no_pause, idle)?;
        // libssh2 可能已缓存短写后的剩余内容；完成前不能替换或修改原缓冲。
        match dest.write(&buffer[offset..]) {
            Ok(0) => match session {
                Some(session) => wait_network(session)?,
                None => std::thread::sleep(Duration::from_millis(10)),
            },
            Ok(count) => {
                offset += count;
                last = Instant::now();
                progress(count);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => match session {
                Some(session) => wait_network(session)?,
                None => std::thread::sleep(Duration::from_millis(10)),
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_budget_is_reset_by_pause_and_cancellation_stops_wait() {
        let cancel = AtomicBool::new(false);
        let closed = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        let mut last = Instant::now() - IDLE - Duration::from_secs(1);
        assert!(ready(&mut last, &cancel, &closed, &pause, IDLE).is_err());
        let mut last = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        assert!(ready(&mut last, &cancel, &closed, &pause, IDLE).is_err());
    }
    #[test]
    fn slow_continuous_writes_can_exceed_idle_budget() {
        struct Slow;
        impl Write for Slow {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_millis(10));
                Ok(data.len().min(1))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let flag = AtomicBool::new(false);
        let start = Instant::now();
        write_with_budget(
            &mut Slow,
            &[1; 20],
            &flag,
            &flag,
            &flag,
            Duration::from_millis(80),
            |_| {},
            None,
        )
        .unwrap();
        assert!(start.elapsed() > Duration::from_millis(80));
    }
    #[test]
    fn stalled_write_times_out_and_pause_does_not() {
        struct Stalled;
        impl Write for Stalled {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let flag = AtomicBool::new(false);
        assert!(write_with_budget(
            &mut Stalled,
            &[1],
            &flag,
            &flag,
            &flag,
            Duration::from_millis(40),
            |_| {},
            None,
        )
        .is_err());
        let pause = AtomicBool::new(true);
        let mut last = Instant::now() - Duration::from_secs(1);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                pause.store(false, Ordering::Relaxed);
            });
            ready(&mut last, &flag, &flag, &pause, Duration::from_millis(40)).unwrap();
        });
    }
    #[test]
    fn short_writes_and_wouldblock_keep_every_byte() {
        struct Sink {
            bytes: Vec<u8>,
            calls: usize,
        }
        impl Write for Sink {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                if self.calls.is_multiple_of(3) {
                    return Err(std::io::ErrorKind::WouldBlock.into());
                }
                let size = data.len().min(7);
                self.bytes.extend_from_slice(&data[..size]);
                Ok(size)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let flag = AtomicBool::new(false);
        let data = (0..100).collect::<Vec<u8>>();
        let mut dest = Sink {
            bytes: Vec::new(),
            calls: 0,
        };
        write(&mut dest, &data, &flag, &flag, &flag, |_| {}).unwrap();
        assert_eq!(dest.bytes, data);
    }
}
