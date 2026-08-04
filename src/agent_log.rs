//! Agent 调用日志持久化存储
//!
//! 每次调用 Agent（输入消息 + 输出回复）以 JSONL 形式按天落盘，
//! 供 `haimen agent log` 查询。存储目录默认为 `~/.haimen/agent-logs/`，
//! 可用 `[agent_log]` 配置覆盖。
//!
//! 文件格式：`{YYYY-MM-DD}.jsonl`，每行一条 JSON 记录，追加写入。

use chrono::{Duration as ChronoDuration, Local};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::config::settings::{AgentLogConfig, load_settings};

/// 一次 Agent 调用的日志记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentLogRecord {
    /// ISO 8601 本地时间戳（调用完成时刻）
    pub timestamp: String,
    /// 调用来源："gateway" | "xiaozhi" | "cli"
    pub source: String,
    /// Agent 名称（agent.name()）
    pub agent: String,
    /// 来源通道名（gateway 的 lark/dingtalk 等）
    pub connector: Option<String>,
    /// 会话标识（gateway: connector:conversation_id；xiaozhi: 设备 session）
    pub chat_id: Option<String>,
    /// 发送者（gateway）
    pub sender_id: Option<String>,
    /// Agent 的 session_id（用于 resume）
    pub session_id: Option<String>,
    /// Agent 子进程工作目录
    pub work_dir: String,
    /// 用户输入
    pub input: String,
    /// 输出回复（error/timeout 时为 None）
    pub output: Option<String>,
    /// 状态："success" | "error" | "timeout"
    pub status: String,
    /// 错误信息（status != success 时）
    pub error: Option<String>,
    /// Agent 调用耗时（毫秒）
    pub latency_ms: u64,
}

/// 全局写锁，串行化并发追加，防止多连接器/xiaozhi 并发写时交错
static RECORD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn record_lock() -> &'static Mutex<()> {
    RECORD_LOCK.get_or_init(|| Mutex::new(()))
}

/// 读取 agent 日志配置（文件缺失时使用默认值）
fn agent_log_config() -> AgentLogConfig {
    load_settings().ok().flatten().unwrap_or_default().agent_log
}

/// 解析日志目录（配置 dir 或默认 ~/.haimen/agent-logs）
fn resolve_log_dir(cfg: &AgentLogConfig) -> PathBuf {
    match &cfg.dir {
        Some(d) => crate::gateway::chat_loop::expand_tilde(d).into(),
        None => crate::config::settings::get_settings_dir().join("agent-logs"),
    }
}

/// 记录一次 Agent 调用
///
/// - 配置 `enabled = false` 时直接跳过
/// - 写入失败只记录 warning，不阻塞消息处理
pub fn record(rec: &AgentLogRecord) {
    let cfg = agent_log_config();
    if !cfg.enabled {
        return;
    }

    let dir = resolve_log_dir(&cfg);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(path = %dir.display(), error = %e, "创建 agent 日志目录失败，跳过记录");
        return;
    }

    let file_name = format!("{}.jsonl", Local::now().format("%Y-%m-%d"));
    let path = dir.join(file_name);

    let line = match serde_json::to_string(rec) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "序列化 agent 日志失败，跳过记录");
            return;
        }
    };

    let _guard = match record_lock().lock() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = %e, "获取 agent 日志写锁失败，跳过记录");
            return;
        }
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "打开 agent 日志文件失败，跳过记录");
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        tracing::warn!(path = %path.display(), error = %e, "写入 agent 日志失败，跳过记录");
    }
    let _ = file.flush();
}

