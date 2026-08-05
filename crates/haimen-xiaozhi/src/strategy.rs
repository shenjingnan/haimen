//! WebSocket 响应策略 — 决定录音结束后如何生成回放音频
//!
//! # 内置策略
//!
//! | 策略 | 行为 |
//! |------|------|
//! | [`EchoStrategy`] | 原样回传设备录制的音频（默认） |
//!
//! # 扩展
//!
//! 第三方可实现 [`ResponseStrategy`] trait 以自定义回放行为。
//! 例如 Phase 2 将新增 `TtsStrategy`，忽略设备音频，改为 TTS → PCM → Opus 下发。

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use crate::types::{AudioFrame, AudioParams, PlaybackEvent};

/// WebSocket 响应策略：决定录音结束后如何生成回放音频
///
/// # 流式 ASR 支持
///
/// 实现 [`supports_streaming_asr`] 返回 `true` 的策略可以启用流式 ASR 管道：
/// - [`on_recording_start`] — 在录音开始时被调用，用于启动 ASR 管道
/// - [`on_audio_frame`] — 每收到一帧 Opus 数据时被调用，策略可在此处解码并喂给 ASR
/// - [`generate_response`] — 录音结束时调用，对于流式策略此时 ASR 结果已就绪
///
/// 默认实现（Echo、TTS 等）的钩子均为 no-op，不受影响。
#[async_trait]
pub trait ResponseStrategy: Send + Sync {
    /// 策略名称（用于日志和诊断）
    fn name(&self) -> &'static str;

    /// 返回 HELLO 握手时应告知设备的音频参数
    ///
    /// 默认实现回传客户端发送的参数（Echo 模式适用）。
    /// TTS 模式应覆盖为 24000Hz 以匹配 TTS 引擎输出。
    ///
    /// # 参数
    ///
    /// * `client_params` — 设备端在 HELLO 消息中上报的音频参数
    fn hello_audio_params(&self, client_params: &AudioParams) -> AudioParams {
        client_params.clone()
    }

    /// 生成响应音频帧
    ///
    /// # 参数
    ///
    /// * `audio_buffer` — 设备录音阶段缓冲的音频帧
    ///   - [`EchoStrategy`] 使用这些帧原样回传
    ///   - `TtsStrategy` 将忽略此参数，改从 TTS 生成
    ///   - 流式 ASR 策略（如 `AsrLlmTtsStrategy`）也将忽略此参数，
    ///     因为音频已在 [`on_audio_frame`] 中实时处理
    /// * `session_id` — 当前 WebSocket 会话 ID，用于日志和状态关联
    ///
    /// # 返回
    ///
    /// * `Ok(Vec<AudioFrame>)` — 要播放给设备的音频帧
    /// * `Err(String)` — 错误描述，此时不会播放任何音频
    async fn generate_response(
        &self,
        audio_buffer: Vec<AudioFrame>,
        session_id: &str,
    ) -> Result<Vec<AudioFrame>, String>;

    // ────────── 流式 ASR 钩子（可选覆盖） ──────────

    /// 策略是否支持流式 ASR（录音期间实时识别）
    ///
    /// 返回 `true` 时，[`on_recording_start`] 和 [`on_audio_frame`] 将在录音期间被调用。
    /// 默认返回 `false`。
    fn supports_streaming_asr(&self) -> bool {
        false
    }

    /// 录音开始时调用（仅当 [`supports_streaming_asr`] 返回 `true` 时）
    ///
    /// 流式策略应在此方法中启动 ASR WebSocket 管道。
    /// 默认实现为 no-op。
    async fn on_recording_start(&self, _session_id: &str) -> Result<(), String> {
        Ok(())
    }

    /// 每收到一帧 Opus 数据时调用（仅当 [`supports_streaming_asr`] 返回 `true` 时）
    ///
    /// 流式策略应在此方法中解码 Opus 帧并喂入 ASR 管道。
    /// 注意：即使支持流式 ASR，音频仍会缓冲在 `audio_buffer` 中，
    /// 供 [`generate_response`] 在需要时使用（如作为后备）。
    /// 默认实现为 no-op。
    async fn on_audio_frame(&self, _frame: &AudioFrame) -> Result<(), String> {
        Ok(())
    }

    // ────────── 流式回放（边合成边播放） ──────────

    /// 策略是否支持流式回放（边合成边播放）
    ///
    /// 返回 `true` 时，[`generate_response_stream`] 将替代 [`generate_response`] 被调用，
    /// 通过 `frame_tx` 逐帧发送生成的音频，ws.rs 收到帧后立即发给设备。
    /// 默认返回 `false`。
    fn supports_streaming_playback(&self) -> bool {
        false
    }

