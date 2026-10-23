//! keyboard-interactive 提示通过临时通道交给界面；回答只保留在本轮认证内存。
use crate::model::*;
#[cfg(test)]
use ssh2::{KeyboardInteractivePrompt, Prompt};
use tauri::Emitter;

#[derive(serde::Serialize, Clone)]
pub struct Challenge {
    id: String,
    address: String,
    instructions: String,
    prompts: Vec<Question>,
}
#[derive(serde::Serialize, Clone)]
struct Question {
    text: String,
    echo: bool,
}

/// 异步 SSH 认证等待界面回答；取消 future 时也移除回答入口和关闭提示。
pub(crate) async fn answer_async(
    backend: &Backend,
    endpoint: &Endpoint,
    instructions: &str,
    prompts: Vec<(String, bool)>,
) -> Result<Vec<String>> {
    if prompts.len() > 16 {
        return Err(AppError::new("认证提示数量超出限制"));
    }
    let app = backend
        .auth_app
        .as_ref()
        .ok_or_else(|| AppError::new("当前环境不支持交互式认证"))?;
    let id = identifier();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    lock(&backend.auth_answers)?.insert(id.clone(), sender);
    struct Cleanup<'a> {
        backend: &'a Backend,
        id: String,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            if let Ok(mut answers) = self.backend.auth_answers.lock() {
                answers.remove(&self.id);
            }
            if let Some(app) = &self.backend.auth_app {
                let _ = app.emit("auth-challenge-closed", &self.id);
            }
        }
    }
    let _cleanup = Cleanup {
        backend,
        id: id.clone(),
    };
    let count = prompts.len();
    app.emit(
        "auth-challenge",
        Challenge {
            id,
            address: format!("{}@{}:{}", endpoint.username, endpoint.host, endpoint.port),
            instructions: instructions.into(),
            prompts: prompts
                .into_iter()
                .map(|(text, echo)| Question { text, echo })
                .collect(),
        },
    )
    .map_err(|_| AppError::new("无法显示认证提示"))?;
    let start = std::time::Instant::now();
    loop {
        match receiver.try_recv() {
            Ok(Some(answer)) if answer.len() == count => return Ok(answer),
            Ok(Some(_)) => return Err(AppError::new("认证回答数量不匹配")),
            Ok(None) => return Err(AppError::new("用户取消了交互式认证")),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(AppError::new("认证回答入口已关闭"))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if start.elapsed().as_secs() >= 120 {
            return Err(AppError::new("认证提示已超时或应用已关闭"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
pub struct Interactive<'a> {
    pub backend: &'a Backend,
    pub endpoint: &'a Endpoint,
    pub error: Option<String>,
    pub rounds: usize,
}
#[cfg(test)]
impl KeyboardInteractivePrompt for Interactive<'_> {
    fn prompt(&mut self, _: &str, instructions: &str, prompts: &[Prompt<'_>]) -> Vec<String> {
        if prompts.is_empty() {
            return Vec::new();
        }
        if self.error.is_some() || self.rounds >= 16 || prompts.len() > 16 {
            self.error = Some("认证挑战次数或提示数量超出限制".into());
            return vec![String::new(); prompts.len()];
        }
        self.rounds += 1;
        let result = (|| -> Result<Vec<String>> {
            let app = self
                .backend
                .auth_app
                .as_ref()
                .ok_or_else(|| AppError::new("当前环境不支持交互式认证"))?;
            let id = identifier();
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            lock(&self.backend.auth_answers)?.insert(id.clone(), sender);
            let challenge = Challenge {
                id: id.clone(),
                address: format!(
                    "{}@{}:{}",
                    self.endpoint.username, self.endpoint.host, self.endpoint.port
                ),
                instructions: instructions.into(),
                prompts: prompts
                    .iter()
                    .map(|p| Question {
                        text: p.text.to_string(),
                        echo: p.echo,
                    })
                    .collect(),
            };
            let result = (|| {
                app.emit("auth-challenge", challenge)
                    .map_err(|_| AppError::new("无法显示认证提示"))?;
                let answer = receiver
                    .recv_timeout(std::time::Duration::from_secs(120))
                    .map_err(|_| AppError::new("认证提示已超时或应用已关闭"))?;
                let answer = answer.ok_or_else(|| AppError::new("用户取消了交互式认证"))?;
                if answer.len() != prompts.len() {
                    return Err(AppError::new("认证回答数量不匹配"));
                }
                Ok(answer)
            })();
            lock(&self.backend.auth_answers)?.remove(&id);
            let _ = app.emit("auth-challenge-closed", id);
            result
        })();
        match result {
            Ok(answers) => answers,
            Err(error) => {
                self.error = Some(error.message);
                vec![String::new(); prompts.len()]
            }
        }
    }
}
