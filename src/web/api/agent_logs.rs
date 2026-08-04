//! Agent 调用日志 REST API
//!
//! - `GET /api/v1/agent/logs` — 查询 Agent 调用日志（支持日期/来源/会话/状态/条数过滤）
//!
//! 只读接口，不触发日志清理（`cleanup` 属于 CLI 副作用）。

use axum::{Json, extract::Query};
use serde::Deserialize;

/// 查询参数（均可选）
#[derive(Debug, Deserialize)]
pub struct AgentLogsQuery {
    /// YYYY-MM-DD，缺省扫全部日期文件
    day: Option<String>,
    /// 来源：gateway | xiaozhi | cli
    source: Option<String>,
    /// chat_id 精确匹配
    chat: Option<String>,
    /// 状态：success | error | timeout
    status: Option<String>,
    /// 返回条数上限，默认 200，钳制在 1..=5000
    limit: Option<usize>,
}

/// `GET /api/v1/agent/logs`
///
/// 返回 `{ success, data: { enabled, records } }`：
/// - `enabled=false` 表示 `[agent_log] enabled` 未开启（无数据可查），前端据此提示
/// - `records` 为按时间倒序的 `AgentLogRecord` 列表
pub async fn get_agent_logs(Query(q): Query<AgentLogsQuery>) -> Json<serde_json::Value> {
    let cfg = crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    // 日志未启用：不落盘 → 一定无数据，作为页面状态返回而非错误
    if !cfg.agent_log.enabled {
        return Json(serde_json::json!({
            "success": true,
            "data": { "enabled": false, "records": [] }
        }));
    }

    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let records = crate::agent_log::load(
        q.day.as_deref(),
        q.source.as_deref(),
        q.chat.as_deref(),
        q.status.as_deref(),
        limit,
    );

    Json(serde_json::json!({
        "success": true,
        "data": { "enabled": true, "records": records }
    }))
}
