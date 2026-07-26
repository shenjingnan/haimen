//! xiaozhi-esp32 TTS 响应策略
//!
//! 将预设文本通过当前激活的 TTS Provider 合成为音频，编码为 Opus 帧后发送给设备播放。
//!
//! # 管线
//!
//! ```text
//! 预设文本
//!   ↓ TtsProvider::synthesize(format="pcm", sample_rate=24000)
//! PCM16 mono 24000Hz (Vec<u8>)
//!   ↓ pcm_to_opus_frames() 按 60ms 分帧
//! Vec<OpusPacket>
//!   ↓ 封装为 AudioFrame { timestamp, data }
//! play_back_frames() (已有复用)
//!   ↓ BinaryProtocol2 → 设备播放
//! ```

use async_trait::async_trait;
use haimen_xiaozhi::{AudioFrame, AudioParams, ResponseStrategy};
use opus2::{self, Application, Channels};
use univoice::tts::TtsRequest;

use crate::config::settings::TtsConfig;

/// TTS 响应策略：忽略用户录音，将预设文本合成语音发送给设备
///
/// 通过 `TtsConfig.active_provider` 自动选择使用的 TTS 提供商。
pub struct TtsStrategy {
    /// 要转成语音的文本
    text: String,
    /// TTS 音色覆盖（优先级高于配置）
    voice: Option<String>,
    /// TTS 配置（包含活跃提供商和凭证）
    tts_config: TtsConfig,
}

impl TtsStrategy {
    /// 创建 TTS 策略
    ///
    /// 音色未指定时会从 `tts_config` 或默认值读取。
    pub fn new(text: String, voice: Option<String>, tts_config: TtsConfig) -> Self {
        Self {
            text,
            voice,
            tts_config,
        }
    }

    /// 从 TTS 配置构建策略
    ///
    /// `voice_override` 可以覆盖配置中的音色（用于 CLI 参数 `--xiaozhi-tts-voice`）。
    pub fn from_config(text: String, voice_override: Option<String>, tts: &TtsConfig) -> Self {
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

        Self {
            text,
            voice: voice_override,
            tts_config,
        }
    }
}

