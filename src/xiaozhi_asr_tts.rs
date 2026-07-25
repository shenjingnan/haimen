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

use async_trait::async_trait;
use futures_util::StreamExt;
use haimen_xiaozhi::{AudioFrame, AudioParams, ResponseStrategy};
use opus2::{Channels, Decoder};
use univoice::asr::{
    AsrProvider, AudioInput, BaseProviderOption, DEFAULT_CHUNK_SIZE, DoubaoAsr, DoubaoAsrMode,
    DoubaoAsrOption, adapt_audio_input,
};
use univoice::tts::TtsRequest;

use crate::config::settings::TtsConfig;

use crate::xiaozhi_tts::pcm_to_opus_frames;

/// ASR→TTS 响应策略：将设备录制的语音识别为文字，再合成为语音回传
///
/// 管线：Opus 解码 (16kHz) → Doubao ASR → TTS Provider (24kHz) → Opus 编码
pub struct AsrTtsStrategy {
    /// 火山引擎 App Key（ASR 使用）
    app_key: String,
    /// 火山引擎 Access Token（ASR 使用）
    access_token: String,
    /// TTS 配置（包含活跃提供商和凭证）
    tts_config: TtsConfig,
}

impl AsrTtsStrategy {
    /// 创建 ASR→TTS 策略
    pub fn new(app_key: String, access_token: String, tts_config: TtsConfig) -> Self {
        Self {
            app_key,
            access_token,
            tts_config,
        }
    }

    /// 从 ASR + TTS 配置构建策略
    ///
    /// `voice_override` 可以覆盖配置中的音色（用于 CLI 参数 `--xiaozhi-tts-voice`）。
    pub fn from_config(
        asr: &crate::config::settings::AsrConfig,
        tts: &TtsConfig,
        voice_override: Option<String>,
    ) -> Result<Self, String> {
        let app_key = asr.resolved_app_key()?;
        let access_token = asr.resolved_access_token()?;

        // 用 CLI 覆盖的音色生成一个修改后的 TtsConfig
        let tts_config = if let Some(ref v) = voice_override {
            let mut cfg = tts.clone();
            cfg.providers
                .entry(cfg.active_provider.clone())
                .or_default()
                .insert("voice".to_string(), v.clone());
            cfg
        } else {
            tts.clone()
        };

        Ok(Self {
            app_key,
            access_token,
            tts_config,
        })
    }

    /// 设置 Resource ID（声音克隆等场景）
    pub fn with_resource_id(mut self, resource_id: String) -> Self {
        self.tts_config
            .providers
            .entry(self.tts_config.active_provider.clone())
            .or_default()
            .insert("resource_id".to_string(), resource_id);
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

        let asr = DoubaoAsr::new(DoubaoAsrOption {
            base: BaseProviderOption {
                language: Some("zh-CN".into()),
                ..Default::default()
            },
            app_key: Some(self.app_key.clone()),
            access_key: Some(self.access_token.clone()),
            mode: DoubaoAsrMode::Streaming,
            ..Default::default()
        });

        let audio_stream = adapt_audio_input(AudioInput::Data(pcm_16k), DEFAULT_CHUNK_SIZE);

        let text = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            asr_listen_to_text(&asr, audio_stream),
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
        let tts = crate::tts_factory::create_tts_provider(&self.tts_config)
            .map_err(|e| format!("创建 TTS 提供者失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            text = %text,
            provider = %self.tts_config.active_provider,
            "ASR-TTS: 开始语音合成",
        );

        let response = tts
            .synthesize(TtsRequest {
                text: text.clone(),
                options: None,
            })
            .await
            .map_err(|e| format!("TTS 合成失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            audio_size = response.audio.len(),
            format = %response.format,
            "ASR-TTS: 合成完成",
        );

        if response.audio.is_empty() {
            tracing::warn!(
                session_id = %session_id,
                "ASR-TTS: TTS 返回空音频",
            );
            return Ok(Vec::new());
        }

        // ── Step 4: PCM → Opus 编码 (24kHz, 60ms) ──
        let opus_frames = pcm_to_opus_frames(&response.audio, 24000, 60)
            .map_err(|e| format!("Opus 编码失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            frame_count = opus_frames.len(),
            "ASR-TTS: Opus 编码完成",
        );

        // ── Step 5: 封装为 AudioFrame ──
        let mut frames = Vec::with_capacity(opus_frames.len());
        let mut timestamp: u32 = 0;
        for opus in opus_frames {
            frames.push(AudioFrame {
                timestamp,
                data: opus,
            });
            timestamp = timestamp.wrapping_add(60);
        }

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
// ASR 流式识别 → 完整文本
// ═══════════════════════════════════════════════════════════════════════════════

/// 对音频流执行 ASR 识别，返回完整识别文本
async fn asr_listen_to_text(
    asr: &DoubaoAsr,
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
        creds.insert("app_key".to_string(), "test-app-key".to_string());
        creds.insert("access_token".to_string(), "test-access-token".to_string());
        providers.insert("doubao".to_string(), creds);
        crate::config::settings::TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
        }
    }

    #[test]
    fn test_t7_strategy_name() {
        let strategy =
            AsrTtsStrategy::new("app_key".into(), "access_token".into(), make_tts_config());
        assert_eq!(strategy.name(), "asr-tts");
    }

    #[test]
    fn test_t8_hello_audio_params() {
        let strategy =
            AsrTtsStrategy::new("app_key".into(), "access_token".into(), make_tts_config());
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
        let strategy = AsrTtsStrategy::new("k".into(), "t".into(), make_tts_config())
            .with_resource_id("seed-tts-1.0".into());
        // resource_id should be set in tts_config providers
        assert_eq!(
            strategy.tts_config.get_credential("resource_id").as_deref(),
            Some("seed-tts-1.0")
        );
    }
}