/// 查询 Agent 调用日志
///
/// - 按时间倒序返回
/// - `day`：只读指定日期文件（YYYY-MM-DD）
/// - `source`：按来源过滤（gateway/xiaozhi/cli）
/// - `chat`：按 chat_id 精确过滤
/// - `limit`：最多返回条数
pub fn load(
    day: Option<&str>,
    source: Option<&str>,
    chat: Option<&str>,
    limit: usize,
) -> Vec<AgentLogRecord> {
    let cfg = agent_log_config();
    let dir = resolve_log_dir(&cfg);
    if !dir.exists() {
        return Vec::new();
    }

    let mut records = Vec::new();
    let target_name = day.map(|d| format!("{}.jsonl", d));

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !fname.ends_with(".jsonl") {
                continue;
            }
            // day 过滤：文件名必须精确匹配 {day}.jsonl
            if let Some(target) = &target_name {
                if target != fname {
                    continue;
                }
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(rec) = serde_json::from_str::<AgentLogRecord>(line) {
                    records.push(rec);
                }
            }
        }
    }

    records.retain(|r| {
        source.map(|s| r.source == s).unwrap_or(true)
            && chat
                .map(|c| r.chat_id.as_deref() == Some(c))
                .unwrap_or(true)
    });

    // ISO 8601 同格式字符串按字典序即时间倒序
    records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    if records.len() > limit {
        records.truncate(limit);
    }
    records
}

