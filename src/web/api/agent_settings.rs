//! Agent 配置 REST API
//!
//! 提供对 `settings.toml` 中 `[gateway]` 的 Agent 配置的读写接口。
//! 使用文件直接读写，不依赖 axum State（简化路由注册）。
//!
//! # 端点
//!
//! - `GET /api/v1/settings/agent` — 获取 Agent 配置（active_provider + providers）
//! - `PUT /api/v1/settings/agent` — 更新 Agent 配置
//! - `GET /api/v1/agent/providers` — 列出注册表中所有 Agent 提供商
//! - `POST /api/v1/settings/agent/verify` — 验证指定 Agent CLI 是否可用

use std::collections::HashMap;

use axum::{Json, http::StatusCode};

use crate::config::settings::AppConfig;

// ═══════════════════════════════════════════════════════════════════════════════
// 工具函数
// ═══════════════════════════════════════════════════════════════════════════════

/// 从请求 JSON 中读取 providers 映射
fn parse_providers(value: &serde_json::Value) -> Option<HashMap<String, HashMap<String, String>>> {
    let obj = value.get("providers")?.as_object()?;
    let mut providers = HashMap::new();
    for (name, fields) in obj {
        if let Some(fields_obj) = fields.as_object() {
            let mut creds = HashMap::new();
            for (k, v) in fields_obj {
                if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
                    creds.insert(k.clone(), s.to_string());
                }
            }
            providers.insert(name.clone(), creds);
        }
    }
    Some(providers)
}

/// 加载当前配置（文件不存在时返回默认值）
fn load_config() -> AppConfig {
    crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Agent 端点
// ═══════════════════════════════════════════════════════════════════════════════

/// `GET /api/v1/settings/agent`
///
/// 返回完整的 Agent 配置：所有已配置的提供商参数 + 当前激活的提供商。
pub async fn get_agent_settings() -> Json<serde_json::Value> {
    let cfg = load_config();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "active_provider": cfg.gateway.active_provider,
            "providers": cfg.gateway.providers,
            "resolved": {
                "agent": cfg.gateway.resolved_agent(),
            }
        }
    }))
}

/// `PUT /api/v1/settings/agent`
///
/// 替换完整的 Agent 配置。支持以下字段：
/// - `active_provider` — 切换当前激活的 Agent 提供商
/// - `providers` — 所有提供商的完整参数映射
pub async fn update_agent_settings(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = load_config();

    // 更新 active_provider（如果提供）
    if let Some(active) = body.get("active_provider").and_then(|v| v.as_str()) {
        if !active.is_empty() {
            cfg.gateway.active_provider = active.to_string();
        }
    }

    // 更新 providers（如果提供）
    if let Some(providers) = parse_providers(&body) {
        cfg.gateway.providers = providers;
    }

    if let Err(e) = crate::config::settings::save_settings(&cfg) {
        tracing::warn!(error = %e, "保存 Agent 配置到文件失败");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "error": format!("保存配置失败: {}", e)
            })),
        ));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "data": {
            "active_provider": cfg.gateway.active_provider,
            "providers": cfg.gateway.providers,
        }
    })))
}

/// `GET /api/v1/agent/providers`
///
/// 返回注册表中所有可用的 Agent 提供商（id + display_name）。
/// 前端据此动态渲染提供商 Tab，新增 Agent 后无需改动前端。
pub async fn list_agent_providers() -> Json<serde_json::Value> {
    let providers: Vec<serde_json::Value> = crate::agents::registry::registry()
        .list()
        .into_iter()
        .map(|info| {
            serde_json::json!({
                "id": info.id,
                "display_name": info.display_name,
            })
        })
        .collect();

    Json(serde_json::json!({
        "success": true,
        "data": { "providers": providers }
    }))
}

/// `POST /api/v1/settings/agent/verify`
///
/// 验证指定 Agent 提供商是否可用。
///
/// 请求体包含 `provider` 字段。统一通过注册表构造 Agent 并调用其
/// `check_available()`（如 `claude --version` / `codex --version`），
/// 新增 Agent 无需改动此处。
pub async fn verify_agent_credentials(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let provider = body
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("claude-code");

    let cfg = load_config();
    let agent = match crate::agents::registry::registry().build(provider, &cfg.gateway) {
        Ok(agent) => agent,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "data": { "valid": false, "message": e }
            })));
        }
    };

    match agent.check_available().await {
        Ok(()) => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": true, "message": format!("{} CLI 可用", agent.name()) }
        }))),
        Err(e) => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": e }
        }))),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_providers_valid() {
        let json = serde_json::json!({
            "providers": {
                "claude-code": {
                    "note": "CLI 工具无需额外凭证"
                },
                "codex": {
                    "note": "CLI 工具无需额外凭证"
                }
            }
        });
        let providers = parse_providers(&json).unwrap();
        assert_eq!(providers.len(), 2);
        assert!(providers.contains_key("claude-code"));
        assert!(providers.contains_key("codex"));
    }

    #[test]
    fn test_parse_providers_skips_empty() {
        let json = serde_json::json!({
            "providers": {
                "claude-code": {
                    "note": ""
                }
            }
        });
        let providers = parse_providers(&json).unwrap();
        let cc = providers.get("claude-code").unwrap();
        assert!(cc.get("note").is_none());
    }

    #[test]
    fn test_parse_providers_missing() {
        let json = serde_json::json!({"other": "value"});
        assert!(parse_providers(&json).is_none());
    }
}
