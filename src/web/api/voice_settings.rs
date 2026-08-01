//! ASR / TTS 语音配置 REST API
//!
//! 提供对 `settings.toml` 中 `[asr]` 和 `[tts]` 配置的读写接口。
//! ASR 和 TTS 端点均使用 axum State 持有共享 `Arc<RwLock<...>>`，
//! 在保存到磁盘后同步更新内存中的配置，实现运行时热加载。
//!
//! # 端点
//!
//! - `GET /api/v1/settings/asr` — 获取 ASR 配置（所有提供商 + 当前激活）
//! - `PUT /api/v1/settings/asr` — 更新 ASR 全部配置（providers + active_provider）
//! - `POST /api/v1/settings/asr/verify` — 验证指定提供商的凭证有效性
//! - `GET /api/v1/settings/tts` — 获取 TTS 配置（所有提供商 + 当前激活）
//! - `PUT /api/v1/settings/tts` — 更新 TTS 全部配置（providers + active_provider），
//!   同时更新内存中的共享配置，使策略热加载
//! - `GET /api/v1/settings/tts/voices` — 获取可用音色列表（支持 `?provider=` 参数）
//! - `POST /api/v1/settings/tts/verify` — 验证指定 TTS 提供商的凭证有效性

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::{Json, extract::Query, extract::State, http::StatusCode};

use crate::config::settings::AppConfig;
use crate::config::settings::AsrConfig;
use crate::config::settings::TtsConfig;

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
// 豆包 TTS 模型 / 音色分类
// ═══════════════════════════════════════════════════════════════════════════════

/// 依据豆包音色 ID 后缀判断所属模型（火山引擎命名约定）
///
/// 2.0 线音色后缀为 `_uranus_bigtts` / `_jupiter_bigtts`，以及 `saturn_*_tob`；
/// 其余（`_moon_bigtts` / `_mars_bigtts` / `_emo_*` / `_conversation_*` 等）为 1.0 线。
fn doubao_voice_model(id: &str) -> &'static str {
    if id.ends_with("_uranus_bigtts")
        || id.ends_with("_jupiter_bigtts")
        || id.starts_with("saturn_")
    {
        "seed-tts-2.0"
    } else {
        "seed-tts-1.0"
    }
}