#[async_trait]
impl ResponseStrategy for TtsStrategy {
    fn name(&self) -> &'static str {
        "tts"
    }

    /// TTS 模式告知设备使用 24000Hz 播放（匹配 TTS 引擎输出）
    fn hello_audio_params(&self, _client_params: &AudioParams) -> AudioParams {
        AudioParams {
            format: "opus".into(),
            sample_rate: 24000,
            channels: 1,
            frame_duration: 60,
        }
    }

    /// 生成 TTS 响应：忽略用户录音，通过 TTS 合成语音
    async fn generate_response(
        &self,
        _audio_buffer: Vec<AudioFrame>,
        session_id: &str,
    ) -> Result<Vec<AudioFrame>, String> {
        // 1. 通过工厂创建 TTS Provider
        let tts = crate::tts_factory::create_tts_provider(&self.tts_config)?;

        tracing::info!(
            session_id = %session_id,
            text = %self.text,
            voice = ?self.voice,
            provider = %self.tts_config.active_provider,
            "TTS: 开始语音合成",
        );

        // 2. TTS 合成 → PCM
        let response = tts
            .synthesize(TtsRequest {
                text: self.text.clone(),
                options: None,
            })
            .await
            .map_err(|e| format!("TTS 合成失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            audio_size = response.audio.len(),
            format = %response.format,
            "TTS: 合成完成，开始编码 Opus",
        );

        if response.audio.is_empty() {
            tracing::warn!(
                session_id = %session_id,
                "TTS: 返回空音频数据",
            );
            return Ok(Vec::new());
        }

        // 3. PCM → Opus 帧（60ms 帧，24kHz，16-bit mono）
        let opus_frames = pcm_to_opus_frames(&response.audio, 24000, 60)
            .map_err(|e| format!("Opus 编码失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            frame_count = opus_frames.len(),
            "TTS: Opus 编码完成",
        );

        // 4. 封装为 AudioFrame，时间戳从 0 开始累加 60ms
        let mut frames = Vec::with_capacity(opus_frames.len());
        let mut timestamp: u32 = 0;
        for opus in opus_frames {
            frames.push(AudioFrame {
                timestamp,
                data: opus,
            });
            timestamp = timestamp.wrapping_add(60);
        }

        Ok(frames)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PCM → Opus 非流式编码
// ═══════════════════════════════════════════════════════════════════════════════

/// 将 PCM16 mono 数据按固定帧长编码为裸 Opus 包
///
/// # 参数
///
/// * `pcm` — 完整 PCM16 little-endian mono 数据
/// * `sample_rate` — PCM 采样率（Hz）
/// * `frame_duration_ms` — 每帧时长（毫秒，必须为 Opus 支持的值）
///
/// # 返回
///
/// 每个元素为一个裸 Opus 包（无 OGG 容器封装）
pub(crate) fn pcm_to_opus_frames(
    pcm: &[u8],
    sample_rate: u32,
    frame_duration_ms: u32,
) -> Result<Vec<Vec<u8>>, String> {
    if frame_duration_ms == 0 {
        return Err("frame_duration_ms 不能为 0".into());
    }
    if sample_rate == 0 {
        return Err("sample_rate 不能为 0".into());
    }
    let frame_samples = (sample_rate as u64 * frame_duration_ms as u64 / 1000) as usize;
    let frame_bytes = frame_samples * 2; // 16-bit

    let mut encoder = opus2::Encoder::new(sample_rate, Channels::Mono, Application::Audio)
        .map_err(|e| format!("创建 Opus 编码器失败: {}", e))?;

    // 最高编码质量（因为是离线合成，非实时流）
    let _ = encoder.set_complexity(10);
    // 启用 VBR（可变比特率，节省带宽）
    let _ = encoder.set_vbr(true);
    // 启用 DTX（静音检测）
    let _ = encoder.set_dtx(true);

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut opus_buf = vec![0u8; 4000]; // Opus 最大包大小

    for chunk in pcm.chunks(frame_bytes) {
        let mut frame = chunk.to_vec();
        if frame.len() < frame_bytes {
            // 尾部不足一帧，零填充
            frame.resize(frame_bytes, 0);
        }

        // PCM i16le → &[i16]
        let pcm_i16: Vec<i16> = frame
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        let encoded_len = encoder
            .encode(&pcm_i16, &mut opus_buf)
            .map_err(|e| format!("Opus 编码错误: {}", e))?;

        frames.push(opus_buf[..encoded_len].to_vec());
    }

    Ok(frames)
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ─── PCM→Opus 编码测试 ─────────────────────────────────

    #[test]
    fn test_t1_pcm_to_opus_empty_input() {
        let result = pcm_to_opus_frames(&[], 24000, 60);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_t2_pcm_to_opus_single_frame() {
        // 60ms @ 24kHz 16-bit mono = 2880 bytes
        let pcm = vec![0u8; 2880];
        let result = pcm_to_opus_frames(&pcm, 24000, 60).unwrap();
        assert_eq!(result.len(), 1);
        // 静音的 Opus 包应该很小
        assert!(result[0].len() < 100);
        assert!(!result[0].is_empty());
    }

    #[test]
    fn test_t3_pcm_to_opus_multi_frame() {
        // 3 帧 = 8640 bytes
        let pcm = vec![0u8; 8640];
        let result = pcm_to_opus_frames(&pcm, 24000, 60).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_t4_pcm_to_opus_partial_last_frame() {
        // 2.5 帧 = 7200 bytes → 应产生 3 帧（最后半帧零填充）
        let pcm = vec![0xFFu8; 7200];
        let result = pcm_to_opus_frames(&pcm, 24000, 60).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_t5_pcm_to_opus_different_rate() {
        // 20ms @ 16kHz 16-bit mono = 640 bytes
        let pcm = vec![0u8; 640];
        let result = pcm_to_opus_frames(&pcm, 16000, 20).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_t6_pcm_to_opus_invalid_frame_duration() {
        let pcm = vec![0u8; 2880];
        // 帧时长为 0 会导致除零错误
        let result = pcm_to_opus_frames(&pcm, 24000, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_t7_opus_output_not_empty() {
        // 非静音 PCM 数据应产生正常的 Opus 包
        let mut pcm = Vec::with_capacity(2880);
        for i in 0..1440 {
            // 使用正弦波避免溢出（i16 范围: -32768..32767）
            let val = ((i as f64 * 0.1).sin() * 10000.0) as i16;
            pcm.extend_from_slice(&val.to_le_bytes());
        }
        let result = pcm_to_opus_frames(&pcm, 24000, 60).unwrap();
        assert_eq!(result.len(), 1);
        // 非静音 Opus 包应该比静音的大
        assert!(result[0].len() > 10);
    }

    // ─── TtsStrategy 基本测试 ───────────────────────────────

    fn make_tts_config() -> crate::config::settings::TtsConfig {
        let mut providers = std::collections::HashMap::new();
        let mut creds = std::collections::HashMap::new();
        creds.insert("app_key".to_string(), "test-app-key".to_string());
        creds.insert("access_token".to_string(), "test-access-token".to_string());
        providers.insert("doubao".to_string(), creds);
        crate::config::settings::TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
            ..Default::default()
        }
    }

    #[test]
    fn test_t8_strategy_name() {
        let strategy = TtsStrategy::new("test".into(), None, make_tts_config());
        assert_eq!(strategy.name(), "tts");
    }

    #[test]
    fn test_t9_hello_audio_params() {
        let strategy = TtsStrategy::new("test".into(), None, make_tts_config());
        let client_params = AudioParams {
            format: "opus".into(),
            sample_rate: 16000,
            channels: 1,
            frame_duration: 60,
        };
        let result = strategy.hello_audio_params(&client_params);
        // TTS 模式应返回 24000Hz
        assert_eq!(result.sample_rate, 24000);
        assert_eq!(result.format, "opus");
        assert_eq!(result.channels, 1);
        assert_eq!(result.frame_duration, 60);
    }

    #[test]
    fn test_t10_strategy_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TtsStrategy>();
    }
}
