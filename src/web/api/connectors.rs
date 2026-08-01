//! 消息渠道（Connector）配置与状态 REST API
//!
//! 提供 `settings.toml` 中 `[connectors]`（飞书/钉钉）的配置读写，
//! 以及每个渠道可用状态的实时探测（L1 配置层 + L2 认证层，不含运行时）。
//!
//! # 端点
//!
//! - `GET /api/v1/settings/connectors` — 获取飞书/钉钉渠道配置
//! - `PUT /api/v1/settings/connectors` — 部分更新渠道配置
//! - `GET /api/v1/connectors/status` — 获取每个渠道的可用状态

use axum::{Json, http::StatusCode};

use crate::config::settings::AppConfig;

/// 加载当前配置（文件不存在时返回默认值）
fn load_config() -> AppConfig {
    crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
// 渠道配置读写
// ═══════════════════════════════════════════════════════════════════════════════

/// `GET /api/v1/settings/connectors`
///
/// 返回飞书/钉钉渠道配置。未配置的渠道返回默认结构（enabled=false）。
/// 凭证字段返回明文，前端通过 `type="password"` 控制展示/隐藏。
pub async fn get_connectors_settings() -> Json<serde_json::Value> {
    let cfg = load_config();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "lark": lark_settings_json(cfg.connectors.lark.as_ref()),
            "dingtalk": dingtalk_settings_json(cfg.connectors.dingtalk.as_ref()),
        }
    }))
}

fn lark_settings_json(cfg: Option<&crate::config::settings::LarkConfig>) -> serde_json::Value {
    // lark_cli_path 由内部默认值（"lark-cli"）决定，Web 端不暴露该配置
    match cfg {
        Some(c) => serde_json::json!({
            "enabled": c.enabled,
        }),
        None => serde_json::json!({
            "enabled": false,
        }),
    }
}

fn dingtalk_settings_json(
    cfg: Option<&crate::config::settings::DingTalkConnectorConfig>,
) -> serde_json::Value {
    match cfg {
        Some(c) => serde_json::json!({
            "enabled": c.enabled,
            "client_id": c.client_id,
            "client_secret": c.client_secret,
            "allow_from": c.allow_from,
            "share_session_in_channel": c.share_session_in_channel,
            "robot_code": c.robot_code,
        }),
        None => serde_json::json!({
            "enabled": false,
            "client_id": "",
            "client_secret": "",
            "allow_from": "*",
            "share_session_in_channel": false,
            "robot_code": "",
        }),
    }
}

/// `PUT /api/v1/settings/connectors`
///
/// 部分更新渠道配置：请求体中提供的字段更新，缺失字段保留；配置节不存在时自动创建。
pub async fn update_connectors_settings(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = load_config();

    // 更新飞书配置（lark_cli_path 由内部默认值决定，Web 端不接受配置）
    if let Some(lark) = body.get("lark") {
        let mut lark_cfg = cfg.connectors.lark.clone().unwrap_or_default();
        if let Some(v) = lark.get("enabled").and_then(|v| v.as_bool()) {
            lark_cfg.enabled = v;
        }
        cfg.connectors.lark = Some(lark_cfg);
    }

    // 更新钉钉配置
    if let Some(dt) = body.get("dingtalk") {
        let mut dt_cfg = cfg.connectors.dingtalk.clone().unwrap_or_default();
        if let Some(v) = dt.get("enabled").and_then(|v| v.as_bool()) {
            dt_cfg.enabled = v;
        }
        if let Some(v) = dt.get("client_id").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                dt_cfg.client_id = v.to_string();
            }
        }
        if let Some(v) = dt.get("client_secret").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                dt_cfg.client_secret = v.to_string();
            }
        }
        if let Some(v) = dt.get("allow_from").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                dt_cfg.allow_from = v.to_string();
            }
        }
        if let Some(v) = dt.get("robot_code").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                dt_cfg.robot_code = v.to_string();
            }
        }
        if let Some(v) = dt.get("share_session_in_channel").and_then(|v| v.as_bool()) {
            dt_cfg.share_session_in_channel = v;
        }
        cfg.connectors.dingtalk = Some(dt_cfg);
    }

    if let Err(e) = crate::config::settings::save_settings(&cfg) {
        tracing::warn!(error = %e, "保存渠道配置到文件失败");
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
            "lark": lark_settings_json(cfg.connectors.lark.as_ref()),
            "dingtalk": dingtalk_settings_json(cfg.connectors.dingtalk.as_ref()),
        }
    })))
}

