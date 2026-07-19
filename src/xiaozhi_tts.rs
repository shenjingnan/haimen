//! xiaozhi-esp32 TTS 响应策略
//!
//! 将预设文本通过 Doubao TTS 合成为音频，编码为 Opus 帧后发送给设备播放。
//!
//! # 管线
//!
//! ```text
//! 预设文本
//!   ↓ DoubaoTts::synthesize(format="pcm", sample_rate=24000)
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
use univoice::tts::provider::{DoubaoTts, DoubaoTtsOption};
use univoice::tts::{BaseTtsOption, TtsProvider, TtsRequest};

/// TTS 响应策略：忽略用户录音，将预设文本合成语音发送给设备
///
/// # 关于音色和 Resource ID
///
/// 火山引擎 TTS 的 `resource_id` 要与音色匹配：
/// - `seed-tts-1.0` — 用于经典 V1 音色（moon_bigtts, mars_bigtts 等）
/// - `seed-tts-2.0` — 用于 V2 音色（uranus_bigtts, jupiter_bigtts 等）
///
/// 当 `cluster` 为 `volcano_icl` 时 → `seed-tts-1.0`
/// 其他 cluster 值（含默认）→ `seed-tts-2.0`
pub struct TtsStrategy {
    /// 要转成语音的文本
    text: String,
    /// TTS 音色（None 使用环境变量 DOUBAO_VOICE_TYPE 或 Doubao V2 默认值）
    voice: Option<String>,
    /// 火山引擎 App Key
    app_key: String,
    /// 火山引擎 Access Token
    access_token: String,
    /// 火山引擎 Resource ID（None 后由 cluster 推导）
    resource_id: Option<String>,
    /// 火山引擎 Cluster（用于推导 resource_id）
    cluster: Option<String>,
}

impl TtsStrategy {
    /// 创建 TTS 策略
    ///
    /// 音色和 cluster 未指定时会从环境变量 `DOUBAO_VOICE_TYPE` / `DOUBAO_CLUSTER` 读取。
    pub fn new(text: String, voice: Option<String>, app_key: String, access_token: String) -> Self {
        // 音色：CLI 参数 > 环境变量 > univoice 默认
        let voice = voice
            .or_else(|| std::env::var("DOUBAO_VOICE_TYPE").ok())
            .or_else(|| Some("zh_female_xiaohe_uranus_bigtts".into()));
        // cluster：环境变量
        let cluster = std::env::var("DOUBAO_CLUSTER").ok();

        Self {
            text,
            voice,
            app_key,
            access_token,
            resource_id: None,
            cluster,
        }
    }

    /// 设置 Resource ID（声音克隆等场景）
    pub fn with_resource_id(mut self, resource_id: String) -> Self {
        self.resource_id = Some(resource_id);
        self
    }

    /// 将 cluster 映射为 resource_id
    ///
    /// 与 TypeScript `mapClusterToResourceId` 逻辑一致：
    /// - `volcano_icl` → `seed-tts-1.0`（声音克隆）
    /// - 其他 → `seed-tts-2.0`
    fn resolve_resource_id(&self) -> String {
        if let Some(ref rid) = self.resource_id {
            return rid.clone();
        }
        match self.cluster.as_deref() {
            Some("volcano_icl") => "seed-tts-1.0".into(),
            _ => "seed-tts-2.0".into(),
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
        // 1. TTS 合成 → PCM
        let resource_id = self.resolve_resource_id();
        let voice = self.voice.clone().map(Into::into);

        tracing::info!(
            session_id = %session_id,
            text = %self.text,
            voice = ?voice,
            resource_id = %resource_id,
            cluster = ?self.cluster,
            "TTS: 开始语音合成",
        );

        let tts = DoubaoTts::new(DoubaoTtsOption {
            base: BaseTtsOption {
                format: Some("pcm".into()),
                voice,
                ..Default::default()
            },
            app_id: Some(self.app_key.clone()),
            access_token: Some(self.access_token.clone()),
            resource_id: Some(resource_id),
            ..Default::default()
        });

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

        // 2. PCM → Opus 帧（60ms 帧，24kHz，16-bit mono）
        let opus_frames = pcm_to_opus_frames(&response.audio, 24000, 60)
            .map_err(|e| format!("Opus 编码失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            frame_count = opus_frames.len(),
            "TTS: Opus 编码完成",
        );

        // 3. 封装为 AudioFrame，时间戳从 0 开始累加 60ms
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

    #[test]
    fn test_t8_strategy_name() {
        let strategy =
            TtsStrategy::new("test".into(), None, "app_key".into(), "access_token".into());
        assert_eq!(strategy.name(), "tts");
    }

    #[test]
    fn test_t9_hello_audio_params() {
        let strategy =
            TtsStrategy::new("test".into(), None, "app_key".into(), "access_token".into());
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

    #[test]
    fn test_t11_with_resource_id() {
        let strategy = TtsStrategy::new("t".into(), None, "k".into(), "t".into())
            .with_resource_id("seed-tts-1.0".into());
        assert_eq!(strategy.resource_id, Some("seed-tts-1.0".into()));
    }

    #[test]
    fn test_t12_resolve_resource_id_default() {
        // 无 cluster → seed-tts-2.0
        let strategy = TtsStrategy::new("t".into(), None, "k".into(), "t".into());
        assert_eq!(strategy.resolve_resource_id(), "seed-tts-2.0");
    }

    #[test]
    fn test_t13_resolve_resource_id_custom() {
        let strategy = TtsStrategy::new("t".into(), None, "k".into(), "t".into())
            .with_resource_id("custom-resource".into());
        assert_eq!(strategy.resolve_resource_id(), "custom-resource");
    }

    #[test]
    fn test_t14_voice_default_when_none() {
        // 当未设置 DOUBAO_VOICE_TYPE 环境变量时，应使用 V2 默认音色
        let strategy = TtsStrategy::new("t".into(), None, "k".into(), "t".into());
        assert_eq!(
            strategy.voice.as_deref(),
            Some("zh_female_xiaohe_uranus_bigtts")
        );
    }

    #[test]
    fn test_t15_voice_uses_env_var() {
        // 设置 DOUBAO_VOICE_TYPE 环境变量
        unsafe {
            std::env::set_var("DOUBAO_VOICE_TYPE", "zh_female_vv_uranus_bigtts");
        }
        let strategy = TtsStrategy::new("t".into(), None, "k".into(), "t".into());
        assert_eq!(
            strategy.voice.as_deref(),
            Some("zh_female_vv_uranus_bigtts")
        );
        unsafe {
            std::env::remove_var("DOUBAO_VOICE_TYPE");
        }
    }

    #[test]
    fn test_t16_voice_cli_overrides_env() {
        unsafe {
            std::env::set_var("DOUBAO_VOICE_TYPE", "env_voice");
        }
        let strategy =
            TtsStrategy::new("t".into(), Some("cli_voice".into()), "k".into(), "t".into());
        assert_eq!(strategy.voice.as_deref(), Some("cli_voice"));
        unsafe {
            std::env::remove_var("DOUBAO_VOICE_TYPE");
        }
    }
}
