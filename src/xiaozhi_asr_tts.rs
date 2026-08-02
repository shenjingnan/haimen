//! xiaozhi-esp32 ASR → TTS 响应策略
//!
//! 将设备录制的 Opus 音频解码为 PCM，通过 Doubao ASR 识别为文字，
//! 再通过 Doubao TTS 合成为语音，编码为 Opus 帧后发送给设备播放。
//!
//! # 管线
//!
//! ```text
//! 设备 Opus 帧 (16kHz)
//!   ↓ opus2::Decoder
//! PCM16 mono 16000Hz
//!   ↓ DoubaoAsr::listen_stream
//! 识别文本
//!   ↓ DoubaoTts::synthesize(format="pcm")
//! PCM16 mono 24000Hz
//!   ↓ pcm_to_opus_frames() (24kHz, 60ms)
//! Vec<OpusPacket>
//!   ↓ 封装为 AudioFrame { timestamp, data }
//! play_back_frames() (已有复用)
//!   ↓ BinaryProtocol2 → 设备播放
//! ```

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures_util::StreamExt;
use haimen_xiaozhi::{AudioFrame, AudioParams, ResponseStrategy};
use opus2::{Channels, Decoder};
use univoice::asr::{
    AsrProvider, AudioContainerFormat, AudioInput, BaseProviderOption, DEFAULT_CHUNK_SIZE,
    DoubaoAsr, DoubaoAsrMode, DoubaoAsrOption, GlmAsr, GlmAsrOption, MimoAsr, MimoAsrOption,
    QwenAsr, QwenAsrOption, XfyunAsr, XfyunAsrOption, adapt_audio_input,
};
use univoice::tts::TtsRequest;

use crate::config::settings::{AsrConfig, TtsConfig};

use crate::xiaozhi_tts::pcm_to_opus_frames;

/// 共享 TTS 配置类型
pub type SharedTtsConfig = Arc<RwLock<TtsConfig>>;

/// 共享 ASR 配置类型
pub type SharedAsrConfig = Arc<RwLock<AsrConfig>>;

/// ASR→TTS 响应策略：将设备录制的语音识别为文字，再合成为语音回传
///
/// 管线：Opus 解码 (16kHz) → Doubao ASR → TTS Provider (24kHz) → Opus 编码
pub struct AsrTtsStrategy {
    /// ASR 配置（包含活跃提供商和凭证），通过 Arc<RwLock> 支持运行时热加载
    asr_config: SharedAsrConfig,
    /// TTS 配置（包含活跃提供商和凭证），通过 Arc<RwLock> 支持运行时热加载
    tts_config: SharedTtsConfig,
    /// CLI 音色覆盖（--xiaozhi-tts-voice），叠加到共享配置之上，不写入磁盘
    voice_override: Option<String>,
}

impl AsrTtsStrategy {
    /// 创建 ASR→TTS 策略
    pub fn new(
        asr_config: SharedAsrConfig,
        tts_config: SharedTtsConfig,
        voice_override: Option<String>,
    ) -> Self {
        Self {
            asr_config,
            tts_config,
            voice_override,
        }
    }

    /// 从 ASR + TTS 配置构建策略
    ///
    /// `voice_override` 可以覆盖配置中的音色（用于 CLI 参数 `--xiaozhi-tts-voice`）。
    ///
    /// ASR 配置通过 Arc<RwLock> 共享，Web API 保存时同步更新此对象，实现运行时热加载。
    pub fn from_config(
        asr_config: SharedAsrConfig,
        shared_tts_config: SharedTtsConfig,
        voice_override: Option<String>,
    ) -> Result<Self, String> {
        // 验证当前配置的凭证是否有效（构造时检查一次，运行时也会动态读取）
        {
            let cfg = asr_config.read().unwrap();
            match cfg.active_provider.as_str() {
                "doubao" => {
                    cfg.get_credential("api_key")
                        .ok_or_else(|| "ASR API Key 未配置（当前提供商: doubao）".to_string())?;
                }
                "xfyun" => {
                    cfg.get_credential("app_id")
                        .ok_or_else(|| "ASR app_id 未配置（当前提供商: xfyun）".to_string())?;
                    cfg.get_credential("api_key")
                        .ok_or_else(|| "ASR api_key 未配置（当前提供商: xfyun）".to_string())?;
                    cfg.get_credential("api_secret")
                        .ok_or_else(|| "ASR api_secret 未配置（当前提供商: xfyun）".to_string())?;
                }
                // qwen / glm / mimo 等使用 api_key
                _ => {
                    cfg.get_credential("api_key").ok_or_else(|| {
                        format!(
                            "ASR API Key 配置无效（当前提供商: {}）",
                            cfg.active_provider
                        )
                    })?;
                }
            }
        }

        Ok(Self {
            asr_config,
            tts_config: shared_tts_config,
            voice_override,
        })
    }

