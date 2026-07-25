//! ASR / TTS 语音配置 REST API
//!
//! 提供对 `settings.toml` 中 `[asr]` 和 `[tts]` 配置的读写接口。
//! 使用文件直接读写，不依赖 axum State（简化路由注册）。
//!
//! # 端点
//!
//! - `GET /api/v1/settings/asr` — 获取 ASR 配置（所有提供商 + 当前激活）
//! - `PUT /api/v1/settings/asr` — 更新 ASR 全部配置（providers + active_provider）
//! - `POST /api/v1/settings/asr/verify` — 验证指定提供商的凭证有效性
//! - `GET /api/v1/settings/tts` — 获取 TTS 配置（脱敏）
//! - `PUT /api/v1/settings/tts` — 更新 TTS 配置 + 持久化
//! - `GET /api/v1/settings/tts/voices` — 获取可用音色列表

use std::collections::HashMap;

use axum::{Json, http::StatusCode};

use crate::config::settings::AppConfig;

// ═══════════════════════════════════════════════════════════════════════════════
// 工具函数
// ═══════════════════════════════════════════════════════════════════════════════

/// 从请求 JSON 中读取可选的字符串字段，空字符串视为 `None`（清除）
fn optional_str(value: &serde_json::Value, key: &str) -> Option<Option<String>> {
    value.get(key).map(|v| match v.as_str() {
        Some("") => None,
        Some(s) => Some(s.to_string()),
        None => None,
    })
}

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
// ASR 端点
// ═══════════════════════════════════════════════════════════════════════════════

/// `GET /api/v1/settings/asr`
///
/// 返回完整的 ASR 配置：所有已配置的提供商凭证 + 当前激活的提供商。
/// 凭证字段返回明文，前端通过 `type="password"` 控制展示/隐藏。
///
/// 同时返回当前激活提供商的 resolved 值（配置 → 环境变量回退）。
pub async fn get_asr_settings() -> Json<serde_json::Value> {
    let cfg = load_config();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "active_provider": cfg.asr.active_provider,
            "providers": cfg.asr.providers,
            "resolved": {
                "app_key": cfg.asr.resolved_app_key().ok(),
                "access_key": cfg.asr.resolved_access_token().ok(),
            }
        }
    }))
}

/// `PUT /api/v1/settings/asr`
///
/// 替换完整的 ASR 配置。支持以下字段：
/// - `active_provider` — 切换当前激活的服务商
/// - `providers` — 所有提供商的完整凭证映射
pub async fn update_asr_settings(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = load_config();

    // 更新 active_provider（如果提供）
    if let Some(active) = body.get("active_provider").and_then(|v| v.as_str()) {
        if !active.is_empty() {
            cfg.asr.active_provider = active.to_string();
        }
    }

    // 更新 providers（如果提供）
    if let Some(providers) = parse_providers(&body) {
        cfg.asr.providers = providers;
    }

    if let Err(e) = crate::config::settings::save_settings(&cfg) {
        tracing::warn!(error = %e, "保存 ASR 配置到文件失败");
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
            "active_provider": cfg.asr.active_provider,
            "providers": cfg.asr.providers,
        }
    })))
}

/// `POST /api/v1/settings/asr/verify`
///
/// 验证指定提供商的凭证是否有效。
///
/// 请求体包含 `provider` 字段和各提供商对应的凭证字段：
/// - doubao: `app_key` + `access_key`
/// - qwen / glm / mimo: `api_key`
/// - xfyun: `app_id` + `api_key` + `api_secret`
pub async fn verify_asr_credentials(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let provider = body
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("doubao");

    match provider {
        "doubao" => verify_doubao(&body).await,
        "qwen" => {
            verify_http_key(
                "qwen",
                &body,
                "api_key",
                "Authorization",
                "Bearer {key}",
                "https://dashscope.aliyuncs.com/api/v1/models",
            )
            .await
        }
        "glm" => {
            verify_http_key(
                "glm",
                &body,
                "api_key",
                "Authorization",
                "Bearer {key}",
                "https://open.bigmodel.cn/api/paas/v4/audio/transcriptions",
            )
            .await
        }
        "mimo" => {
            verify_http_key(
                "mimo",
                &body,
                "api_key",
                "api-key",
                "{key}",
                "https://api.xiaomimimo.com/v1/chat/completions",
            )
            .await
        }
        "xfyun" => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": "讯飞凭证验证需 WebSocket HMAC 鉴权，请在 Web UI 保存后直接测试语音识别功能".to_string() }
        }))),
        _ => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": format!("暂不支持验证 {} 提供商", provider) }
        }))),
    }
}