// ═══════════════════════════════════════════════════════════════════════════════
// 渠道可用状态
// ═══════════════════════════════════════════════════════════════════════════════

/// `GET /api/v1/connectors/status`
///
/// 返回每个渠道的可用状态（L1 配置层 + L2 认证层）。
/// 未配置/未启用的渠道也会返回条目，保证前端固定渲染。
pub async fn get_connectors_status() -> Json<serde_json::Value> {
    let cfg = load_config();

    let mut connectors = Vec::new();
    connectors.push(lark_status(&cfg).await);
    connectors.push(dingtalk_status(&cfg).await);

    Json(serde_json::json!({
        "success": true,
        "data": { "connectors": connectors }
    }))
}

fn status_json(
    id: &str,
    name: &str,
    configured: bool,
    enabled: bool,
    auth_ok: bool,
    status: &str,
    detail: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "configured": configured,
        "enabled": enabled,
        "auth_ok": auth_ok,
        "status": status,
        "detail": detail,
    })
}

/// 飞书状态：lark-cli 安装 + 认证情况
async fn lark_status(cfg: &AppConfig) -> serde_json::Value {
    let Some(lark_cfg) = &cfg.connectors.lark else {
        return status_json("lark", "飞书", false, false, false, "disabled", "未配置");
    };
    if !lark_cfg.enabled {
        return status_json("lark", "飞书", true, false, false, "disabled", "未启用");
    }

    let channel = haimen_lark::LarkChannel::new(&lark_cfg.lark_cli_path);
    let health = channel.probe().await;
    if !health.lark_cli_found {
        return status_json(
            "lark",
            "飞书",
            true,
            true,
            false,
            "misconfigured",
            "lark-cli 未安装",
        );
    }
    if !health.authenticated {
        return status_json(
            "lark",
            "飞书",
            true,
            true,
            false,
            "auth_failed",
            "飞书未认证",
        );
    }
    status_json("lark", "飞书", true, true, true, "online", "")
}

/// 钉钉状态：配置校验 + 换取 access_token 验证凭据
async fn dingtalk_status(cfg: &AppConfig) -> serde_json::Value {
    let Some(dt_cfg) = &cfg.connectors.dingtalk else {
        return status_json(
            "dingtalk",
            "钉钉",
            false,
            false,
            false,
            "disabled",
            "未配置",
        );
    };
    if !dt_cfg.enabled {
        return status_json("dingtalk", "钉钉", true, false, false, "disabled", "未启用");
    }

    let dt: crate::connectors::dingtalk::config::DingTalkConfig = dt_cfg.clone().into();
    let resolved = match dt.resolve_env_refs() {
        Ok(r) => r,
        Err(e) => return status_json("dingtalk", "钉钉", true, true, false, "misconfigured", &e),
    };

    if let Err(errors) = resolved.validate() {
        let detail = errors.join("; ");
        return status_json(
            "dingtalk",
            "钉钉",
            true,
            true,
            false,
            "misconfigured",
            &detail,
        );
    }

    match crate::connectors::dingtalk::token::verify_credentials(
        resolved.client_id.clone(),
        resolved.client_secret.clone(),
    )
    .await
    {
        Ok(()) => status_json("dingtalk", "钉钉", true, true, true, "online", ""),
        Err(e) => status_json("dingtalk", "钉钉", true, true, false, "auth_failed", &e),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    fn write_toml_settings(home: &std::path::Path, content: &str) {
        let settings_dir = home.join(".haimen");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(settings_dir.join("settings.toml"), content).unwrap();
    }

    #[test]
    fn test_get_connectors_settings_defaults() {
        run_with_temp_home(|_| {
            let res = block_on(get_connectors_settings());
            let data = &res.0["data"];
            assert_eq!(data["lark"]["enabled"], false);
            // lark_cli_path 不暴露给 Web
            assert!(data["lark"].get("lark_cli_path").is_none());
            assert_eq!(data["dingtalk"]["enabled"], false);
            assert_eq!(data["dingtalk"]["allow_from"], "*");
        });
    }

    #[test]
    fn test_get_connectors_settings_with_config() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.lark]
enabled = true
lark_cli_path = "my-lark"

[connectors.dingtalk]
enabled = false
client_id = "id"
client_secret = "secret"
"#,
            );
            let res = block_on(get_connectors_settings());
            let data = &res.0["data"];
            assert_eq!(data["lark"]["enabled"], true);
            assert!(data["lark"].get("lark_cli_path").is_none());
            assert_eq!(data["dingtalk"]["enabled"], false);
            assert_eq!(data["dingtalk"]["client_id"], "id");
        });
    }

    #[test]
    fn test_update_connectors_settings_partial_preserves_rest() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.dingtalk]
