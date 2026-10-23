//! 同步比较选项：严格模式默认保留内容校验，排除目录在扫描时剪枝。
use crate::model::*;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub quick: bool,
    #[serde(default)]
    pub exclude: Vec<String>,
}

pub struct Exclusions(Vec<glob::Pattern>);
impl Options {
    pub fn exclusions(&self) -> Result<Exclusions> {
        if self.exclude.len() > 32 {
            return Err(AppError::new("排除项最多 32 条"));
        }
        let mut patterns = Vec::new();
        for value in &self.exclude {
            let pattern = value.trim().trim_end_matches('/');
            if pattern.is_empty()
                || pattern.len() > 256
                || pattern.starts_with('/')
                || pattern.contains(['\\', ':', '\0'])
                || pattern.split('/').any(|part| part == "." || part == "..")
            {
                return Err(AppError::new(
                    "排除项须为目录内相对路径或通配符，每条最多 256 字节",
                ));
            }
            patterns
                .push(glob::Pattern::new(pattern).map_err(|_| AppError::new("排除通配符无效"))?);
        }
        Ok(Exclusions(patterns))
    }
}
impl Exclusions {
    pub fn contains(&self, relative: &str) -> bool {
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        self.0.iter().any(|pattern| {
            let path = if pattern.as_str().contains('/') {
                relative
            } else {
                relative.rsplit('/').next().unwrap_or(relative)
            };
            pattern.matches_with(path, options)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exclusions_are_relative_and_match_hidden_nested_paths() {
        let rules = Options {
            quick: false,
            exclude: vec!["*.log".into(), ".git/".into(), "build/**/cache".into()],
        }
        .exclusions()
        .unwrap();
        assert!(rules.contains("nested/a.log"));
        assert!(rules.contains("nested/.git"));
        assert!(rules.contains("build/cache"));
        assert!(rules.contains("build/a/cache"));
        assert!(!rules.contains("nested/a.txt"));
        for bad in ["../a", "/root", "C:\\a", "a/../b", "["] {
            assert!(Options {
                quick: true,
                exclude: vec![bad.into()]
            }
            .exclusions()
            .is_err());
        }
    }
}
