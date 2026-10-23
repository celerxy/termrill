//! 性能日志通过有界队列交给独立线程；网络协程不等待终端或磁盘。
//! 只写阶段、数量和时间；固定大小轮转，不记录路径、凭据或内容。
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

static SENDER: OnceLock<SyncSender<String>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const QUEUE_CAPACITY: usize = 1024;

pub fn initialize(directory: PathBuf) -> std::io::Result<()> {
    if SENDER.get().is_some() {
        return Ok(());
    }
    fs::create_dir_all(&directory)?;
    let path = directory.join("performance.log");
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    std::thread::Builder::new()
        .name("termrill-performance-log".into())
        .spawn(move || write_records(path, file, receiver))?;
    let _ = SENDER.set(sender);
    record(format_args!("[TermRill performance] session_started=true"));
    Ok(())
}

fn write_records(path: PathBuf, file: File, receiver: Receiver<String>) {
    let mut length = file.metadata().map_or(0, |metadata| metadata.len());
    let mut file = Some(file);
    for record in receiver {
        let lost = DROPPED.swap(0, Ordering::Relaxed);
        let result = (|| -> std::io::Result<()> {
            if length >= MAX_BYTES {
                // Windows 轮转前必须释放打开的文件，随后重新打开追加句柄。
                drop(file.take());
                let previous = path.with_file_name("performance.previous.log");
                if previous.exists() {
                    fs::remove_file(&previous)?;
                }
                fs::rename(&path, previous)?;
                file = Some(OpenOptions::new().create(true).append(true).open(&path)?);
                length = 0;
            }
            let file = file
                .as_mut()
                .ok_or_else(|| std::io::Error::other("日志文件未打开"))?;
            if lost > 0 {
                let summary = format!("[TermRill performance] dropped_records={lost}\n");
                file.write_all(summary.as_bytes())?;
                length += summary.len() as u64;
            }
            file.write_all(record.as_bytes())?;
            length += record.len() as u64;
            Ok(())
        })();
        if result.is_err() {
            // 日志失效只停止观察线程，不向网络协程传播失败或阻塞。
            eprintln!("[TermRill performance] log_write_failed=true");
            break;
        }
        // 控制台写入也只发生在日志线程，慢终端只会丢弃诊断记录。
        eprint!("{record}");
    }
}

pub fn record(message: std::fmt::Arguments<'_>) {
    let Some(sender) = SENDER.get() else {
        return;
    };
    // 时间在调用处采集，不能以日志线程实际写盘时间替代事件时间。
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let record = format!("unix_ms={timestamp} {message}\n");
    if sender.try_send(record).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

#[macro_export]
macro_rules! performance_log {
    ($($argument:tt)*) => { $crate::performance::record(format_args!($($argument)*)) };
}

/// 累计进程 CPU 微秒，用于区分应用计算繁忙与响应等待；包含所有应用线程。
#[cfg(windows)]
pub fn cpu_micros() -> Option<u64> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // 当前进程伪句柄有效，四个输出均为独立、已初始化的 FILETIME 存储。
    let success = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if success == 0 {
        return None;
    }
    let ticks =
        |value: FILETIME| (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    Some(ticks(kernel).saturating_add(ticks(user)) / 10)
}
#[cfg(not(windows))]
pub fn cpu_micros() -> Option<u64> {
    None
}