enabled = false
client_id = "id"
client_secret = "secret"
"#,
            );
            // 只更新 enabled，client_id/secret 应保留
            let body = serde_json::json!({ "dingtalk": { "enabled": true } });
            let _ = block_on(update_connectors_settings(Json(body))).unwrap();
            let res = block_on(get_connectors_settings());
            let data = &res.0["data"];
            assert_eq!(data["dingtalk"]["enabled"], true);
            assert_eq!(data["dingtalk"]["client_id"], "id");
            assert_eq!(data["dingtalk"]["client_secret"], "secret");
        });
    }

    #[test]
    fn test_update_connectors_settings_creates_missing_section() {
        run_with_temp_home(|_| {
            let body = serde_json::json!({
                "lark": { "enabled": true }
            });
            let _ = block_on(update_connectors_settings(Json(body))).unwrap();
            let res = block_on(get_connectors_settings());
            let data = &res.0["data"];
            assert_eq!(data["lark"]["enabled"], true);
        });
    }

    #[test]
    fn test_update_connectors_settings_empty_string_skipped() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.dingtalk]
enabled = true
client_id = "keep-id"
client_secret = "keep-secret"
"#,
            );
            // 空字符串不应覆盖已有值
            let body = serde_json::json!({
                "dingtalk": { "client_id": "", "client_secret": "" }
            });
            let _ = block_on(update_connectors_settings(Json(body))).unwrap();
            let res = block_on(get_connectors_settings());
            let data = &res.0["data"];
            assert_eq!(data["dingtalk"]["client_id"], "keep-id");
            assert_eq!(data["dingtalk"]["client_secret"], "keep-secret");
        });
    }

    /// 在同步闭包中执行异步接口（run_with_temp_home 的闭包是同步的）
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(fut)
    }

    #[test]
    fn test_status_not_configured() {
        run_with_temp_home(|_| {
            let res = block_on(get_connectors_status());
            let connectors = &res.0["data"]["connectors"];
            assert_eq!(connectors[0]["id"], "lark");
            assert_eq!(connectors[0]["configured"], false);
            assert_eq!(connectors[0]["status"], "disabled");
            assert_eq!(connectors[1]["id"], "dingtalk");
            assert_eq!(connectors[1]["status"], "disabled");
        });
    }

    #[test]
    fn test_status_disabled_when_not_enabled() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.lark]
enabled = false

[connectors.dingtalk]
enabled = false
client_id = "id"
client_secret = "secret"
"#,
            );
            let res = block_on(get_connectors_status());
            let connectors = &res.0["data"]["connectors"];
            assert_eq!(connectors[0]["configured"], true);
            assert_eq!(connectors[0]["enabled"], false);
            assert_eq!(connectors[0]["status"], "disabled");
            assert_eq!(connectors[1]["configured"], true);
            assert_eq!(connectors[1]["status"], "disabled");
        });
    }

    #[test]
    fn test_status_dingtalk_misconfigured_missing_creds() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.dingtalk]
enabled = true
"#,
            );
            let res = block_on(get_connectors_status());
            let dt = &res.0["data"]["connectors"][1];
            assert_eq!(dt["configured"], true);
            assert_eq!(dt["enabled"], true);
            assert_eq!(dt["status"], "misconfigured");
            assert!(dt["detail"].as_str().unwrap().contains("client_id"));
        });
    }
}