    /// 流式生成回放事件并发送到回放管道
    ///
    /// 与 [`generate_response`] 不同，此方法不返回所有帧，而是通过 `frame_tx`
    /// 逐帧发送生成的[`PlaybackEvent`]（音频帧或文本消息）。调用方在 `frame_tx`
    /// 的 Receiver 端逐帧读取并发送给设备。
    ///
    /// 策略可通过发送文本事件（[`PlaybackEvent::Stt`] / [`PlaybackEvent::LlmSentence`]）
    /// 让设备屏幕显示 ASR 识别文本和 LLM 回复文本。
    ///
    /// # 参数
    ///
    /// * `audio_buffer` — 设备录音阶段缓冲的音频帧
    /// * `session_id` — 当前 WebSocket 会话 ID
    /// * `frame_tx` — 回放事件发送端，实现边合成边播放
    ///
    /// # 约定
    ///
    /// - 方法返回时，`frame_tx` 已被 drop（Receiver 端收到 None）
    /// - 返回前应确保所有必要的事件已发送完毕
    /// - 默认实现调用 [`generate_response`] 后将每帧包装为 [`PlaybackEvent::Audio`] 发送
    async fn generate_response_stream(
        &self,
        audio_buffer: Vec<AudioFrame>,
        session_id: &str,
        frame_tx: mpsc::Sender<PlaybackEvent>,
    ) -> Result<(), String> {
        let frames = self.generate_response(audio_buffer, session_id).await?;
        for frame in frames {
            if frame_tx.send(PlaybackEvent::Audio(frame)).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    // ────────── VAD 端点检测（语音结束信号） ──────────

    /// 返回 VAD 端点通知器
    ///
    /// 当 ASR 通过 VAD 检测到用户已说完话时，返回的 Notify 会被触发。
    /// ws.rs 收到此信号后可提前结束录音，立即进入回放处理阶段。
    ///
    /// 返回 `None` 表示不支持 VAD 信号（将依赖安全超时或 Listen::Stop 结束录音）。
    fn vad_completion(&self) -> Option<Arc<Notify>> {
        None
    }

    // ────────── 主动问候（可选覆盖） ──────────

    /// 设备唤醒问候：设备检测到唤醒词（`listen/detect`）时，服务端主动播报。
    ///
    /// 返回要播放的 [`AudioFrame`]（时间戳从 0 起、步长 60ms，与 [`generate_response`]
    /// 批处理路径产出格式一致，可直接喂给 `playback_frames`）；`None` 表示不播报。
    ///
    /// 实现方负责读取配置判断是否启用、TTS 合成、PCM → Opus 封装。
    /// 默认实现返回 `None`（不播报），现有策略不受影响。
    ///
    /// # 参数
    ///
    /// * `session_id` — 当前 WebSocket 会话 ID，用于日志和状态关联
    async fn wake_greeting(&self, _session_id: &str) -> Option<Vec<AudioFrame>> {
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 内置策略：Echo（原回声回放）
// ═══════════════════════════════════════════════════════════════════════════════

/// Echo 策略：将设备录制的音频原样回传播放
///
/// 这是当前默认行为，与重构前 `echo_playback` 逻辑一致。
pub struct EchoStrategy;

#[async_trait]
impl ResponseStrategy for EchoStrategy {
    fn name(&self) -> &'static str {
        "echo"
    }

    /// 直接返回缓冲的音频帧，不做任何转换
    async fn generate_response(
        &self,
        audio_buffer: Vec<AudioFrame>,
        _session_id: &str,
    ) -> Result<Vec<AudioFrame>, String> {
        Ok(audio_buffer)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(timestamp: u32, data: &[u8]) -> AudioFrame {
        AudioFrame {
            timestamp,
            data: data.to_vec(),
        }
    }

    fn make_audio_params() -> AudioParams {
        AudioParams {
            format: "opus".into(),
            sample_rate: 16000,
            channels: 1,
            frame_duration: 60,
        }
    }

    // ─── 基础测试 ──────────────────────────────────────────

    #[test]
    fn test_t1_echo_strategy_name() {
        let strategy = EchoStrategy;
        assert_eq!(strategy.name(), "echo");
    }

    #[test]
    fn test_t2_echo_hello_audio_params_default() {
        let strategy = EchoStrategy;
        let client_params = make_audio_params();
        let result = strategy.hello_audio_params(&client_params);
        // Echo 模式应原样回传客户端参数
        assert_eq!(result.format, "opus");
        assert_eq!(result.sample_rate, 16000);
        assert_eq!(result.channels, 1);
        assert_eq!(result.frame_duration, 60);
    }

    #[tokio::test]
    async fn test_t3_echo_strategy_returns_buffer_unchanged() {
        let strategy = EchoStrategy;
        let buffer = vec![
            make_frame(0, &[0x01, 0x02]),
            make_frame(60, &[0x03, 0x04]),
            make_frame(120, &[0x05, 0x06]),
        ];

        let result = strategy
            .generate_response(buffer.clone(), "test-session")
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), buffer);
    }

    #[tokio::test]
    async fn test_t4_echo_strategy_empty_buffer() {
        let strategy = EchoStrategy;
        let buffer: Vec<AudioFrame> = vec![];

        let result = strategy
            .generate_response(buffer.clone(), "test-session")
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_t6_echo_strategy_single_frame() {
        let strategy = EchoStrategy;
        let buffer = vec![make_frame(0, &[0xFF; 40])];

        let result = strategy
            .generate_response(buffer.clone(), "test-session")
            .await;

        assert!(result.is_ok());
        let frames = result.unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].timestamp, 0);
        assert_eq!(frames[0].data.len(), 40);
    }

    #[tokio::test]
    async fn test_t7_echo_strategy_large_buffer() {
        let strategy = EchoStrategy;
        let buffer: Vec<AudioFrame> = (0..100).map(|i| make_frame(i * 60, &[0xAA; 60])).collect();

        let result = strategy
            .generate_response(buffer.clone(), "large-session")
            .await;

        assert!(result.is_ok());
        let frames = result.unwrap();
        assert_eq!(frames.len(), 100);
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(frame.timestamp, (i as u32) * 60);
            assert_eq!(frame.data.len(), 60);
        }
    }