    /// 设置 Resource ID（声音克隆等场景）
    pub fn with_resource_id(self, resource_id: String) -> Self {
        if let Ok(mut cfg) = self.tts_config.write() {
            let active = cfg.active_provider.clone();
            cfg.providers
                .entry(active)
                .or_default()
                .insert("resource_id".to_string(), resource_id);
        }
        self
    }
}

#[async_trait]
impl ResponseStrategy for AsrTtsStrategy {
    fn name(&self) -> &'static str {
        "asr-tts"
    }

    /// 告知设备使用 24000Hz 播放（匹配 TTS 引擎输出）
    fn hello_audio_params(&self, _client_params: &AudioParams) -> AudioParams {
        AudioParams {
            format: "opus".into(),
            sample_rate: 24000,
            channels: 1,
            frame_duration: 60,
        }
    }

    /// 生成 ASR→TTS 响应
    ///
    /// 1. Opus 帧解码 (16kHz) → PCM
    /// 2. Doubao ASR 识别 → 文本
    /// 3. TTS Provider 合成 (24kHz) → PCM
    /// 4. PCM → Opus 编码 (24kHz, 60ms)
    /// 5. 封装 AudioFrame → 返回
    async fn generate_response(
        &self,
        audio_buffer: Vec<AudioFrame>,
        session_id: &str,
    ) -> Result<Vec<AudioFrame>, String> {
        // ── Step 0: 空缓冲区检查 ──
        if audio_buffer.is_empty() {
            tracing::warn!(
                session_id = %session_id,
                "ASR-TTS: 音频缓冲区为空，跳过处理",
            );
            return Ok(Vec::new());
        }

        // ── Step 1: Opus → PCM 解码 (16kHz, 60ms 帧) ──
        tracing::info!(
            session_id = %session_id,
            frame_count = audio_buffer.len(),
            "ASR-TTS: 开始 Opus 解码",
        );

        let pcm_16k = decode_opus_frames_to_pcm(&audio_buffer, 16000, 60)
            .map_err(|e| format!("Opus 解码失败: {}", e))?;

        let duration_ms = if pcm_16k.is_empty() {
            0
        } else {
            pcm_16k.len() as u64 * 1000 / (16000 * 2)
        };

        tracing::info!(
            session_id = %session_id,
            pcm_bytes = pcm_16k.len(),
            duration_ms = duration_ms,
            "ASR-TTS: Opus 解码完成",
        );

        if pcm_16k.is_empty() {
            tracing::warn!(
                session_id = %session_id,
                "ASR-TTS: 解码后 PCM 为空",
            );
            return Ok(Vec::new());
        }

        // ── Step 2: Doubao ASR 语音识别 ──
        tracing::info!(
            session_id = %session_id,
            "ASR-TTS: 开始语音识别",
        );

        // 从共享配置读取最新 ASR 凭证，动态创建提供商实例
        let asr = {
            let cfg = self.asr_config.read().unwrap();
            create_asr_provider(&cfg)?
        };

        let audio_stream = adapt_audio_input(AudioInput::Data(pcm_16k), DEFAULT_CHUNK_SIZE);

        let text = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            asr_listen_to_text(&*asr, audio_stream),
        )
        .await
        .map_err(|_| "ASR 识别超时 (30s)".to_string())?
        .map_err(|e| format!("ASR 识别失败: {}", e))?;

        if text.is_empty() {
            tracing::warn!(
                session_id = %session_id,
                "ASR-TTS: 识别结果为空（可能为静音或无有效语音）",
            );
            return Ok(Vec::new());
        }

        tracing::info!(
            session_id = %session_id,
            text = %text,
            "ASR-TTS: 识别完成",
        );

        // ── Step 3: TTS 语音合成 ──
        // 从共享配置读取最新 TTS 配置，叠加 CLI 音色覆盖
        // 如果 TTS 提供者创建或合成失败，返回内置「失败，请重试」提示音
        let frames = match async {
            // 在单独的块中获取并释放 TTS 配置锁，避免 RwLockReadGuard 跨越 .await
            let provider = {
                let cfg = self.tts_config.read().unwrap();
                let mut work_cfg = cfg.clone();
                if let Some(ref voice) = self.voice_override {
                    work_cfg
                        .providers
                        .entry(work_cfg.active_provider.clone())
                        .or_default()
                        .insert("voice".to_string(), voice.clone());
                }
                crate::tts_factory::create_tts_provider(&work_cfg)? // cfg 在此处释放
            };

            let response = provider
                .synthesize(TtsRequest {
                    text: text.clone(),
                    options: None,
                })
                .await
                .map_err(|e| format!("TTS 合成失败: {}", e))?;

            if response.audio.is_empty() {
                return Err::<Vec<AudioFrame>, String>("TTS 返回空音频".to_string());
            }

            // PCM → Opus 编码 (24kHz, 60ms)
            let opus_frames = pcm_to_opus_frames(&response.audio, 24000, 60)
                .map_err(|e| format!("Opus 编码失败: {}", e))?;

            // 封装为 AudioFrame
            let mut frames = Vec::with_capacity(opus_frames.len());
            let mut timestamp: u32 = 0;
            for opus in opus_frames {
                frames.push(AudioFrame {
                    timestamp,
                    data: opus,
                });
                timestamp = timestamp.wrapping_add(60);
            }

            Ok::<_, String>(frames)
        }
        .await
        {
            Ok(frames) => frames,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "TTS 合成失败，返回 fallback 提示音",
                );
                let opus_frames = crate::xiaozhi_tts::fallback_error_audio_frames()
                    .map_err(|e| format!("Fallback 音频编码失败: {}", e))?;
                let mut frames = Vec::with_capacity(opus_frames.len());
                let mut timestamp: u32 = 0;
                for opus in opus_frames {
                    frames.push(AudioFrame {
                        timestamp,
                        data: opus,
                    });
                    timestamp = timestamp.wrapping_add(60);
                }
                frames
            }
        };

        tracing::info!(
            session_id = %session_id,
            frame_count = frames.len(),
            "ASR-TTS: 管线完成",
        );

        Ok(frames)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Opus → PCM 解码
