//! 用户显式启用的 Linux 指标采集；固定脚本，不插入路径或用户参数。
use crate::model::*;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};

static LAST_SAMPLE: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());
pub fn admit_sample(connection_id: &str) -> Result<()> {
    let mut samples = LAST_SAMPLE
        .lock()
        .map_err(|_| AppError::new("采样限流不可用"))?;
    samples.retain(|_, instant| instant.elapsed() < Duration::from_secs(60));
    if samples
        .get(connection_id)
        .is_some_and(|instant| instant.elapsed() < Duration::from_secs(5))
    {
        return Err(AppError::new("同一连接状态采样至少间隔 5 秒"));
    }
    if samples.len() >= 256 {
        return Err(AppError::new("采样连接数超过限制"));
    }
    samples.insert(connection_id.into(), Instant::now());
    Ok(())
}

pub const METRICS_SCRIPT: &str = "LC_ALL=C; export LC_ALL; test -r /proc/stat && test -r /proc/meminfo || exit 65; printf 'TERMRILL_CPU\\n'; head -n 1 /proc/stat; printf 'TERMRILL_MEM\\n'; head -n 64 /proc/meminfo; printf 'TERMRILL_DISK\\n'; df -Pk / | tail -n 1";
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub output: String,
    pub exit_code: Option<u32>,
    pub truncated: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub cpu_total: u64,
    pub cpu_idle: u64,
    pub memory_total: u64,
    pub memory_available: u64,
    pub disk_total: u64,
    pub disk_available: u64,
}
pub fn parse(output: &CommandOutput) -> Result<Metrics> {
    if output.exit_code != Some(0) || output.truncated {
        return Err(AppError::new(
            "状态采集不可用：需 Linux /proc、df 和 SSH 命令权限；文件功能仍可使用",
        ));
    }
    let lines: Vec<_> = output.output.lines().collect();
    let invalid = || AppError::new("状态数据缺失或无效，未显示为正常状态");
    let cpu = lines
        .iter()
        .position(|line| *line == "TERMRILL_CPU")
        .and_then(|i| lines.get(i + 1))
        .ok_or_else(invalid)?;
    let values: Vec<u64> = cpu
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| invalid())?;
    if !cpu.starts_with("cpu ") || values.len() < 4 {
        return Err(invalid());
    }
    let total = values
        .iter()
        .try_fold(0u64, |sum, value| sum.checked_add(*value))
        .ok_or_else(invalid)?;
    let idle = values[3]
        .checked_add(*values.get(4).unwrap_or(&0))
        .ok_or_else(invalid)?;
    let memory = |name: &str| -> Result<u64> {
        let line = lines
            .iter()
            .find(|line| line.starts_with(name))
            .ok_or_else(invalid)?;
        line.split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024))
            .ok_or_else(invalid)
    };
    let disk = lines
        .iter()
        .position(|line| *line == "TERMRILL_DISK")
        .and_then(|i| lines.get(i + 1))
        .ok_or_else(invalid)?;
    let fields: Vec<_> = disk.split_whitespace().collect();
    let disk_number = |index: usize| -> Result<u64> {
        fields
            .get(index)
            .and_then(|s| s.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024))
            .ok_or_else(invalid)
    };
    let result = Metrics {
        cpu_total: total,
        cpu_idle: idle,
        memory_total: memory("MemTotal:")?,
        memory_available: memory("MemAvailable:")?,
        disk_total: disk_number(1)?,
        disk_available: disk_number(3)?,
    };
    if result.memory_total == 0
        || result.disk_total == 0
        || result.memory_available > result.memory_total
        || result.disk_available > result.disk_total
    {
        return Err(invalid());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_cpu_without_double_counting_guest_and_rejects_missing_metrics() {
        let mut result = CommandOutput { output: "TERMRILL_CPU\ncpu 10 0 10 70 10 0 0 0 5 0\nTERMRILL_MEM\nMemTotal: 100 kB\nMemAvailable: 40 kB\nTERMRILL_DISK\n/dev/root 200 50 150 25% /\n".into(), exit_code: Some(0), truncated: false };
        let metrics = parse(&result).unwrap();
        assert_eq!(metrics.cpu_total, 100);
        assert_eq!(metrics.cpu_idle, 80);
        assert_eq!(metrics.disk_available, 150 * 1024);
        result.exit_code = Some(65);
        assert!(parse(&result).is_err());
        result.exit_code = Some(0);
        result.output = "missing".into();
        assert!(parse(&result).is_err());
    }
}
