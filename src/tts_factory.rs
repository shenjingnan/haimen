//! TTS Provider 工厂
//!
//! 根据 TtsConfig 的 `active_provider` 动态创建对应的 `TtsProvider` 实例。
//! 替代之前在各策略中硬编码 `DoubaoTts::new()` 的方式。

use univoice::tts::VoiceId;
use univoice::tts::provider::{
    DoubaoTts, DoubaoTtsOption, GlmTts, GlmTtsOption, OpenaiTts, OpenaiTtsOption, Qwen3Tts,
    Qwen3TtsOption,
};
use univoice::tts::traits::TtsProvider;
use univoice::tts::types::BaseTtsOption;

use crate::config::settings::TtsConfig;

/// 获取当前激活 TTS Provider 的默认音色
fn default_voice(provider: &str) -> &'static str {
    match provider {
        "doubao" => "zh_female_xiaohe_uranus_bigtts",
        "qwen" => "Cherry",
        "glm" => "tongtong",
        _ => "",
    }
}

/// 构建 BaseTtsOption 公共部分
fn build_base_option(config: &TtsConfig, active: &str) -> BaseTtsOption {
    BaseTtsOption {
        api_key: config.get_credential("api_key"),
        voice: config
            .get_credential("voice")
            .or_else(|| Some(default_voice(active).into()))
            .map(|v| VoiceId::from(v.as_str())),
        format: Some("pcm".into()),
        ..Default::default()
    }
}

/// 根据配置创建 TTS Provider 实例
///
/// 返回当前 `active_provider` 对应的 TTS Provider，如果提供商不支持或凭证缺失则返回错误。
pub fn create_tts_provider(config: &TtsConfig) -> Result<Box<dyn TtsProvider>, String> {
    let active = config.active_provider.as_str();

    match active {
        "doubao" => {
            let app_key = config
                .get_credential("app_key")
                .ok_or_else(|| "缺少 Doubao App Key".to_string())?;
            let access_token = config
                .get_credential("access_token")
                .ok_or_else(|| "缺少 Doubao Access Token".to_string())?;
            let cluster = config.get_credential("cluster");
            let resource_id =
                config
                    .get_credential("resource_id")
                    .or_else(|| match cluster.as_deref() {
                        Some("volcano_icl") => Some("seed-tts-1.0".into()),
                        _ => Some("seed-tts-2.0".into()),
                    });
            let voice = config
                .get_credential("voice")
                .or_else(|| Some("zh_female_xiaohe_uranus_bigtts".into()))
                .map(|v| VoiceId::from(v.as_str()));

            Ok(Box::new(DoubaoTts::new(DoubaoTtsOption {
                base: BaseTtsOption {
                    voice,
                    format: Some("pcm".into()),
                    ..Default::default()
                },
                app_id: Some(app_key),
                access_token: Some(access_token),
                resource_id,
                ..Default::default()
            })))
        }
        "qwen" => {
            let api_key = config
                .get_credential("api_key")
                .ok_or_else(|| "缺少 Qwen API Key".to_string())?;
            let mut base = build_base_option(config, "qwen");
            base.api_key = Some(api_key);
            // Qwen3TTS 使用 DashScope Realtime WebSocket 协议，
            // 支持模型 "qwen3-tts-instruct-flash-realtime"（默认）和 48 个英文音色。
            Ok(Box::new(Qwen3Tts::new(Qwen3TtsOption {
                base,
                ..Default::default()
            })))
        }
        "glm" => {
            let api_key = config
                .get_credential("api_key")
                .ok_or_else(|| "缺少 GLM API Key".to_string())?;
            let mut base = build_base_option(config, "glm");
            base.api_key = Some(api_key);
            Ok(Box::new(GlmTts::new(GlmTtsOption {
                base,
                ..Default::default()
            })))
        }
        "openai" => {
            let api_key = config
                .get_credential("api_key")
                .ok_or_else(|| "缺少 OpenAI API Key".to_string())?;
            let mut base = build_base_option(config, "openai");
            base.api_key = Some(api_key);
            Ok(Box::new(OpenaiTts::new(OpenaiTtsOption {
                base,
                ..Default::default()
            })))
        }
        _ => Err(format!("不支持的 TTS 提供商: {}", active)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_create_doubao_provider() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("app_key".to_string(), "test-app-key".to_string());
        creds.insert("access_token".to_string(), "test-access-token".to_string());
        providers.insert("doubao".to_string(), creds);

        let config = TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
            ..Default::default()
        };
        let provider = create_tts_provider(&config).unwrap();
        assert_eq!(provider.name(), "doubao");
    }

    #[test]
    fn test_create_qwen_provider() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("api_key".to_string(), "test-api-key".to_string());
        providers.insert("qwen".to_string(), creds);

        let config = TtsConfig {
            active_provider: "qwen".to_string(),
            providers,
            ..Default::default()
        };
        let provider = create_tts_provider(&config).unwrap();
        assert_eq!(provider.name(), "qwen3-tts");
    }

    #[test]
    fn test_create_glm_provider() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("api_key".to_string(), "test-api-key".to_string());
        providers.insert("glm".to_string(), creds);

        let config = TtsConfig {
            active_provider: "glm".to_string(),
            providers,
            ..Default::default()
        };
        let provider = create_tts_provider(&config).unwrap();
        assert_eq!(provider.name(), "glm");
    }

    #[test]
    fn test_create_openai_provider() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("api_key".to_string(), "test-api-key".to_string());
        providers.insert("openai".to_string(), creds);

        let config = TtsConfig {
            active_provider: "openai".to_string(),
            providers,
            ..Default::default()
        };
        let provider = create_tts_provider(&config).unwrap();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn test_create_unsupported_provider() {
        let config = TtsConfig {
            active_provider: "unknown".to_string(),
            providers: HashMap::new(),
            ..Default::default()
        };
        let result = create_tts_provider(&config);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("不支持的 TTS 提供商"));
    }

    #[test]
    fn test_create_doubao_missing_creds() {
        let config = TtsConfig {
            active_provider: "doubao".to_string(),
            providers: HashMap::new(),
            ..Default::default()
        };
        let result = create_tts_provider(&config);
        assert!(result.is_err());
    }
}
