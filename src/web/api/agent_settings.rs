//! Agent 配置 REST API
//!
//! 提供对 `settings.toml` 中 `[gateway]` 的 Agent 配置的读写接口。
//! 使用文件直接读写，不依赖 axum State（简化路由注册）。
//!
//! # 端点
//!
//! - `GET /api/v1/settings/agent` — 获取 Agent 配置（active_provider + providers）
//! - `PUT /api/v1/settings/agent` — 更新 Agent 配置
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

/// `POST /api/v1/settings/agent/verify`
///
/// 验证指定 Agent 提供商是否可用。
///
/// 请求体包含 `provider` 字段：
/// - `claude-code` — 检查 `claude --version`
/// - `codex` — 检查 `codex --version`
pub async fn verify_agent_credentials(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let provider = body
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("claude-code");

    match provider {
        "claude-code" => {
            let available = check_claude_available().await;
            Ok(Json(serde_json::json!({
                "success": true,
                "data": {
                    "valid": available,
                    "message": if available {
                        "Claude Code CLI 可用"
                    } else {
                        "Claude CLI 未安装或不可用，请执行: npm install -g @anthropic-ai/claude-code"
                    }
                }
            })))
        }
        "codex" => {
            let available = check_codex_available().await;
            Ok(Json(serde_json::json!({
                "success": true,
                "data": {
                    "valid": available,
                    "message": if available {
                        "Codex CLI 可用"
                    } else {
                        "Codex CLI 未安装或不可用，请执行: npm install -g @openai/codex"
                    }
                }
            })))
        }
        _ => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": format!("暂不支持验证 {} 提供商", provider) }
        }))),
    }
}

/// 检查 claude CLI 是否可用
async fn check_claude_available() -> bool {
    tokio::process::Command::new("claude")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 检查 codex CLI 是否可用
async fn check_codex_available() -> bool {
    tokio::process::Command::new("codex")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
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
