//! 单向同步规则独立落盘，不改连接配置，不保存认证信息，也不自动执行。
use crate::{directory_sync_options::Options, model::*};
use std::{fs, path::Path, sync::Mutex};

static STORE: Mutex<()> = Mutex::new(());

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub local: String,
    pub remote: String,
    pub direction: String,
    pub options: Options,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Store {
    scopes: std::collections::BTreeMap<String, Vec<Rule>>,
}

impl Rule {
    fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > 64
            || self.name.trim().is_empty()
            || self.name.len() > 160
            || !["upload", "download"].contains(&self.direction.as_str())
            || !Path::new(&self.local).is_absolute()
            || !self.remote.starts_with('/')
            || [&self.local, &self.remote]
                .iter()
                .any(|path| path.len() > 4096 || path.contains('\0'))
        {
            return Err(AppError::new("同步规则名称、方向或绝对路径无效"));
        }
        self.options.exclusions()?;
        Ok(())
    }
}

pub fn access(
    path: &Path,
    scope: &str,
    rule: Option<Rule>,
    remove: Option<String>,
) -> Result<Vec<Rule>> {
    let _guard = STORE
        .lock()
        .map_err(|_| AppError::new("同步规则存储不可用"))?;
    if scope.is_empty() || scope.len() > 8192 || (rule.is_some() && remove.is_some()) {
        return Err(AppError::new("同步规则连接范围或操作无效"));
    }
    let mut stored: Store = if path.exists() {
        if fs::metadata(path)?.len() > 4 * 1024 * 1024 {
            return Err(AppError::new("同步规则文件超过 4 MiB，未覆盖原文件"));
        }
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        Store::default()
    };
    let changed = rule.is_some() || remove.is_some();
    let mut rules = stored.scopes.get(scope).cloned().unwrap_or_default();
    if let Some(rule) = rule {
        rule.validate()?;
        if let Some(old) = rules.iter_mut().find(|old| old.id == rule.id) {
            *old = rule;
        } else {
            if rules.len() >= 20 {
                return Err(AppError::new("每个连接最多保存 20 条同步规则"));
            }
            rules.push(rule);
        }
    }
    if let Some(id) = remove {
        rules.retain(|rule| rule.id != id);
    }
    if changed {
        if rules.is_empty() {
            stored.scopes.remove(scope);
        } else {
            stored.scopes.insert(scope.into(), rules.clone());
        }
        if stored.scopes.len() > 100 {
            return Err(AppError::new("同步规则连接范围最多 100 个"));
        }
        let bytes = serde_json::to_vec_pretty(&stored)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(AppError::new("同步规则总大小超过 4 MiB"));
        }
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)?;
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_are_scoped_and_corruption_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!(
            "termrill-sync-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("rules.json");
        let rule = Rule {
            id: "one".into(),
            name: "同步".into(),
            local: dir.to_string_lossy().into(),
            remote: "/work".into(),
            direction: "upload".into(),
            options: Options::default(),
        };
        assert_eq!(access(&path, "a", Some(rule), None).unwrap().len(), 1);
        assert!(access(&path, "b", None, None).unwrap().is_empty());
        assert!(access(&path, "a", None, Some("one".into()))
            .unwrap()
            .is_empty());
        fs::write(&path, b"broken").unwrap();
        assert!(access(&path, "a", None, Some("one".into())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken");
        fs::remove_dir_all(dir).unwrap();
    }
}
