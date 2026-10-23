//! 工作台偏好独立存储；不包含认证信息或主机信任，命令保存不触发执行。
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path, sync::Mutex};

static STORE: Mutex<()> = Mutex::new(());
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedCommand {
    pub id: String,
    pub name: String,
    pub command: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub groups: BTreeMap<String, String>,
    pub commands: Vec<SavedCommand>,
}
impl Preferences {
    fn validate(&self) -> Result<()> {
        if self.groups.len() > 200 || self.commands.len() > 50 {
            return Err(AppError::new("最多分组 200 个连接，保存 50 条命令"));
        }
        for (id, group) in &self.groups {
            if id.is_empty()
                || id.len() > 128
                || group.chars().count() > 40
                || group.chars().any(char::is_control)
            {
                return Err(AppError::new("连接分组无效，组名最多 40 字"));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for item in &self.commands {
            if item.id.is_empty()
                || item.id.len() > 128
                || !ids.insert(&item.id)
                || item.name.trim().is_empty()
                || item.name.chars().count() > 80
            {
                return Err(AppError::new("命令名称或标识无效"));
            }
            validate_command(&item.command)?;
        }
        Ok(())
    }
}
pub fn validate_command(command: &str) -> Result<()> {
    if command.trim().is_empty() || command.len() > 8192 || command.contains('\0') {
        return Err(AppError::new("命令不能为空、含 NUL 或超过 8 KiB"));
    }
    Ok(())
}
pub fn access(path: &Path, update: Option<Preferences>) -> Result<Preferences> {
    let _guard = STORE
        .lock()
        .map_err(|_| AppError::new("工作台偏好锁不可用"))?;
    // 即使要写入，也先检查旧文件，不能用默认值覆盖损坏的配置。
    let previous: Preferences = if path.exists() {
        if fs::metadata(path)?.len() > 1024 * 1024 {
            return Err(AppError::new("工作台配置超过 1 MiB"));
        }
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        Preferences::default()
    };
    previous.validate()?;
    let Some(next) = update else {
        return Ok(previous);
    };
    next.validate()?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&next)?)?;
    fs::rename(temporary, path)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_roundtrip_and_corruption_preserves_original() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "termrill-workbench-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("workbench.json");
        let mut prefs = Preferences::default();
        prefs.groups.insert("connection".into(), "测试组".into());
        access(&path, Some(prefs)).unwrap();
        assert_eq!(access(&path, None).unwrap().groups["connection"], "测试组");
        fs::write(&path, b"corrupt").unwrap();
        assert!(access(&path, Some(Preferences::default())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rejects_invalid_commands_and_credential_fields() {
        assert!(validate_command("\0whoami").is_err());
        assert!(validate_command(&"x".repeat(8193)).is_err());
        assert!(serde_json::from_str::<Preferences>(
            r#"{"groups":{},"commands":[],"password":"x"}"#
        )
        .is_err());
        let prefs = Preferences {
            commands: vec![
                SavedCommand {
                    id: "a".into(),
                    name: "状态".into(),
                    command: "pwd".into()
                };
                2
            ],
            ..Preferences::default()
        };
        assert!(prefs.validate().is_err());
    }
}