/// 通用的 HTTP API Key 验证：向指定 URL 发送带认证头的 GET 请求，
/// 非 401 响应即视为凭证有效。
async fn verify_http_key(
    provider: &str,
    body: &serde_json::Value,
    key_field: &str,
    header_name: &str,
    header_template: &str,
    url: &str,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let api_key = body
        .get(key_field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("缺少 {key_field}")
                })),
            )
        })?;

    let header_value = header_template.replace("{key}", api_key);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "success": false, "error": e.to_string() })),
            )
        })?;

    let response = client
        .get(url)
        .header(header_name, &header_value)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if status == 401 || status == 403 {
                Ok(Json(serde_json::json!({
                    "success": true,
                    "data": { "valid": false, "message": format!("{} API Key 无效 (HTTP {})", provider, status) }
                })))
            } else {
                // 200、4xx（非401）、5xx 都认为凭证格式有效（服务可达）
                Ok(Json(serde_json::json!({
                    "success": true,
                    "data": { "valid": true, "message": format!("{} 凭证格式正确，服务可达", provider) }
                })))
            }
        }
        Err(e) => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": format!("无法连接到 {} 服务: {}", provider, e) }
        }))),
    }
}

/// 验证 Doubao 凭证（通过 TTS 合成测试）
async fn verify_doubao(
    body: &serde_json::Value,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let app_key = body
        .get("app_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "error": "缺少 app_key"
                })),
            )
        })?;
    let access_key = body
        .get("access_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "error": "缺少 access_key"
                })),
            )
        })?;

    match verify_doubao_token(app_key, access_key).await {
        Ok(()) => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": true, "message": "凭证验证成功" }
        }))),
        Err(e) => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": format!("凭证验证失败: {}", e) }
        }))),
    }
}

/// 用提供的凭证调用 Doubao TTS 合成测试音频验证有效性
async fn verify_doubao_token(app_key: &str, access_token: &str) -> Result<(), String> {
    use univoice::tts::provider::{DoubaoTts, DoubaoTtsOption};
    use univoice::tts::{BaseTtsOption, TtsProvider, TtsRequest};

    let tts = DoubaoTts::new(DoubaoTtsOption {
        base: BaseTtsOption {
            format: Some("pcm".into()),
            voice: Some("zh_female_xiaohe_uranus_bigtts".into()),
            ..Default::default()
        },
        app_id: Some(app_key.to_string()),
        access_token: Some(access_token.to_string()),
        resource_id: Some("seed-tts-2.0".into()),
        ..Default::default()
    });

    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tts.synthesize(TtsRequest {
            text: "测".to_string(),
            options: None,
        }),
    )
    .await
    {
        Ok(Ok(response)) if !response.audio.is_empty() => Ok(()),
        Ok(Ok(_)) => Err("TTS 返回空音频，请检查凭证".to_string()),
        Ok(Err(e)) => Err(format!("TTS 请求失败: {}", e)),
        Err(_) => Err("TTS 请求超时 (15s)".to_string()),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TTS 端点
// ═══════════════════════════════════════════════════════════════════════════════

/// `GET /api/v1/settings/tts`
///
/// 返回实际生效的配置值（配置优先，未设置时回退到环境变量）。
/// 凭证字段返回明文，前端通过 `type="password"` 控制展示/隐藏。
pub async fn get_tts_settings() -> Json<serde_json::Value> {
    let cfg = load_config();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "provider": cfg.tts.provider,
            "voice": Some(cfg.tts.resolved_voice()),
            "app_key": cfg.tts.resolved_app_key().ok(),
            "access_token": cfg.tts.resolved_access_token().ok(),
            "cluster": cfg.tts.resolved_cluster(),
            "resource_id": cfg.tts.resource_id,
        }
    }))
}

