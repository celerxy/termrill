//! 独立 PTY 会话的输入、输出与尺寸调整；停止信号由会话所有者发出。

use crate::model::*;
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::Emitter;

pub enum Input {
    Data(String),
    Resize(u32, u32),
}
pub struct TerminalControl {
    pub connection_id: String,
    pub input: tokio::sync::mpsc::Sender<Input>,
    pub stop: Arc<AtomicBool>,
    pub acknowledged: Arc<AtomicUsize>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Output {
    id: String,
    data: Vec<u8>,
    closed: bool,
    error: Option<String>,
}

/// SSH 通道上窗口更新、欢迎内容与请求确认可交错，只有明确失败或关闭才能判为失败。
pub(crate) async fn wait_request(
    channel: &mut russh::Channel<russh::client::Msg>,
    initial: &mut Vec<u8>,
    phase: &str,
) -> Result<()> {
    loop {
        if setup_message(channel.wait().await, initial, phase)? {
            return Ok(());
        }
    }
}

fn setup_message(
    message: Option<russh::ChannelMsg>,
    initial: &mut Vec<u8>,
    phase: &str,
) -> Result<bool> {
    use russh::ChannelMsg;
    match message {
        Some(ChannelMsg::Success) => Ok(true),
        Some(ChannelMsg::Failure | ChannelMsg::OpenFailure(_)) => {
            Err(AppError::new(format!("服务器拒绝 {phase} 请求")))
        }
        Some(
            ChannelMsg::Eof
            | ChannelMsg::Close
            | ChannelMsg::ExitStatus { .. }
            | ChannelMsg::ExitSignal { .. },
        )
        | None => Err(AppError::new(format!(
            "服务器在确认 {phase} 请求前关闭终端通道"
        ))),
        Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
            if data.len() > (1024 * 1024usize).saturating_sub(initial.len()) {
                return Err(AppError::new("终端启动输出超过 1 MiB 限制"));
            }
            initial.extend_from_slice(&data);
            Ok(false)
        }
        // WindowAdjusted 已由 russh 更新发送窗口；它不是 PTY/Shell 的应答。
        _ => Ok(false),
    }
}

/// 终端只拥有独立通道，关闭终端不关闭共用的 SSH 传输。
#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: tauri::AppHandle,
    backend: Arc<Backend>,
    id: String,
    mut channel: russh::Channel<russh::client::Msg>,
    initial: Vec<u8>,
    mut input: tokio::sync::mpsc::Receiver<Input>,
    stop: Arc<AtomicBool>,
    parent_closed: Arc<AtomicBool>,
    acknowledged: Arc<AtomicUsize>,
) {
    let mut pending_bytes = initial.len();
    if !initial.is_empty() {
        let _ = app.emit(
            "terminal-output",
            Output {
                id: id.clone(),
                data: initial,
                closed: false,
                error: None,
            },
        );
    }
    let mut error = None;
    loop {
        if stop.load(Ordering::Relaxed) || parent_closed.load(Ordering::Relaxed) {
            break;
        }
        pending_bytes = pending_bytes.saturating_sub(acknowledged.swap(0, Ordering::Relaxed));
        let outcome: Result<bool> = tokio::select! {
            message = input.recv() => {
                match message {
                    Some(Input::Data(data)) => send_input(&channel, data, &stop, &parent_closed).await.map(|_| true),
                    Some(Input::Resize(cols,rows)) => channel.window_change(cols,rows,0,0).await.map(|_| true).map_err(Into::into),
                    None => Ok(false),
                }
            }
            message = channel.wait(), if pending_bytes < 256 * 1024 => {
                match message {
                    Some(russh::ChannelMsg::Data { data }) | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                        pending_bytes += data.len();
                        app.emit("terminal-output", Output { id: id.clone(), data: data.to_vec(), closed: false, error: None }).map(|_| true).map_err(|error| AppError::new(format!("终端输出投递失败：{error}")))
                    }
                    Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => Ok(false),
                    _ => Ok(true),
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(20)) => Ok(true),
        };
        match outcome {
            Ok(true) => {}
            Ok(false) => break,
            Err(value) => {
                error = Some(value.message);
                break;
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), channel.close()).await;
    let _ = app.emit(
        "terminal-output",
        Output {
            id: id.clone(),
            data: vec![],
            closed: true,
            error,
        },
    );
    if let Ok(mut terminals) = backend.terminals.lock() {
        terminals.remove(&id);
    }
}

// 通道发送受远端窗口约束，等待期间仍响应终端关闭和主连接断开。
async fn send_input(
    channel: &russh::Channel<russh::client::Msg>,
    data: String,
    stop: &AtomicBool,
    closed: &AtomicBool,
) -> Result<()> {
    let send = channel.data_bytes(data.into_bytes());
    tokio::pin!(send);
    loop {
        if stop.load(Ordering::Relaxed) || closed.load(Ordering::Relaxed) {
            return Err(AppError::new("终端已关闭"));
        }
        tokio::select! { result = &mut send => return result.map_err(Into::into), _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
    }
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    use russh::ChannelMsg;
    #[test]
    fn informational_messages_and_banner_do_not_reject_requests() {
        let mut initial = Vec::new();
        assert!(!setup_message(
            Some(ChannelMsg::WindowAdjusted { new_size: 4096 }),
            &mut initial,
            "PTY"
        )
        .unwrap());
        assert!(!setup_message(
            Some(ChannelMsg::Data {
                data: b"welcome".to_vec().into()
            }),
            &mut initial,
            "PTY"
        )
        .unwrap());
        assert!(setup_message(Some(ChannelMsg::Success), &mut initial, "PTY").unwrap());
        assert!(!setup_message(
            Some(ChannelMsg::WindowAdjusted { new_size: 8192 }),
            &mut initial,
            "Shell"
        )
        .unwrap());
        assert!(setup_message(Some(ChannelMsg::Success), &mut initial, "Shell").unwrap());
        assert_eq!(initial, b"welcome");
    }
    #[test]
    fn rejection_closure_and_output_limit_remain_errors() {
        let mut initial = Vec::new();
        assert!(
            setup_message(Some(ChannelMsg::Failure), &mut initial, "PTY")
                .unwrap_err()
                .message
                .contains("拒绝 PTY")
        );
        assert!(setup_message(Some(ChannelMsg::Eof), &mut initial, "Shell")
            .unwrap_err()
            .message
            .contains("关闭终端通道"));
        initial.resize(1024 * 1024, 0);
        assert!(setup_message(
            Some(ChannelMsg::Data {
                data: vec![1].into()
            }),
            &mut initial,
            "Shell"
        )
        .is_err());
        assert_eq!(initial.len(), 1024 * 1024);
    }
}