/// 从配置推导豆包当前模型（resource_id → cluster → 默认 2.0）
///
/// 与 `tts_factory` 中 `create_tts_provider` 的推导规则保持一致。
fn doubao_model_from_config(cfg: &AppConfig) -> &'static str {
    if let Some(rid) = cfg
        .tts
        .providers
        .get("doubao")
        .and_then(|p| p.get("resource_id"))
        .filter(|s| !s.is_empty())
    {
        if rid == "seed-tts-1.0" {
            return "seed-tts-1.0";
        }
        if rid == "seed-tts-2.0" {
            return "seed-tts-2.0";
        }
    }
    match cfg
        .tts
        .providers
        .get("doubao")
        .and_then(|p| p.get("cluster"))
        .map(String::as_str)
    {
        Some("volcano_icl") => "seed-tts-1.0",
        _ => "seed-tts-2.0",
    }
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
///
/// 保存到磁盘后同步更新共享内存中的 ASR 配置，实现运行时热加载。
pub async fn update_asr_settings(
    State(asr_config): State<Arc<RwLock<AsrConfig>>>,
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

    // 热加载：更新共享内存中的 ASR 配置，下次 ASR 调用立即生效
    *asr_config.write().unwrap() = cfg.asr.clone();

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

    // ASR 侧仅验证 token 有效性，使用默认 TTS 资源与音色
    match verify_doubao_token(
        app_key,
        access_key,
        "seed-tts-2.0",
        "zh_female_xiaohe_uranus_bigtts",
    )
    .await
    {
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
async fn verify_doubao_token(
    app_key: &str,
    access_token: &str,
    resource_id: &str,
    voice: &str,
) -> Result<(), String> {
    use univoice::tts::provider::{DoubaoTts, DoubaoTtsOption};
    use univoice::tts::{BaseTtsOption, TtsProvider, TtsRequest, VoiceId};

    let tts = DoubaoTts::new(DoubaoTtsOption {
        base: BaseTtsOption {
            format: Some("pcm".into()),
            voice: Some(VoiceId::from(voice)),
            ..Default::default()
        },
        app_id: Some(app_key.to_string()),
        access_token: Some(access_token.to_string()),
        resource_id: Some(resource_id.to_string()),
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
/// 返回完整的 TTS 配置：所有已配置的提供商凭证 + 当前激活的提供商。
/// 凭证字段返回明文，前端通过 `type="password"` 控制展示/隐藏。
///
/// 同时返回当前激活提供商的 resolved 值（配置 → 环境变量回退）。
pub async fn get_tts_settings() -> Json<serde_json::Value> {
    let cfg = load_config();
    Json(serde_json::json!({
        "success": true,
        "data": {
            "active_provider": cfg.tts.active_provider,
            "providers": cfg.tts.providers,
            "fixed_text_enabled": cfg.tts.fixed_text_enabled,
            "fixed_text": cfg.tts.fixed_text,
            "resolved": {
                "app_key": cfg.tts.resolved_app_key().ok(),
                "access_token": cfg.tts.resolved_access_token().ok(),
                "voice": Some(cfg.tts.resolved_voice()),
            }
        }
    }))
}

/// `PUT /api/v1/settings/tts`
///
/// 替换完整的 TTS 配置。支持以下字段：
/// - `active_provider` — 切换当前激活的服务商
/// - `providers` — 所有提供商的完整凭证映射
/// - `fixed_text_enabled` — 是否启用固定文本模式
/// - `fixed_text` — 固定文本内容
pub async fn update_tts_settings(
    State(tts_config): State<Arc<RwLock<TtsConfig>>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = load_config();

    // 更新 active_provider（如果提供）
    if let Some(active) = body.get("active_provider").and_then(|v| v.as_str()) {
        if !active.is_empty() {
            cfg.tts.active_provider = active.to_string();
        }
    }

    // 更新 providers（如果提供）
    if let Some(providers) = parse_providers(&body) {
        cfg.tts.providers = providers;
    }

    // 更新固定文本模式（如果提供）
    if let Some(enabled) = body.get("fixed_text_enabled").and_then(|v| v.as_bool()) {
        cfg.tts.fixed_text_enabled = enabled;
    }

    // 更新固定文本内容（如果提供）
    if body.get("fixed_text").is_some() {
        let text = body
            .get("fixed_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        cfg.tts.fixed_text = if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        };
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

    // 更新内存中的共享 TTS 配置，使运行时策略即时感知变更
    *tts_config.write().unwrap() = cfg.tts.clone();

    Ok(Json(serde_json::json!({
        "success": true,
        "data": {
            "active_provider": cfg.tts.active_provider,
            "providers": cfg.tts.providers,
        }
    })))
}

/// `GET /api/v1/settings/tts/voices`
///
/// 获取指定提供商的可用音色列表。通过 `?provider=doubao` 查询参数指定。
/// 默认返回当前激活提供商的音色。
///
/// 支持 `?provider=` 和 `?model=` 查询参数。豆包按模型过滤音色，
/// 每个音色响应附带 `model` 字段；其他提供商的 `model` 参数被忽略。
pub async fn list_tts_voices(params: Query<HashMap<String, String>>) -> Json<serde_json::Value> {
    let cfg = load_config();
    let provider = params
        .get("provider")
        .map(String::as_str)
        .unwrap_or_else(|| cfg.tts.active_provider.as_str());

    let model = params.get("model").map(String::as_str);

    let (voices, resp_model, is_doubao) = match provider {
        "doubao" => {
            let target = model.unwrap_or_else(|| doubao_model_from_config(&cfg));
            let list = univoice::tts::voices::doubao::list_voices()
                .into_iter()
                .filter(|v| doubao_voice_model(&v.id) == target)
                .collect::<Vec<_>>();
            (list, Some(target), true)
        }
        "qwen" => (univoice::tts::voices::qwen3_tts::list_voices(), None, false),
        "glm" => (univoice::tts::voices::glm::list_voices(), None, false),
        "minimax" => (univoice::tts::voices::minimax::list_voices(), None, false),
        "qwen_realtime" => (univoice::tts::voices::qwen3_tts::list_voices(), None, false),
        _ => (Vec::new(), None, false),
    };

    Json(serde_json::json!({
        "success": true,
        "data": {
            "provider": provider,
            "model": resp_model,
            "voices": voices.iter().map(|v| {
                let mut obj = serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "language": v.language,
                });
                if is_doubao {
                    obj["model"] = serde_json::json!(doubao_voice_model(&v.id));
                }
                obj
            }).collect::<Vec<_>>(),
        }
    }))
}

/// `POST /api/v1/settings/tts/verify`
///
/// 验证指定 TTS 提供商的凭证是否有效。
///
/// 请求体包含 `provider` 字段和各提供商对应的凭证字段：
/// - doubao: `app_key` + `access_token`
/// - qwen / glm / openai / minimax / gemini: `api_key`
/// - xfyun: `app_id` + `api_key` + `api_secret`
pub async fn verify_tts_credentials(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let provider = body
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("doubao");

    match provider {
        "doubao" => verify_tts_doubao(&body).await,
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
        "openai" => {
            verify_http_key(
                "openai",
                &body,
                "api_key",
                "Authorization",
                "Bearer {key}",
                "https://api.openai.com/v1/models",
            )
            .await
        }
        "minimax" => {
            verify_http_key(
                "minimax",
                &body,
                "api_key",
                "Authorization",
                "Bearer {key}",
                "https://api.minimax.chat/v1/text/chatcompletion_v2",
            )
            .await
        }
        "gemini" => {
            verify_http_key(
                "gemini",
                &body,
                "api_key",
                "x-goog-api-key",
                "{key}",
                "https://generativelanguage.googleapis.com/v1/models",
            )
            .await
        }
        "xfyun" => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": "讯飞凭证验证需 WebSocket HMAC 鉴权，请在 Web UI 保存后直接测试语音合成功能".to_string() }
        }))),
        _ => Ok(Json(serde_json::json!({
            "success": true,
            "data": { "valid": false, "message": format!("暂不支持验证 {} 提供商", provider) }
        }))),
    }
}

/// 用提供的凭证调用 Doubao TTS 合成测试音频验证有效性
///
/// 复用 ASR 端的 verify_doubao 逻辑（TTS 合成测试）
async fn verify_tts_doubao(
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
    // 用表单里配置的模型（resource_id）与音色做真实合成验证，
    // 避免硬编码 2.0 导致「验证通过但实际合成 55000000」的假通过。
    let resource_id = body
        .get("resource_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("seed-tts-2.0");
    let voice = body
        .get("voice")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("zh_female_xiaohe_uranus_bigtts");

    match verify_doubao_token(app_key, access_token, resource_id, voice).await {
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

    // ─── doubao_voice_model 分类器 ──────────────────────────

    #[test]
    fn test_doubao_voice_model_2_0_suffixes() {
        assert_eq!(
            doubao_voice_model("zh_female_xiaohe_uranus_bigtts"),
            "seed-tts-2.0"
        );
        assert_eq!(
            doubao_voice_model("zh_female_vv_jupiter_bigtts"),
            "seed-tts-2.0"
        );
        assert_eq!(
            doubao_voice_model("saturn_zh_female_keainvsheng_tob"),
            "seed-tts-2.0"
        );
    }

    #[test]
    fn test_doubao_voice_model_1_0_suffixes() {
        assert_eq!(
            doubao_voice_model("zh_female_wanwanxiaohe_moon_bigtts"),
            "seed-tts-1.0"
        );
        assert_eq!(
            doubao_voice_model("zh_male_zhubajie_mars_bigtts"),
            "seed-tts-1.0"
        );
        assert_eq!(
            doubao_voice_model("zh_male_lengkugege_emo_v2_mars_bigtts"),
            "seed-tts-1.0"
        );
        assert_eq!(
            doubao_voice_model("zh_male_xudong_conversation_wvae_bigtts"),
            "seed-tts-1.0"
        );
    }

    // ─── doubao_model_from_config 推导 ──────────────────────

    fn make_doubao_cfg(resource_id: Option<&str>, cluster: Option<&str>) -> AppConfig {
        let mut creds = HashMap::new();
        if let Some(r) = resource_id {
            creds.insert("resource_id".to_string(), r.to_string());
        }
        if let Some(c) = cluster {
            creds.insert("cluster".to_string(), c.to_string());
        }
        let mut providers = HashMap::new();
        providers.insert("doubao".to_string(), creds);
        AppConfig {
            tts: TtsConfig {
                active_provider: "doubao".to_string(),
                providers,
                fixed_text_enabled: false,
                fixed_text: None,
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_doubao_model_from_config_resource_id_priority() {
        let cfg = make_doubao_cfg(Some("seed-tts-1.0"), Some("volcano_icl"));
        assert_eq!(doubao_model_from_config(&cfg), "seed-tts-1.0");

        let cfg = make_doubao_cfg(Some("seed-tts-2.0"), Some("volcano_icl"));
        assert_eq!(doubao_model_from_config(&cfg), "seed-tts-2.0");
    }

    #[test]
    fn test_doubao_model_from_config_cluster_fallback() {
        let cfg = make_doubao_cfg(None, Some("volcano_icl"));
        assert_eq!(doubao_model_from_config(&cfg), "seed-tts-1.0");

        let cfg = make_doubao_cfg(None, None);
        assert_eq!(doubao_model_from_config(&cfg), "seed-tts-2.0");

        let cfg = make_doubao_cfg(None, Some("other_cluster"));
        assert_eq!(doubao_model_from_config(&cfg), "seed-tts-2.0");
    }
}