/// 删除超过 `retention_days` 天的日志文件，返回删除数量
pub fn cleanup(retention_days: u64) -> usize {
    let cfg = agent_log_config();
    let dir = resolve_log_dir(&cfg);
    if !dir.exists() {
        return 0;
    }

    let today = Local::now().date_naive();
    let cutoff = today - ChronoDuration::days(retention_days as i64);
    let mut removed = 0;

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !fname.ends_with(".jsonl") {
                continue;
            }
            let date_str = &fname[..fname.len() - ".jsonl".len()];
            let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
                continue;
            };
            if date < cutoff {
                match std::fs::remove_file(&path) {
                    Ok(()) => removed += 1,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "删除过期 agent 日志失败")
                    }
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    fn sample_record(source: &str, chat: &str) -> AgentLogRecord {
        AgentLogRecord {
            timestamp: crate::datetime::iso_timestamp_now(),
            source: source.to_string(),
            agent: "claude-code".to_string(),
            connector: Some("lark".to_string()),
            chat_id: Some(chat.to_string()),
            sender_id: Some("user_1".to_string()),
            session_id: Some("session-1".to_string()),
            work_dir: "/tmp".to_string(),
            input: "你好".to_string(),
            output: Some("你好！".to_string()),
            status: "success".to_string(),
            error: None,
            latency_ms: 123,
        }
    }

    fn log_dir(home: &std::path::Path) -> PathBuf {
        home.join(".haimen/agent-logs")
    }

    #[test]
    fn test_record_writes_daily_file() {
        run_with_temp_home(|home| {
            let rec = sample_record("cli", "chat-a");
            record(&rec);

            let day = Local::now().format("%Y-%m-%d").to_string();
            let path = log_dir(home).join(format!("{}.jsonl", day));
            assert!(path.exists(), "日志文件应存在");
            let content = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = content.lines().collect();
            assert_eq!(lines.len(), 1);
            let parsed: AgentLogRecord = serde_json::from_str(lines[0]).unwrap();
            assert_eq!(parsed, rec);
        });
    }

    #[test]
    fn test_record_disabled_no_file() {
        run_with_temp_home(|home| {
            // 写一个 enabled = false 的配置
            let settings_dir = home.join(".haimen");
            std::fs::create_dir_all(&settings_dir).unwrap();
            std::fs::write(
                settings_dir.join("settings.toml"),
                "[agent_log]\nenabled = false\n",
            )
            .unwrap();

            record(&sample_record("cli", "chat-a"));
            assert!(!log_dir(home).exists(), "disabled 时不应产生日志文件");
        });
    }

    #[test]
    fn test_record_custom_dir() {
        run_with_temp_home(|home| {
            let custom = home.join("my-logs");
            let settings_dir = home.join(".haimen");
            std::fs::create_dir_all(&settings_dir).unwrap();
            std::fs::write(
                settings_dir.join("settings.toml"),
                format!("[agent_log]\ndir = \"{}\"\n", custom.display()),
            )
            .unwrap();

            record(&sample_record("cli", "chat-a"));
            let day = Local::now().format("%Y-%m-%d").to_string();
            assert!(custom.join(format!("{}.jsonl", day)).exists());
        });
    }

    #[test]
    fn test_load_order_and_filters() {
        run_with_temp_home(|home| {
            let day = Local::now().format("%Y-%m-%d").to_string();
            let path = log_dir(home).join(format!("{}.jsonl", day));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();

            // 三条记录：不同时间戳（手工设定先后），两条 source=gateway、一条 cli
            let mut r1 = sample_record("gateway", "chat-1");
            r1.timestamp = "2026-08-04T10:00:00+08:00".to_string();
            let mut r2 = sample_record("gateway", "chat-2");
            r2.timestamp = "2026-08-04T11:00:00+08:00".to_string();
            let mut r3 = sample_record("cli", "chat-3");
            r3.timestamp = "2026-08-04T12:00:00+08:00".to_string();

            let mut content = String::new();
            content.push_str(&serde_json::to_string(&r1).unwrap());
            content.push('\n');
            content.push_str(&serde_json::to_string(&r2).unwrap());
            content.push('\n');
            content.push_str(&serde_json::to_string(&r3).unwrap());
            content.push('\n');
            std::fs::write(&path, content).unwrap();

            // 全量：时间倒序（r3 最早写入但最晚时间 → 排最前）
            let all = load(None, None, None, 10);
            assert_eq!(all.len(), 3);
            assert_eq!(all[0].timestamp, "2026-08-04T12:00:00+08:00");
            assert_eq!(all[2].timestamp, "2026-08-04T10:00:00+08:00");

            // source 过滤
            let gw = load(None, Some("gateway"), None, 10);
            assert_eq!(gw.len(), 2);
            assert!(gw.iter().all(|r| r.source == "gateway"));

            // chat 过滤
            let c1 = load(None, None, Some("chat-1"), 10);
            assert_eq!(c1.len(), 1);
            assert_eq!(c1[0].chat_id.as_deref(), Some("chat-1"));

            // limit
            let limited = load(None, None, None, 2);
            assert_eq!(limited.len(), 2);

            // day 过滤（错误日期 → 空）
            let wrong_day = load(Some("2000-01-01"), None, None, 10);
            assert!(wrong_day.is_empty());
        });
    }

    #[test]
    fn test_cleanup_removes_old_files() {
        run_with_temp_home(|home| {
            let dir = log_dir(home);
            std::fs::create_dir_all(&dir).unwrap();
            let today = Local::now().format("%Y-%m-%d").to_string();

            std::fs::write(dir.join(format!("{}.jsonl", today)), "x\n").unwrap();
            std::fs::write(dir.join("2000-01-01.jsonl"), "x\n").unwrap();
            std::fs::write(dir.join("not-a-date.txt"), "x\n").unwrap();

            let removed = cleanup(30);
            assert_eq!(removed, 1, "只应删除过期的 2000-01-01.jsonl");
            assert!(dir.join(format!("{}.jsonl", today)).exists());
            assert!(!dir.join("2000-01-01.jsonl").exists());
            assert!(dir.join("not-a-date.txt").exists(), "非 jsonl 文件不应被删");
        });
    }

    #[test]
    fn test_record_concurrent_no_corruption() {
        run_with_temp_home(|home| {
            let mut handles = Vec::new();
            for i in 0..20 {
                handles.push(std::thread::spawn(move || {
                    let rec = sample_record("cli", &format!("chat-{}", i));
                    record(&rec);
                }));
            }
            for h in handles {
                h.join().unwrap();
            }

            let day = Local::now().format("%Y-%m-%d").to_string();
            let path = log_dir(home).join(format!("{}.jsonl", day));
            let content = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
            assert_eq!(lines.len(), 20, "20 条并发写入应逐行完整");
            for line in &lines {
                let parsed: AgentLogRecord = serde_json::from_str(line).unwrap();
                assert!(parsed.chat_id.is_some());
            }
        });
    }
}
