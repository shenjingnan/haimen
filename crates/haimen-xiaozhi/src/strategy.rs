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

use crate::types::{AudioFrame, AudioParams};

/// WebSocket 响应策略：决定录音结束后如何生成回放音频
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
}
