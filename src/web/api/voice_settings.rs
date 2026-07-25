//! ASR / TTS 语音配置 REST API
//!
//! 提供对 `settings.toml` 中 `[asr]` 和 `[tts]` 配置的读写接口。
//! 使用文件直接读写，不依赖 axum State（简化路由注册）。
//!
//! # 端点
//!
//! - `GET /api/v1/settings/asr` — 获取 ASR 配置（脱敏）
//! - `PUT /api/v1/settings/asr` — 更新 ASR 配置 + 持久化
//! - `POST /api/v1/settings/asr/verify` — 验证 ASR 凭证有效性
//! - `GET /api/v1/settings/tts` — 获取 TTS 配置（脱敏）
//! - `PUT /api/v1/settings/tts` — 更新 TTS 配置 + 持久化
//! - `GET /api/v1/settings/tts/voices` — 获取可用音色列表

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
/// 返回实际生效的配置值（配置优先，未设置时回退到环境变量）。
/// 凭证字段返回明文，前端通过 `type="password"` 控制展示/隐藏。
pub async fn get_asr_settings() -> Json<serde_json::Value> {
    let cfg = load_config();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "provider": cfg.asr.provider,
            "app_key": cfg.asr.resolved_app_key().ok(),
            "access_token": cfg.asr.resolved_access_token().ok(),
        }
    }))
}

/// `PUT /api/v1/settings/asr`
pub async fn update_asr_settings(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = load_config();

    if let Some(val) = optional_str(&body, "app_key") {
        cfg.asr.app_key = val;
    }
    if let Some(val) = optional_str(&body, "access_token") {
        cfg.asr.access_token = val;
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
            "provider": cfg.asr.provider,
            "app_key": cfg.asr.app_key,
            "access_token": cfg.asr.access_token,
        }
    })))
}

/// `POST /api/v1/settings/asr/verify`
///
/// 用提供的凭证合成一段极短 TTS 音频来验证 Doubao 凭证是否有效。
pub async fn verify_asr_credentials(
    Json(body): Json<serde_json::Value>,
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
    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "error": "缺少 access_token"
                })),
            )
        })?;

    match verify_doubao_token(app_key, access_token).await {
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
}