/// `PUT /api/v1/settings/tts`
pub async fn update_tts_settings(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = load_config();

    if let Some(val) = optional_str(&body, "voice") {
        cfg.tts.voice = val;
    }
    if let Some(val) = optional_str(&body, "app_key") {
        cfg.tts.app_key = val;
    }
    if let Some(val) = optional_str(&body, "access_token") {
        cfg.tts.access_token = val;
    }
    if let Some(val) = optional_str(&body, "cluster") {
        cfg.tts.cluster = val;
    }
    if let Some(val) = optional_str(&body, "resource_id") {
        cfg.tts.resource_id = val;
    }

    if let Err(e) = crate::config::settings::save_settings(&cfg) {
        tracing::warn!(error = %e, "保存 TTS 配置到文件失败");
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
            "provider": cfg.tts.provider,
            "voice": cfg.tts.voice,
            "app_key": cfg.tts.app_key,
            "access_token": cfg.tts.access_token,
            "cluster": cfg.tts.cluster,
            "resource_id": cfg.tts.resource_id,
        }
    })))
}

/// `GET /api/v1/settings/tts/voices`
pub async fn list_tts_voices() -> Json<serde_json::Value> {
    let voices = univoice::tts::voices::doubao::list_voices();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "provider": "doubao",
            "voices": voices.iter().map(|v| serde_json::json!({
                "id": v.id,
                "name": v.name,
                "language": v.language,
            })).collect::<Vec<_>>(),
        }
    }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optional_str_present() {
        let json = serde_json::json!({"key": "value"});
        assert_eq!(optional_str(&json, "key"), Some(Some("value".to_string())));
    }

    #[test]
    fn test_optional_str_empty_as_none() {
        let json = serde_json::json!({"key": ""});
        assert_eq!(optional_str(&json, "key"), Some(None));
    }

    #[test]
    fn test_optional_str_missing() {
        let json = serde_json::json!({"other": "val"});
        assert_eq!(optional_str(&json, "key"), None);
    }

    #[test]
    fn test_optional_str_null() {
        let json = serde_json::json!({"key": null});
        assert_eq!(optional_str(&json, "key"), Some(None));
    }

    #[test]
    fn test_parse_providers_valid() {
        let json = serde_json::json!({
            "providers": {
                "doubao": {
                    "app_key": "key1",
                    "access_key": "token1"
                },
                "qwen": {
                    "api_key": "qwen-key"
                }
            }
        });
        let providers = parse_providers(&json).unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(
            providers.get("doubao").unwrap().get("app_key").unwrap(),
            "key1"
        );
        assert_eq!(
            providers.get("qwen").unwrap().get("api_key").unwrap(),
            "qwen-key"
        );
    }

    #[test]
    fn test_parse_providers_skips_empty() {
        let json = serde_json::json!({
            "providers": {
                "doubao": {
                    "app_key": "key1",
                    "access_key": ""
                }
            }
        });
        let providers = parse_providers(&json).unwrap();
        let doubao = providers.get("doubao").unwrap();
        assert_eq!(doubao.get("app_key").unwrap(), "key1");
        assert!(doubao.get("access_key").is_none());
    }

    #[test]
    fn test_parse_providers_missing() {
        let json = serde_json::json!({"other": "value"});
        assert!(parse_providers(&json).is_none());
    }
}