    // ─── Trait 约束测试 ────────────────────────────────────

    /// 验证 ResponseStrategy 满足 Send + Sync
    fn assert_send_sync<T: Send + Sync>() {}
    #[test]
    fn test_t8_echo_strategy_send_sync() {
        assert_send_sync::<EchoStrategy>();
    }

    // ─── Arc<dyn ResponseStrategy> 可用性 ──────────────────

    #[tokio::test]
    async fn test_t9_echo_strategy_via_trait_object() {
        let strategy: Arc<dyn ResponseStrategy> = Arc::new(EchoStrategy);
        assert_eq!(strategy.name(), "echo");

        let buffer = vec![make_frame(0, &[0x01; 10])];
        let result = strategy
            .generate_response(buffer.clone(), "trait-object")
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), buffer);
    }

    // ─── 流式 ASR 钩子默认行为（no-op） ──────────────────

    #[test]
    fn test_t10_streaming_defaults() {
        let strategy = EchoStrategy;
        assert!(
            !strategy.supports_streaming_asr(),
            "Echo 策略应不支持流式 ASR"
        );
    }

    #[tokio::test]
    async fn test_t11_on_recording_start_noop() {
        let strategy = EchoStrategy;
        let result = strategy.on_recording_start("test-session").await;
        assert!(result.is_ok(), "默认 on_recording_start 应为 Ok(())");
    }

    #[tokio::test]
    async fn test_t12_on_audio_frame_noop() {
        let strategy = EchoStrategy;
        let frame = make_frame(0, &[0x01, 0x02]);
        let result = strategy.on_audio_frame(&frame).await;
        assert!(result.is_ok(), "默认 on_audio_frame 应为 Ok(())");
    }

    /// 通过 trait object 调用流式钩子确保分发正确
    #[tokio::test]
    async fn test_t13_streaming_hooks_via_trait_object() {
        let strategy: Arc<dyn ResponseStrategy> = Arc::new(EchoStrategy);
        assert!(!strategy.supports_streaming_asr());

        let result = strategy.on_recording_start("test-session").await;
        assert!(result.is_ok());

        let frame = make_frame(0, &[0x01]);
        let result = strategy.on_audio_frame(&frame).await;
        assert!(result.is_ok());
    }

    /// 验证流式默认实现把帧包装为 PlaybackEvent::Audio 后逐帧发送
    #[tokio::test]
    async fn test_t14_generate_response_stream_emits_audio_events() {
        let strategy = EchoStrategy;
        let buffer = vec![make_frame(0, &[0x01, 0x02])];
        let (tx, mut rx) = tokio::sync::mpsc::channel::<PlaybackEvent>(16);

        let result = strategy
            .generate_response_stream(buffer, "test-session", tx)
            .await;
        assert!(result.is_ok(), "默认 generate_response_stream 应返回 Ok");

        let event = rx.recv().await.expect("应收到一个事件");
        match event {
            PlaybackEvent::Audio(frame) => {
                assert_eq!(frame.timestamp, 0);
                assert_eq!(frame.data, vec![0x01, 0x02]);
            }
            other => panic!("默认实现应发送 Audio 事件，得到 {:?}", other),
        }
        assert!(rx.recv().await.is_none(), "发送端 drop 后应收到 None");
    }

    // ─── 唤醒问候默认行为（no-op） ──────────────────────

    #[tokio::test]
    async fn test_t15_wake_greeting_default_none() {
        let strategy = EchoStrategy;
        let result = strategy.wake_greeting("test-session").await;
        assert!(result.is_none(), "默认 wake_greeting 应返回 None（不播报）");
    }

    /// 通过 trait object 调用 wake_greeting 确保 vtable 分发正确
    #[tokio::test]
    async fn test_t16_wake_greeting_via_trait_object() {
        let strategy: Arc<dyn ResponseStrategy> = Arc::new(EchoStrategy);
        let result = strategy.wake_greeting("trait-object").await;
        assert!(result.is_none(), "trait object 分发后应仍返回 None");
    }
}