// ═══════════════════════════════════════════════════════════════════════════════

/// 将 Opus 帧列表解码为 PCM16 mono 数据
///
/// # 参数
///
/// * `frames` — Opus 裸包列表（无容器封装）
/// * `sample_rate` — 编码时的采样率 (Hz)，ESP32 上传为 16000
/// * `frame_duration_ms` — 每帧时长（毫秒），ESP32 上传为 60ms
///
/// # 返回
///
/// 连续 PCM16 LE mono 字节序列
fn decode_opus_frames_to_pcm(
    frames: &[AudioFrame],
    sample_rate: u32,
    frame_duration_ms: u32,
) -> Result<Vec<u8>, String> {
    if frame_duration_ms == 0 {
        return Err("frame_duration_ms 不能为 0".into());
    }
    if sample_rate == 0 {
        return Err("sample_rate 不能为 0".into());
    }

    // 每帧采样数: 60ms @ 16kHz = 960 samples
    let frame_samples = (sample_rate as u64 * frame_duration_ms as u64 / 1000) as usize;

    let mut decoder = Decoder::new(sample_rate, Channels::Mono)
        .map_err(|e| format!("创建 Opus 解码器失败: {}", e))?;

    let mut all_pcm: Vec<u8> = Vec::new();
    let mut pcm_buf = vec![0i16; frame_samples];

    for frame in frames {
        if frame.data.is_empty() {
            continue;
        }

        let decoded_samples = decoder
            .decode(&frame.data, &mut pcm_buf, false)
            .map_err(|e| format!("Opus 解码错误: {}", e))?;

        // 将 i16 采样转换为小端字节序
        for sample in &pcm_buf[..decoded_samples] {
            all_pcm.extend_from_slice(&sample.to_le_bytes());
        }
    }

    Ok(all_pcm)
}

// ═══════════════════════════════════════════════════════════════════════════════
// ASR 提供者工厂
// ═══════════════════════════════════════════════════════════════════════════════

/// 根据配置创建 ASR 提供者实例
///
/// 支持动态切换 ASR 提供商，通过 `AsrConfig.active_provider` 控制。
fn create_asr_provider(cfg: &AsrConfig) -> Result<Box<dyn AsrProvider>, String> {
    match cfg.active_provider.as_str() {
        "qwen" => {
            let api_key = cfg
                .get_credential("api_key")
                .ok_or_else(|| "ASR API Key 未配置（当前提供商: qwen）".to_string())?;
            Ok(Box::new(QwenAsr::new(QwenAsrOption {
                base: BaseProviderOption {
                    api_key: Some(api_key),
                    model: Some("paraformer-realtime-v2".into()),
                    language: Some("zh-CN".into()),
                    // xiaozhi 管道输出 PCM16 mono，必须显式设置格式（Qwen 默认 Mp3）
                    format: Some(AudioContainerFormat::Pcm),
                    ..Default::default()
                },
                sample_rate: Some(16000),
                enable_punctuation_prediction: Some(true),
                enable_inverse_text_normalization: Some(true),
                ..Default::default()
            })))
        }
        "glm" => {
            let api_key = cfg
                .get_credential("api_key")
                .ok_or_else(|| "ASR API Key 未配置（当前提供商: glm）".to_string())?;
            Ok(Box::new(GlmAsr::new(GlmAsrOption {
                base: BaseProviderOption {
                    api_key: Some(api_key),
                    language: Some("zh-CN".into()),
                    ..Default::default()
                },
                ..Default::default()
            })))
        }
        "mimo" => {
            let api_key = cfg
                .get_credential("api_key")
                .ok_or_else(|| "ASR API Key 未配置（当前提供商: mimo）".to_string())?;
            Ok(Box::new(MimoAsr::new(MimoAsrOption {
                base: BaseProviderOption {
                    api_key: Some(api_key),
                    ..Default::default()
                },
                language: Some("zh-CN".into()),
            })))
        }
        "xfyun" => {
            let app_id = cfg
                .get_credential("app_id")
                .ok_or_else(|| "ASR app_id 未配置（当前提供商: xfyun）".to_string())?;
            let api_key = cfg
                .get_credential("api_key")
                .ok_or_else(|| "ASR api_key 未配置（当前提供商: xfyun）".to_string())?;
            let api_secret = cfg
                .get_credential("api_secret")
                .ok_or_else(|| "ASR api_secret 未配置（当前提供商: xfyun）".to_string())?;
            Ok(Box::new(XfyunAsr::new(XfyunAsrOption {
                base: BaseProviderOption {
                    api_key: Some(api_key),
                    language: Some("zh-CN".into()),
                    ..Default::default()
                },
                app_id: Some(app_id),
                api_secret: Some(api_secret),
                sample_rate: Some(16000),
                ..Default::default()
            })))
        }
        _ => {
            // doubao（默认）
            let api_key = cfg
                .get_credential("api_key")
                .ok_or_else(|| "ASR API Key 未配置（当前提供商: doubao）".to_string())?;
            Ok(Box::new(DoubaoAsr::new(DoubaoAsrOption {
                base: BaseProviderOption {
                    language: Some("zh-CN".into()),
                    ..Default::default()
                },
                api_key: Some(api_key),
                mode: DoubaoAsrMode::Streaming,
                ..Default::default()
            })))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// ASR 流式识别 → 完整文本
// ═══════════════════════════════════════════════════════════════════════════════

/// 对音频流执行 ASR 识别，返回完整识别文本
async fn asr_listen_to_text(
    asr: &dyn AsrProvider,
    audio_stream: univoice::asr::AudioStream,
) -> Result<String, String> {
    let mut stream = asr
        .listen_stream(audio_stream)
        .await
        .map_err(|e| format!("ASR 启动失败: {}", e))?;

    let mut full_text = String::new();
    let mut chunk_count = 0;

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => {
                chunk_count += 1;
                if chunk.is_final && !chunk.text.is_empty() {
                    full_text.push_str(&chunk.text);
                }
            }
            Err(e) => {
                tracing::warn!("ASR 识别块错误: {}", e);
            }
        }
    }

    tracing::debug!(
        chunk_count = chunk_count,
        text_len = full_text.len(),
        "ASR 流式识别完成",
    );

    Ok(full_text)
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xiaozhi_tts::pcm_to_opus_frames;

    // ─── Opus 编解码往返测试 ───────────────────────────

    #[test]
    fn test_t1_opus_roundtrip() {
        // 生成测试 PCM 数据: 1 帧 = 960 samples * 2 bytes = 1920 bytes @ 16kHz 60ms
        let mut pcm_original = Vec::with_capacity(1920);
        for i in 0..960 {
            let val = ((i as f64 * 0.1).sin() * 10000.0) as i16;
            pcm_original.extend_from_slice(&val.to_le_bytes());
        }

        // 编码为 Opus
        let opus_frames = pcm_to_opus_frames(&pcm_original, 16000, 60).unwrap();
        assert!(!opus_frames.is_empty(), "Opus 编码不应返回空");
        assert_eq!(
            opus_frames.len(),
            1,
            "1 帧 16000Hz 60ms 应产生 1 个 Opus 包"
        );

        // 构建 AudioFrame
        let audio_frames = vec![AudioFrame {
            timestamp: 0,
            data: opus_frames[0].clone(),
        }];

        // 解码回 PCM
        let pcm_decoded = decode_opus_frames_to_pcm(&audio_frames, 16000, 60).unwrap();
        assert!(!pcm_decoded.is_empty(), "Opus 解码不应返回空");
        assert_eq!(
            pcm_decoded.len(),
            1920,
            "解码后应为 1920 bytes (960 samples * 2 bytes)"
        );
    }

    // ─── 解码边界测试 ──────────────────────────────────

    #[test]
    fn test_t2_decode_empty_frames() {
        let frames: Vec<AudioFrame> = vec![];
        let result = decode_opus_frames_to_pcm(&frames, 16000, 60);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_t3_decode_invalid_frame_duration() {
        let frames = vec![AudioFrame {
            timestamp: 0,
            data: vec![0x80, 0xFF],
        }];
        let result = decode_opus_frames_to_pcm(&frames, 16000, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_t4_decode_invalid_sample_rate() {
        let frames = vec![AudioFrame {
            timestamp: 0,
            data: vec![0x80, 0xFF],
        }];
        let result = decode_opus_frames_to_pcm(&frames, 0, 60);
        assert!(result.is_err());
    }

    #[test]
    fn test_t5_decode_skip_empty_data() {
        let frames = vec![
            AudioFrame {
                timestamp: 0,
                data: vec![],
            },
            AudioFrame {
                timestamp: 60,
                data: vec![],
            },
        ];
        let result = decode_opus_frames_to_pcm(&frames, 16000, 60);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_t6_decode_invalid_opus_data() {
        // 无效的 Opus 数据应返回错误
        let frames = vec![AudioFrame {
            timestamp: 0,
            data: vec![0xFF, 0xFF, 0xFF, 0xFF], // 无效 Opus
        }];
        let result = decode_opus_frames_to_pcm(&frames, 16000, 60);
        assert!(result.is_err());
    }

    // ─── Strategy 基本测试 ─────────────────────────────

    fn make_tts_config() -> crate::config::settings::TtsConfig {
        let mut providers = std::collections::HashMap::new();
        let mut creds = std::collections::HashMap::new();
        creds.insert("api_key".to_string(), "test-app-key".to_string());
        providers.insert("doubao".to_string(), creds);
        crate::config::settings::TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
            ..Default::default()
        }
    }

    fn make_asr_config() -> crate::config::settings::AsrConfig {
        let mut providers = std::collections::HashMap::new();
        let mut creds = std::collections::HashMap::new();
        creds.insert("api_key".to_string(), "test-app-key".to_string());
        providers.insert("doubao".to_string(), creds);
        crate::config::settings::AsrConfig {
            active_provider: "doubao".to_string(),
            providers,
        }
    }

    fn make_shared_tts_config() -> SharedTtsConfig {
        Arc::new(RwLock::new(make_tts_config()))
    }

    fn make_shared_asr_config() -> SharedAsrConfig {
        Arc::new(RwLock::new(make_asr_config()))
    }

    fn make_strategy() -> AsrTtsStrategy {
        AsrTtsStrategy::new(make_shared_asr_config(), make_shared_tts_config(), None)
    }

    #[test]
    fn test_t7_strategy_name() {
        let strategy = make_strategy();
        assert_eq!(strategy.name(), "asr-tts");
    }

    #[test]
    fn test_t8_hello_audio_params() {
        let strategy = make_strategy();
        let client_params = AudioParams {
            format: "opus".into(),
            sample_rate: 16000,
            channels: 1,
            frame_duration: 60,
        };
        let result = strategy.hello_audio_params(&client_params);
        assert_eq!(result.sample_rate, 24000);
        assert_eq!(result.format, "opus");
        assert_eq!(result.channels, 1);
        assert_eq!(result.frame_duration, 60);
    }

    #[test]
    fn test_t9_strategy_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AsrTtsStrategy>();
    }

    #[test]
    fn test_t10_with_resource_id() {
        let strategy = make_strategy().with_resource_id("seed-tts-1.0".into());
        // resource_id should be set in tts_config providers
        assert_eq!(
            strategy
                .tts_config
                .read()
                .unwrap()
                .get_credential("resource_id")
                .as_deref(),
            Some("seed-tts-1.0")
        );
    }

    // ─── ASR 提供者工厂测试 ──────────────────────────

    fn make_qwen_asr_config() -> crate::config::settings::AsrConfig {
        let mut providers = std::collections::HashMap::new();
        let mut creds = std::collections::HashMap::new();
        creds.insert("api_key".to_string(), "test-qwen-api-key".to_string());
        providers.insert("qwen".to_string(), creds);
        crate::config::settings::AsrConfig {
            active_provider: "qwen".to_string(),
            providers,
        }
    }

    #[test]
    fn test_t11_create_asr_provider_doubao() {
        let cfg = make_asr_config();
        let provider = create_asr_provider(&cfg).expect("doubao 提供者创建应成功");
        assert_eq!(provider.name(), "doubao");
    }

    #[test]
    fn test_t12_create_asr_provider_qwen() {
        let cfg = make_qwen_asr_config();
        let provider = create_asr_provider(&cfg).expect("qwen 提供者创建应成功");
        assert_eq!(provider.name(), "qwen");
    }

    #[test]
    fn test_t13_create_asr_provider_missing_creds() {
        let cfg = crate::config::settings::AsrConfig {
            active_provider: "qwen".to_string(),
            providers: std::collections::HashMap::new(),
        };
        let result = create_asr_provider(&cfg);
        match result {
            Err(e) => assert!(e.contains("API Key"), "错误信息应包含 API Key，得到: {}", e),
            Ok(_) => panic!("缺少凭证时应返回错误"),
        }
    }
}
