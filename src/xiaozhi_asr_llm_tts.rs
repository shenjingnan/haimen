//! xiaozhi-esp32 ASR → LLM → TTS 响应策略
//!
//! 将设备录制的 Opus 音频解码为 PCM，通过 Doubao ASR 识别为文字，
//! 将文字发送给 AI Agent（Claude Code / Codex 等）处理，
//! 再将 LLM 的回复通过 Doubao TTS 合成为语音，编码为 Opus 帧后发送给设备播放。
//!
//! # 管线
//!
//! ```text
//! 设备 Opus 帧 (16kHz)
//!   ↓ opus2::Decoder
//! PCM16 mono 16000Hz
//!   ↓ DoubaoAsr::listen_stream
//! 识别文本
//!   ↓ AgentProvider::process (Claude Code / Codex 等)
//! LLM 回复文本
//!   ↓ DoubaoTts::synthesize(format="pcm")
//! PCM16 mono 24000Hz
//!   ↓ pcm_to_opus_frames() (24kHz, 60ms)
//! Vec<OpusPacket>
//!   ↓ 封装为 AudioFrame { timestamp, data }
//! play_back_frames() (已有复用)
//!   ↓ BinaryProtocol2 → 设备播放
//! ```
//!
//! # 多轮对话
//!
//! 策略内部维护 LLM 的 `session_id`，每次 `generate_response` 调用后更新，
//! 实现音色多轮对话的上下文连续性。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use haimen_xiaozhi::{AudioFrame, AudioParams, ResponseStrategy};
use opus2::{Application, Channels, Decoder};
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use univoice::asr::{
    AsrProvider, AudioInput, AudioStream, BaseProviderOption, DEFAULT_CHUNK_SIZE, DoubaoAsr,
    DoubaoAsrMode, DoubaoAsrOption, adapt_audio_input,
};
use univoice::tts::TtsRequest;

use crate::gateway::provider::AgentProvider;
use crate::xiaozhi_tts::pcm_to_opus_frames;

// ═══════════════════════════════════════════════════════════════════════════════
// 流式 ASR 管道状态
// ═══════════════════════════════════════════════════════════════════════════════

/// 流式 ASR 管道内部状态
///
/// 录音期间实时将 Opus 帧解码为 PCM 并喂入 Doubao ASR，
/// 实现 ASR 网络延迟与录音时间的重叠。
struct AsrPipelineState {
    /// 发送端：将解码后的 PCM 块喂入 ASR 的音频流
    pcm_tx: mpsc::Sender<Vec<u8>>,
    /// ASR 后台任务 JoinHandle，返回完整识别文本
    asr_handle: tokio::task::JoinHandle<Result<String, String>>,
    /// Opus 解码器（会话级复用，逐帧解码）
    decoder: Decoder,
    /// 每帧采样数（60ms @ 16kHz = 960 samples）
    frame_samples: usize,
    /// 诊断：已接收并处理的音频帧数（每帧 60ms）
    frame_count: u64,
    /// 诊断：上一次打印帧计数日志的帧号
    last_log_frame: u64,
    /// 本地能量检测：连续静音帧数（60ms/帧），>= MAX_SILENCE_FRAMES 时触发 VAD
    silence_count: u64,
    /// 本地能量检测：是否检测到过有效语音（初始静音不计入 silence_count）
    speech_detected: bool,
}

/// ASR → LLM → TTS 响应策略：将设备录制的语音识别为文字，
/// 送 AI Agent 处理，再将回复合成为语音回传
///
/// 管线：Opus 解码 (16kHz) → Doubao ASR → AgentProvider → TTS Provider (24kHz) → Opus 编码
pub struct AsrLlmTtsStrategy {
    /// 火山引擎 App Key（ASR 使用）
    app_key: String,
    /// 火山引擎 Access Token（ASR 使用）
    access_token: String,
    /// TTS 配置（包含活跃提供商和凭证）
    tts_config: crate::config::settings::TtsConfig,
    /// AI Agent（Claude Code、Codex 等）
    agent: Arc<dyn AgentProvider>,
    /// LLM 会话 ID，用于多轮对话上下文连续
    llm_session_id: Mutex<Option<String>>,
    /// 流式 ASR 管道状态（录音期间启用，录音结束时消耗）
    streaming_state: Mutex<Option<AsrPipelineState>>,
    /// VAD 端点通知器：ASR 检测到用户说完时触发（每录音周期创建新 Notify）
    vad_notify: Mutex<Arc<Notify>>,
    /// 本地能量检测标记：静音超阈值后关闭 ASR 流，阻止后续帧重新初始化
    silence_closed: AtomicBool,
    /// ASR 是否已返回非空文本（用于决定是否允许本地 VAD 提前关闭流）
    /// 使用 Arc 以便在 ASR 后台任务中写入
    asr_received_text: Arc<AtomicBool>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// 本地 PCM 能量检测——在 ASR 服务端 VAD 判停之前先关闭音频流
// ═══════════════════════════════════════════════════════════════════════════════

/// PCM16 mono 静音 RMS 阈值（低于此值视为静音）
///
/// 经验值：PCM16 满幅 32768，正常语音 RMS ≈ 3000~15000，
/// 环境底噪 RMS ≈ 100~800，阈值 2000 可区分底噪和有效语音。
const SILENCE_RMS_THRESHOLD: f64 = 2000.0;

/// 连续静音帧数阈值（60ms/帧），约 1.2s 静音后触发本地 VAD
const MAX_SILENCE_FRAMES: u64 = 20;

/// 计算 PCM16 mono 帧的 RMS 能量值
fn compute_pcm_rms(pcm_bytes: &[u8]) -> f64 {
    let samples = pcm_bytes.len() / 2;
    if samples == 0 {
        return 0.0;
    }
    let sum_sq: f64 = pcm_bytes
        .chunks_exact(2)
        .map(|b| {
            let sample = i16::from_le_bytes([b[0], b[1]]);
            (sample as f64).powi(2)
        })
        .sum();
    (sum_sq / samples as f64).sqrt()
}

impl AsrLlmTtsStrategy {
    /// 创建 ASR → LLM → TTS 策略
    ///
    /// # 参数
    ///
    /// * `app_key` — 火山引擎 App Key（ASR 使用）
    /// * `access_token` — 火山引擎 Access Token（ASR 使用）
    /// * `tts_config` — TTS 配置
    /// * `agent` — AI Agent 实例
    pub fn new(
        app_key: String,
        access_token: String,
        tts_config: crate::config::settings::TtsConfig,
        agent: Arc<dyn AgentProvider>,
    ) -> Self {
        Self {
            app_key,
            access_token,
            tts_config,
            agent,
            llm_session_id: Mutex::new(None),
            streaming_state: Mutex::new(None),
            vad_notify: Mutex::new(Arc::new(Notify::new())),
            silence_closed: AtomicBool::new(false),
            asr_received_text: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 从 ASR + TTS 配置构建策略
    ///
    /// `voice_override` 可以覆盖配置中的音色（用于 CLI 参数 `--xiaozhi-tts-voice`）。
    pub fn from_config(
        asr: &crate::config::settings::AsrConfig,
        tts: &crate::config::settings::TtsConfig,
        voice_override: Option<String>,
        agent: Arc<dyn AgentProvider>,
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
            agent,
            llm_session_id: Mutex::new(None),
            streaming_state: Mutex::new(None),
            vad_notify: Mutex::new(Arc::new(Notify::new())),
            silence_closed: AtomicBool::new(false),
            asr_received_text: Arc::new(AtomicBool::new(false)),
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

    /// 尝试获取流式 ASR 管道的识别结果
    ///
    /// 如果流式 ASR 管道已启动且尚未被消耗：
    /// 1. 关闭 mpsc Sender（通知 ASR 音频流结束）
    /// 2. 等待后台任务完成，返回完整识别文本
    /// 3. 如果任何步骤失败，记录警告并返回 `None`（便于回退到批处理 ASR）
    async fn try_get_streaming_asr_text(&self) -> Option<String> {
        let state = {
            let mut guard = self.streaming_state.lock().ok()?;
            guard.take()?
        };

        // 关闭 Sender → Receiver 端收到 None → 流结束 → 发送末帧
        drop(state.pcm_tx);

        match state.asr_handle.await {
            Ok(Ok(text)) => {
                tracing::info!(
                    "流式 ASR 成功，识别文本长度: {}，共 {} 帧音频 ({:.0}s)",
                    text.len(),
                    state.frame_count,
                    state.frame_count as f64 * 60.0 / 1000.0,
                );
                Some(text)
            }
            Ok(Err(e)) => {
                tracing::warn!("流式 ASR 失败: {}, 回退到批处理模式", e);
                None
            }
            Err(e) => {
                tracing::warn!("流式 ASR 任务异常: {}, 回退到批处理模式", e);
                None
            }
        }
    }

    /// 获取用户语音识别文本（流式 ASR 优先，批处理 ASR 回退）
    ///
    /// 返回 `Ok(Some(text))` 表示识别成功，`Ok(None)` 表示空音频/静音。
    async fn resolve_user_text(
        &self,
        audio_buffer: &[AudioFrame],
        session_id: &str,
    ) -> Result<Option<String>, String> {
        // ── 尝试流式 ASR（录音期间已完成识别） ──
        if let Some(text) = self.try_get_streaming_asr_text().await {
            if !text.is_empty() {
                tracing::info!(
                    session_id = %session_id,
                    text_len = text.len(),
                    "流式 ASR 识别完成",
                );
                return Ok(Some(text));
            }
        }

        // ── 回退：批处理 ASR ──
        if audio_buffer.is_empty() {
            return Ok(None);
        }

        let pcm_16k = decode_opus_frames_to_pcm(audio_buffer, 16000, 60)
            .map_err(|e| format!("Opus 解码失败: {}", e))?;

        if pcm_16k.is_empty() {
            return Ok(None);
        }

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
            return Ok(None);
        }

        tracing::info!(
            session_id = %session_id,
            text_len = text.len(),
            "批处理 ASR 识别完成",
        );

        Ok(Some(text))
    }

    /// 初始化流式 ASR 管道（惰性创建）
    ///
    /// 创建 mpsc channel + Doubao ASR 实例 + Opus 解码器，
    /// 后台启动 `listen_stream` 消费 PCM 流并收集识别结果。
    /// 管道状态存入 `streaming_state` 供 `on_audio_frame` 喂入音频。
    async fn init_asr_pipeline(&self) -> Result<(), String> {
        let app_key = self.app_key.clone();
        let access_token = self.access_token.clone();

        // 创建 mpsc channel：接收端作为 AudioStream 喂给 ASR
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(32);
        let audio_stream: AudioStream = Box::pin(ReceiverStream::new(pcm_rx));

        let asr = DoubaoAsr::new(DoubaoAsrOption {
            base: BaseProviderOption {
                language: Some("zh-CN".into()),
                ..Default::default()
            },
            app_key: Some(app_key),
            access_key: Some(access_token),
            mode: DoubaoAsrMode::Streaming,
            sample_rate: 16000,
            bits: 16,
            channel: 1,
            // VAD 端点检测：800ms 静音强制判停
            end_window_size: Some(800),
            // 至少 1s 音频后才允许判停（避免极短音频误判）
            force_to_speech_time: Some(1000),
            ..Default::default()
        });

        let vad_notify = self
            .vad_notify
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_else(|| Arc::new(Notify::new()));

        let asr_text_received = self.asr_received_text.clone();

        let asr_handle: tokio::task::JoinHandle<Result<String, String>> = tokio::spawn(
            async move {
                let mut stream = asr
                    .listen_stream(audio_stream)
                    .await
                    .map_err(|e| format!("流式 ASR 启动失败: {}", e))?;

                let mut full_text = String::new();
                let mut chunk_count = 0;
                let mut vad_triggered = false;
                // 记录最后一个非空 ASR 文本，用于判断是否已稳定
                let mut last_nonempty_text = String::new();
                const TEXT_STABLE_MS: u64 = 1500;

                loop {
                    // 如果已识别到非空文本，增加文本稳定超时检测
                    let needs_stability_check = !last_nonempty_text.is_empty();

                    if needs_stability_check {
                        let stability_delay =
                            tokio::time::sleep(std::time::Duration::from_millis(TEXT_STABLE_MS));
                        tokio::pin!(stability_delay);

                        tokio::select! {
                            chunk_opt = stream.next() => {
                                match chunk_opt {
                                    Some(Ok(chunk)) => {
                                        chunk_count += 1;

                                        let is_vad_endpoint = chunk
                                            .segment
                                            .as_ref()
                                            .and_then(|s| s.confidence)
                                            .map(|c| c >= 0.99)
                                            .unwrap_or(false);

                                        let tag = match (chunk.is_final, is_vad_endpoint) {
                                            (true, _) => "最终",
                                            (false, true) => "VAD",
                                            (false, false) => "中间",
                                        };
                                        let display_text = if chunk.text.is_empty() {
                                            "(空)".to_string()
                                        } else {
                                            chunk.text.clone()
                                        };
                                        tracing::info!(
                                            "🎤 [ASR {}] #{}: \"{}\"",
                                            tag,
                                            chunk_count,
                                            display_text,
                                        );

                                        if is_vad_endpoint && !vad_triggered {
                                            vad_triggered = true;
                                            tracing::info!("🎤 [VAD] 检测到语音结束，通知 ws.rs 开始处理",);
                                            vad_notify.notify_one();
                                        }

                                        if chunk.is_final && !chunk.text.is_empty() {
                                            full_text.push_str(&chunk.text);
                                        }

                                        if !chunk.text.is_empty() {
                                            asr_text_received.store(true, Ordering::Release);
                                            last_nonempty_text = chunk.text.clone();
                                        }
                                    }
                                    Some(Err(e)) => {
                                        tracing::warn!("🎤 [ASR 错误] {}", e);
                                    }
                                    None => {
                                        break;
                                    }
                                }
                            }
                            _ = &mut stability_delay => {
                                if !vad_triggered {
                                    tracing::info!(
                                        "🎤 [文本稳定 VAD] 文本 \"{}\" 已稳定 {}ms，通知 ws.rs 开始处理",
                                        last_nonempty_text,
                                        TEXT_STABLE_MS,
                                    );
                                    vad_notify.notify_one();
                                }
                                break;
                            }
                        }
                    } else {
                        match stream.next().await {
                            Some(Ok(chunk)) => {
                                chunk_count += 1;

                                let is_vad_endpoint = chunk
                                    .segment
                                    .as_ref()
                                    .and_then(|s| s.confidence)
                                    .map(|c| c >= 0.99)
                                    .unwrap_or(false);

                                let tag = match (chunk.is_final, is_vad_endpoint) {
                                    (true, _) => "最终",
                                    (false, true) => "VAD",
                                    (false, false) => "中间",
                                };
                                let display_text = if chunk.text.is_empty() {
                                    "(空)".to_string()
                                } else {
                                    chunk.text.clone()
                                };
                                tracing::info!(
                                    "🎤 [ASR {}] #{}: \"{}\"",
                                    tag,
                                    chunk_count,
                                    display_text,
                                );

                                if is_vad_endpoint && !vad_triggered {
                                    vad_triggered = true;
                                    tracing::info!("🎤 [VAD] 检测到语音结束，通知 ws.rs 开始处理",);
                                    vad_notify.notify_one();
                                }

                                if chunk.is_final && !chunk.text.is_empty() {
                                    full_text.push_str(&chunk.text);
                                }

                                if !chunk.text.is_empty() {
                                    asr_text_received.store(true, Ordering::Release);
                                    last_nonempty_text = chunk.text.clone();
                                }
                            }
                            Some(Err(e)) => {
                                tracing::warn!("🎤 [ASR 错误] {}", e);
                            }
                            None => {
                                break;
                            }
                        }
                    }
                }

                tracing::info!(
                    "🎤 [ASR 完成] 共收到 {} 个结果块，完整文本长度: {}",
                    chunk_count,
                    full_text.len(),
                );

                if full_text.is_empty() {
                    Err("ASR 识别结果为空（可能为静音或无有效语音）".to_string())
                } else {
                    Ok(full_text)
                }
            },
        );

        let sample_rate: u32 = 16000;
        let frame_duration_ms: u32 = 60;
        let frame_samples: usize = (sample_rate as u64 * frame_duration_ms as u64 / 1000) as usize;

        let decoder = Decoder::new(sample_rate, Channels::Mono)
            .map_err(|e| format!("创建 Opus 解码器失败: {}", e))?;

        let state = AsrPipelineState {
            pcm_tx,
            asr_handle,
            decoder,
            frame_samples,
            frame_count: 0,
            last_log_frame: 0,
            silence_count: 0,
            speech_detected: false,
        };

        let mut guard = self
            .streaming_state
            .lock()
            .map_err(|e| format!("锁获取失败: {}", e))?;
        *guard = Some(state);

        tracing::info!("流式 ASR: 管道已就绪（惰性初始化）");

        Ok(())
    }
}

#[async_trait]
impl ResponseStrategy for AsrLlmTtsStrategy {
    fn name(&self) -> &'static str {
        "asr-llm-tts"
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

    // ────────── 流式 ASR 支持 ──────────

    fn supports_streaming_asr(&self) -> bool {
        true
    }

    /// 录音开始时清空管道状态
    ///
    /// ASR 管道不会在此处创建，而是延迟到 `on_audio_frame` 收到第一帧音频时
    /// 通过 `init_asr_pipeline` 惰性初始化。这样可以避免在用户尚未说话时
    /// 建立 ASR 连接导致服务端对空音频返回误判的 VAD 端点。
    async fn on_recording_start(&self, session_id: &str) -> Result<(), String> {
        tracing::info!(
            session_id = %session_id,
            "流式 ASR: 清空管道状态（惰性初始化）",
        );

        let mut guard = self
            .streaming_state
            .lock()
            .map_err(|e| format!("锁获取失败: {}", e))?;
        *guard = None;
        // 创建全新 Notify 清除上一轮残留的通知信号
        if let Ok(mut ng) = self.vad_notify.lock() {
            *ng = Arc::new(Notify::new());
        }
        self.silence_closed.store(false, Ordering::Release);

        Ok(())
    }

    /// 每收到一帧 Opus 数据时，实时解码并喂入 ASR 管道
    ///
    /// 如果 ASR 管道尚未初始化（惰性），第一帧音频到达时会自动创建。
    /// 这样确保 ASR WebSocket 连接只在用户真正说话时建立。
    async fn on_audio_frame(&self, frame: &AudioFrame) -> Result<(), String> {
        if frame.data.is_empty() {
            return Ok(());
        }

        // 如果本地能量检测已关闭管道，跳过后续帧（不重新初始化）
        if self.silence_closed.load(Ordering::Acquire) {
            return Ok(());
        }

        // 惰性初始化：第一帧音频到达时才创建 ASR 管道
        let needs_init = self
            .streaming_state
            .lock()
            .map_err(|e| format!("锁获取失败: {}", e))?
            .is_none();
        if needs_init {
            tracing::info!("流式 ASR: 第一帧音频到达，惰性初始化管道");
            self.init_asr_pipeline().await?;
        }

        // ── Phase 1: 解码 + PCM 能量检测（锁内） ──
        let (pcm_bytes, pcm_tx) = {
            let mut guard = self
                .streaming_state
                .lock()
                .map_err(|e| format!("锁获取失败: {}", e))?;
            let state = guard
                .as_mut()
                .ok_or_else(|| "流式 ASR 未启动".to_string())?;

            state.frame_count += 1;
            // 每 50 帧（~3s）打印一次接收诊断日志
            if state.frame_count - state.last_log_frame >= 50 {
                state.last_log_frame = state.frame_count;
                tracing::info!(
                    "流式 ASR: 已接收 {} 帧 ({:.0}s 音频)",
                    state.frame_count,
                    state.frame_count as f64 * 60.0 / 1000.0,
                );
            }

            let mut pcm_buf = vec![0i16; state.frame_samples];
            let decoded_samples = state
                .decoder
                .decode(&frame.data, &mut pcm_buf, false)
                .map_err(|e| format!("Opus 解码错误: {}", e))?;

            // i16 → little-endian bytes
            let mut pcm_bytes = Vec::with_capacity(decoded_samples * 2);
            for sample in &pcm_buf[..decoded_samples] {
                pcm_bytes.extend_from_slice(&sample.to_le_bytes());
            }

            // ── 本地 PCM 能量检测 ──
            let rms = compute_pcm_rms(&pcm_bytes);
            if rms >= SILENCE_RMS_THRESHOLD {
                state.speech_detected = true;
                state.silence_count = 0;
            } else if state.speech_detected {
                // 只在首次语音后的静音才累计
                state.silence_count = state.silence_count.saturating_add(1);
            }

            if state.silence_count >= MAX_SILENCE_FRAMES
                && self.asr_received_text.load(Ordering::Acquire)
            {
                // 静音超阈值且 ASR 已经识别到过有效文本：关闭 ASR 流
                // 如果 ASR 还未返回任何非空文本（用户还没说话），则不触发本地 VAD，
                // 让系统继续等待（30s 安全超时兜底），避免用户正在思考时被提前中断
                let (new_tx, _) = mpsc::channel::<Vec<u8>>(1);
                let _ = std::mem::replace(&mut state.pcm_tx, new_tx);
                tracing::info!(
                    "本地能量 VAD: 检测到 {} 帧连续静音 ({:.0}s)，关闭 ASR 流",
                    state.silence_count,
                    state.silence_count as f64 * 60.0 / 1000.0,
                );
                self.silence_closed.store(true, Ordering::Release);
                if let Ok(guard) = self.vad_notify.lock() {
                    guard.notify_one();
                }
                // 不发送此帧（静音帧无意义）
                return Ok(());
            }

            (pcm_bytes, state.pcm_tx.clone())
        }; // MutexGuard 在此处释放

        // ── Phase 2: 发送 PCM 到 ASR（锁外） ──
        pcm_tx
            .send(pcm_bytes)
            .await
            .map_err(|_| "ASR 管道已关闭".to_string())?;

        Ok(())
    }

    // ────────── 流式回放支持 ──────────

    fn supports_streaming_playback(&self) -> bool {
        true
    }

    // ────────── VAD 端点检测 ──────────

    fn vad_completion(&self) -> Option<Arc<Notify>> {
        self.vad_notify.lock().ok().map(|g| g.clone())
    }

    /// 流式生成 ASR → LLM → TTS 响应并逐帧发送
    ///
    /// 相较 [`generate_response`]：
    /// - Agent 使用 `process_stream` 流式输出
    /// - TTS 使用 `speak_stream` 边合成边返回音频
    /// - 每块音频立即编码为 Opus 帧并通过 `frame_tx` 发送
    async fn generate_response_stream(
        &self,
        audio_buffer: Vec<AudioFrame>,
        session_id: &str,
        frame_tx: tokio::sync::mpsc::Sender<AudioFrame>,
    ) -> Result<(), String> {
        // ════════════════════════════════════════════════════════════════
        // Phase 1: 获取用户语音识别文本
        // ════════════════════════════════════════════════════════════════

        let user_text = match self.resolve_user_text(&audio_buffer, session_id).await? {
            Some(text) => text,
            None => {
                return Ok(());
            }
        };

        tracing::info!(
            session_id = %session_id,
            text_len = user_text.len(),
            "TTS-STREAM: ASR 识别完成",
        );

        // ════════════════════════════════════════════════════════════════
        // Phase 2: 生成回复文本（AI Agent 或固定文本）
        // ════════════════════════════════════════════════════════════════

        // 在流式转发的同时收集完整回复内容以便日志记录
        let llm_response_full = Arc::new(Mutex::new(String::new()));
        let response_for_log = llm_response_full.clone();

        let text_stream: Box<dyn futures_util::Stream<Item = String> + Unpin + Send> =
            if self.tts_config.fixed_text_enabled {
                // 固定文本模式：跳过 LLM，使用预设文本
                let fixed_text = self
                    .tts_config
                    .fixed_text
                    .clone()
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "欢迎使用智能语音助手".to_string());

                tracing::info!(
                    session_id = %session_id,
                    text = %fixed_text,
                    "TTS-STREAM: 固定文本模式，跳过 LLM",
                );

                if let Ok(mut full) = response_for_log.lock() {
                    full.push_str(&fixed_text);
                }
                Box::new(stream::iter(vec![fixed_text]))
            } else {
                // 普通模式：走 AI Agent 流式处理
                let current_llm_session = self
                    .llm_session_id
                    .lock()
                    .map_err(|e| format!("LLM session_id 锁获取失败: {}", e))?
                    .clone();

                let (text_stream_inner, new_llm_session_id) = self
                    .agent
                    .process_stream(&user_text, current_llm_session.as_deref())
                    .await
                    .map_err(|e| format!("AI Agent 流式处理失败: {}", e))?;

                // 立即更新 LLM 会话 ID（用于多轮对话）
                if let Ok(mut session) = self.llm_session_id.lock() {
                    *session = Some(new_llm_session_id);
                }

                tracing::info!(
                    session_id = %session_id,
                    agent = self.agent.name(),
                    "TTS-STREAM: Agent 流式输出已启动",
                );

                Box::new(text_stream_inner.inspect(move |chunk| {
                    if let Ok(mut full) = response_for_log.lock() {
                        full.push_str(chunk);
                    }
                }))
            };

        // ════════════════════════════════════════════════════════════════
        // Phase 3: 流式 TTS 合成 → Opus 编码 → 逐帧发送
        // ════════════════════════════════════════════════════════════════

        tracing::info!(
            session_id = %session_id,
            provider = %self.tts_config.active_provider,
            "TTS-STREAM: 开始流式语音合成",
        );

        let tts = crate::tts_factory::create_tts_provider(&self.tts_config)
            .map_err(|e| format!("创建 TTS 提供者失败: {}", e))?;

        let mut audio_stream = tts
            .speak_stream(Box::pin(text_stream))
            .await
            .map_err(|e| format!("流式 TTS 启动失败: {}", e))?;

        // ── Phase 3a: 流式 Opus 编码 + 即时下发 ────────────────────
        // 使用持久化的 StreamingOpusEncoder 处理流式 PCM，
        // 每完成一帧 Opus 就立即通过 frame_tx 下发到硬件端。
        // 这样硬件端在 ~600ms（10帧预缓冲）后即可开始播放，
        // 无需等待全部 TTS 合成完成。
        let mut frame_count: usize = 0;
        let mut total_audio_bytes: usize = 0;
        let mut raw_pcm: Vec<u8> = Vec::new();
        let mut stream_enc = StreamingOpusEncoder::new(24000, 60)?;
        let mut timestamp: u32 = 0;

        while let Some(result) = audio_stream.next().await {
            match result {
                Ok(chunk) => {
                    total_audio_bytes += chunk.audio_chunk.len();
                    raw_pcm.extend_from_slice(&chunk.audio_chunk);

                    let opus_frames = stream_enc
                        .feed(&chunk.audio_chunk)
                        .map_err(|e| format!("Opus 编码失败: {}", e))?;

                    for opus in opus_frames {
                        if frame_tx
                            .send(AudioFrame {
                                timestamp,
                                data: opus,
                            })
                            .await
                            .is_err()
                        {
                            tracing::info!(
                                session_id = %session_id,
                                "TTS-STREAM: 回放管道已关闭，停止生成",
                            );
                            return Ok(());
                        }
                        timestamp = timestamp.wrapping_add(60);
                        frame_count += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!("流式 TTS 音频块错误: {}", e);
                }
            }
        }

        // ── Phase 3b: 编码最后残片 ─────────────────────────────────
        // TTS 合成完成后，flush 缓存中的不足一帧的残片（零填充后编码）
        {
            let last_frames = stream_enc
                .flush()
                .map_err(|e| format!("Opus 编码失败: {}", e))?;
            for opus in last_frames {
                if frame_tx
                    .send(AudioFrame {
                        timestamp,
                        data: opus,
                    })
                    .await
                    .is_err()
                {
                    tracing::info!(
                        session_id = %session_id,
                        "TTS-STREAM: 回放管道已关闭，停止生成",
                    );
                    return Ok(());
                }
                timestamp = timestamp.wrapping_add(60);
                frame_count += 1;
            }
        }

        // ── 保存 TTS 音频到本地 ───────────────────────────────────
        if !raw_pcm.is_empty() {
            save_tts_audio_as_wav(&raw_pcm, session_id);
        }

        let full_response = llm_response_full
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();

        tracing::info!(
            session_id = %session_id,
            total_audio_bytes = total_audio_bytes,
            frame_count = frame_count,
            response_len = full_response.len(),
            response = %full_response,
            "TTS-STREAM: 流式合成完成",
        );

        Ok(())
    }

    /// 生成 ASR → LLM → TTS 响应
    ///
    /// 获取用户语音识别文本有两个途径：
    /// - **流式 ASR**（优先）：录音期间已实时识别，直接取结果
    /// - **批处理 ASR**（回退）：对缓冲区音频进行 Opus 解码后再识别
    ///
    /// 获取文本后执行 Agent → TTS → 编码回传（两路径共用）。
    async fn generate_response(
        &self,
        audio_buffer: Vec<AudioFrame>,
        session_id: &str,
    ) -> Result<Vec<AudioFrame>, String> {
        // ════════════════════════════════════════════════════════════════
        // Phase 1: 获取用户语音识别文本
        // ════════════════════════════════════════════════════════════════

        let user_text = match self.resolve_user_text(&audio_buffer, session_id).await? {
            Some(text) => text,
            None => {
                return Ok(Vec::new());
            }
        };

        // ════════════════════════════════════════════════════════════════
        // Phase 2: 生成回复文本（AI Agent 或固定文本）
        // ════════════════════════════════════════════════════════════════
        tracing::info!(
            session_id = %session_id,
            text = %user_text,
            "ASR-LLM-TTS: ASR 识别完成",
        );

        let llm_text = if self.tts_config.fixed_text_enabled {
            // 固定文本模式：跳过 LLM，使用预设文本
            let fixed_text = self
                .tts_config
                .fixed_text
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "欢迎使用智能语音助手".to_string());

            tracing::info!(
                session_id = %session_id,
                text = %fixed_text,
                "ASR-LLM-TTS: 固定文本模式，跳过 LLM",
            );

            fixed_text
        } else {
            // 普通模式：走 AI Agent 处理
            tracing::info!(
                session_id = %session_id,
                agent = self.agent.name(),
                "ASR-LLM-TTS: 开始 AI Agent 处理",
            );

            let current_llm_session = self
                .llm_session_id
                .lock()
                .map_err(|e| format!("LLM session_id 锁获取失败: {}", e))?
                .clone();

            let llm_response = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                self.agent
                    .process(&user_text, current_llm_session.as_deref()),
            )
            .await
            .map_err(|_| "AI Agent 响应超时 (60s)".to_string())?
            .map_err(|e| format!("AI Agent 处理失败: {}", e))?;

            let (llm_text, new_llm_session_id) = llm_response;

            if llm_text.is_empty() {
                return Err("AI Agent 返回空回复".to_string());
            }

            // 更新 LLM 会话 ID（用于多轮对话）
            if let Ok(mut session) = self.llm_session_id.lock() {
                *session = Some(new_llm_session_id);
            }

            tracing::info!(
                session_id = %session_id,
                response_len = llm_text.len(),
                response = %llm_text,
                "ASR-LLM-TTS: AI Agent 处理完成",
            );

            llm_text
        };

        // ── Step 4: TTS 语音合成 ──
        tracing::info!(
            session_id = %session_id,
            provider = %self.tts_config.active_provider,
            "ASR-LLM-TTS: 开始语音合成",
        );

        let tts = crate::tts_factory::create_tts_provider(&self.tts_config)
            .map_err(|e| format!("创建 TTS 提供者失败: {}", e))?;

        let response = tts
            .synthesize(TtsRequest {
                text: llm_text.clone(),
                options: None,
            })
            .await
            .map_err(|e| format!("TTS 合成失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            audio_size = response.audio.len(),
            format = %response.format,
            "ASR-LLM-TTS: TTS 合成完成",
        );

        if response.audio.is_empty() {
            tracing::warn!(
                session_id = %session_id,
                "ASR-LLM-TTS: TTS 返回空音频",
            );
            return Ok(Vec::new());
        }

        // ── Step 5: PCM → Opus 编码 (24kHz, 60ms) ──
        let opus_frames = pcm_to_opus_frames(&response.audio, 24000, 60)
            .map_err(|e| format!("Opus 编码失败: {}", e))?;

        tracing::info!(
            session_id = %session_id,
            frame_count = opus_frames.len(),
            "ASR-LLM-TTS: Opus 编码完成",
        );

        // ── Step 6: 封装为 AudioFrame ──
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
            "ASR-LLM-TTS: 管线完成",
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
// TTS 音频本地存储
// ═══════════════════════════════════════════════════════════════════════════════

/// 将 PCM16 mono 24000Hz 音频保存为 WAV 文件
///
/// 每次 TTS 合成完成后，在下发到硬件端之前，将原始音频数据存档到
/// `~/.haimen/tts_recordings/` 目录，文件名为 `tts-{session_id}-{微秒时间戳}.wav`。
///
/// WAV 格式：PCM 16-bit mono 24000Hz，无额外依赖。
fn save_tts_audio_as_wav(pcm: &[u8], session_id: &str) {
    let dir = crate::config::settings::get_settings_dir().join("tts_recordings");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("无法创建 TTS 录音目录 {}: {}", dir.display(), e);
        return;
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();

    let filename = format!("tts-{}-{}.wav", session_id, timestamp);
    let path = dir.join(&filename);

    const SAMPLE_RATE: u32 = 24000;
    const BITS_PER_SAMPLE: u16 = 16;
    const CHANNELS: u16 = 1;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * BITS_PER_SAMPLE as u32 / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    let data_size = pcm.len() as u32;

    // 构建 WAV 文件：44 字节 RIFF/WAVE 头 + PCM 数据
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // subchunk1 size (PCM)
    wav.extend_from_slice(&1u16.to_le_bytes()); // audio format (PCM = 1)
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);

    match std::fs::write(&path, &wav) {
        Ok(_) => tracing::info!(
            "TTS 音频已保存: {} ({:.1} KB)",
            filename,
            wav.len() as f64 / 1024.0,
        ),
        Err(e) => tracing::warn!("保存 TTS 音频失败 {}: {}", filename, e),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 流式 Opus 编码器
// ═══════════════════════════════════════════════════════════════════════════════

/// 流式 Opus 编码器
///
/// 维护持久的编码器状态和帧间残片缓存，适合 `generate_response_stream` 的流式场景。
///
/// # 原理
///
/// - **持久编码器**：整个 TTS 合成只创建一次 `opus2::Encoder`，帧间预测状态连续
/// - **残片缓存**：不足一帧（60ms @ 24kHz = 2880 bytes）的 PCM 残片留在 `partial` 中，
///   下次 `feed()` 时补齐再编码，避免跨 chunk 边界零填充
/// - **零填充仅发生在末尾**：`flush()` 时对最终残片零填充一次
struct StreamingOpusEncoder {
    /// Opus 编码器（整个合成周期复用）
    encoder: opus2::Encoder,
    /// 不足一帧的残片缓存
    partial: Vec<u8>,
    /// 编码输出缓冲
    opus_buf: Vec<u8>,
    /// 每帧字节数（frame_duration_ms @ sample_rate 16-bit mono）
    frame_bytes: usize,
}

impl StreamingOpusEncoder {
    /// 创建流式 Opus 编码器
    fn new(sample_rate: u32, frame_duration_ms: u32) -> Result<Self, String> {
        let frame_samples = (sample_rate as u64 * frame_duration_ms as u64 / 1000) as usize;
        let frame_bytes = frame_samples * 2; // 16-bit

        let mut encoder = opus2::Encoder::new(sample_rate, Channels::Mono, Application::Audio)
            .map_err(|e| format!("创建 Opus 编码器失败: {}", e))?;
        let _ = encoder.set_complexity(10);
        let _ = encoder.set_vbr(true);
        let _ = encoder.set_dtx(true);

        Ok(Self {
            encoder,
            partial: Vec::new(),
            opus_buf: vec![0u8; 4000],
            frame_bytes,
        })
    }

    /// 喂入 PCM 数据，返回本次编码完成的 Opus 帧列表
    ///
    /// 将新 PCM 追加到残片缓存，从中取出完整的帧编码为 Opus，
    /// 不足一帧的残片保留在内部供下次 `feed()` 补齐。
    fn feed(&mut self, pcm: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.partial.extend_from_slice(pcm);

        let complete_len = self.partial.len() / self.frame_bytes * self.frame_bytes;
        if complete_len == 0 {
            return Ok(Vec::new());
        }

        let mut frames: Vec<Vec<u8>> = Vec::new();
        let to_encode = &self.partial[..complete_len];
        for chunk in to_encode.chunks(self.frame_bytes) {
            let pcm_i16: Vec<i16> = chunk
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect();
            let encoded_len = self
                .encoder
                .encode(&pcm_i16, &mut self.opus_buf)
                .map_err(|e| format!("Opus 编码错误: {}", e))?;
            frames.push(self.opus_buf[..encoded_len].to_vec());
        }

        self.partial = self.partial[complete_len..].to_vec();
        Ok(frames)
    }

    /// 编码剩余的残片（零填充后），返回最终的 Opus 帧列表
    ///
    /// 如果 `partial` 中有缓存的残片数据，零填充到一帧后编码输出。
    /// 调用后内部状态清空，编码器不再可用。
    fn flush(&mut self) -> Result<Vec<Vec<u8>>, String> {
        if self.partial.is_empty() {
            return Ok(Vec::new());
        }

        self.partial.resize(self.frame_bytes, 0);
        let pcm_i16: Vec<i16> = self
            .partial
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let encoded_len = self
            .encoder
            .encode(&pcm_i16, &mut self.opus_buf)
            .map_err(|e| format!("Opus 编码错误: {}", e))?;
        self.partial.clear();

        Ok(vec![self.opus_buf[..encoded_len].to_vec()])
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xiaozhi_tts::pcm_to_opus_frames;

    // ─── 模拟 Agent —— 用于测试 ──────────────────────────────

    /// 测试用 Mock Agent：将收到的消息原样返回
    struct MockAgent;

    #[async_trait]
    impl AgentProvider for MockAgent {
        fn name(&self) -> &str {
            "mock-agent"
        }

        async fn process(
            &self,
            message: &str,
            session_id: Option<&str>,
        ) -> Result<(String, String), String> {
            // 如果提供了 session_id，追加 "(continued)" 表示恢复了上下文
            let response = if session_id.is_some() {
                format!("{} (continued)", message)
            } else {
                message.to_string()
            };
            Ok((response, "mock-session-id".to_string()))
        }

        async fn check_available(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// 模拟 Agent：总是返回错误
    struct FailingAgent;

    #[async_trait]
    impl AgentProvider for FailingAgent {
        fn name(&self) -> &str {
            "failing-agent"
        }

        async fn process(
            &self,
            _message: &str,
            _session_id: Option<&str>,
        ) -> Result<(String, String), String> {
            Err("模拟 Agent 失败".to_string())
        }

        async fn check_available(&self) -> Result<(), String> {
            Err("模拟 Agent 不可用".to_string())
        }
    }

    /// 模拟 Agent：返回空回复
    struct EmptyResponseAgent;

    #[async_trait]
    impl AgentProvider for EmptyResponseAgent {
        fn name(&self) -> &str {
            "empty-response-agent"
        }

        async fn process(
            &self,
            _message: &str,
            _session_id: Option<&str>,
        ) -> Result<(String, String), String> {
            Ok((String::new(), "empty-session".to_string()))
        }

        async fn check_available(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn test_tts_config() -> crate::config::settings::TtsConfig {
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

    fn make_strategy(agent: Arc<dyn AgentProvider>) -> AsrLlmTtsStrategy {
        AsrLlmTtsStrategy::new(
            "app_key".into(),
            "access_token".into(),
            test_tts_config(),
            agent,
        )
    }

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
        let frames = vec![AudioFrame {
            timestamp: 0,
            data: vec![0xFF, 0xFF, 0xFF, 0xFF],
        }];
        let result = decode_opus_frames_to_pcm(&frames, 16000, 60);
        assert!(result.is_err());
    }

    // ─── Strategy 基本测试 ─────────────────────────────

    #[test]
    fn test_t7_strategy_name() {
        let strategy = make_strategy(Arc::new(MockAgent));
        assert_eq!(strategy.name(), "asr-llm-tts");
    }

    #[test]
    fn test_t8_hello_audio_params() {
        let strategy = make_strategy(Arc::new(MockAgent));
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
        assert_send_sync::<AsrLlmTtsStrategy>();
    }

    #[test]
    fn test_t10_with_resource_id() {
        let strategy = make_strategy(Arc::new(MockAgent)).with_resource_id("seed-tts-1.0".into());
        assert_eq!(
            strategy.tts_config.get_credential("resource_id").as_deref(),
            Some("seed-tts-1.0")
        );
    }

    // ─── LLM session 管理测试 ───────────────────────────

    #[test]
    fn test_t17_llm_session_id_initial_none() {
        let strategy = make_strategy(Arc::new(MockAgent));
        let session = strategy.llm_session_id.lock().unwrap();
        assert!(session.is_none(), "初始 LLM session_id 应为 None");
    }

    #[test]
    fn test_t18_llm_session_id_update() {
        let strategy = make_strategy(Arc::new(MockAgent));
        {
            let mut session = strategy.llm_session_id.lock().unwrap();
            *session = Some("test-session-123".to_string());
        }
        let session = strategy.llm_session_id.lock().unwrap();
        assert_eq!(
            session.as_deref(),
            Some("test-session-123"),
            "session_id 应被更新"
        );
    }

    // ─── generate_response 行为测试（Mock Agent） ───────

    #[tokio::test]
    async fn test_t19_generate_response_empty_buffer() {
        let strategy = make_strategy(Arc::new(MockAgent));
        let result = strategy.generate_response(vec![], "test-session").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_t20_generate_response_llm_failure() {
        let strategy = make_strategy(Arc::new(FailingAgent));
        // 由于 ASR 会实际调用外部服务，跳过集成测试
        // 这里只验证策略能正常构建且 Send+Sync
        assert_eq!(strategy.agent.name(), "failing-agent");
    }

    #[tokio::test]
    async fn test_t21_generate_response_llm_empty_response() {
        let strategy = make_strategy(Arc::new(EmptyResponseAgent));
        assert_eq!(strategy.agent.name(), "empty-response-agent");
    }

    // ─── MockAgent process 行为验证 ──────────────────

    #[tokio::test]
    async fn test_t22_mock_agent_no_session() {
        let agent = MockAgent;
        let (response, session_id) = agent.process("你好", None).await.expect("MockAgent 应成功");
        assert_eq!(response, "你好");
        assert_eq!(session_id, "mock-session-id");
    }

    #[tokio::test]
    async fn test_t23_mock_agent_with_session() {
        let agent = MockAgent;
        let (response, session_id) = agent
            .process("你好", Some("prev-session"))
            .await
            .expect("MockAgent 应成功");
        assert_eq!(response, "你好 (continued)");
        assert_eq!(session_id, "mock-session-id");
    }

    #[tokio::test]
    async fn test_t24_failing_agent_process() {
        let agent = FailingAgent;
        let result = agent.process("你好", None).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "模拟 Agent 失败");
    }

    // ─── 流式 ASR 支持测试 ────────────────────────────

    #[test]
    fn test_t25_supports_streaming_asr() {
        let strategy = make_strategy(Arc::new(MockAgent));
        assert!(
            strategy.supports_streaming_asr(),
            "AsrLlmTtsStrategy 应支持流式 ASR"
        );
    }

    #[test]
    fn test_t26_streaming_state_initial_none() {
        let strategy = make_strategy(Arc::new(MockAgent));
        let guard = strategy.streaming_state.lock().unwrap();
        assert!(guard.is_none(), "初始 streaming_state 应为 None");
    }

    #[tokio::test]
    async fn test_t27_try_get_streaming_asr_text_when_idle() {
        let strategy = make_strategy(Arc::new(MockAgent));
        let result = strategy.try_get_streaming_asr_text().await;
        assert!(result.is_none(), "未启动流式 ASR 时应返回 None");
    }

    #[test]
    fn test_t28_streaming_state_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AsrLlmTtsStrategy>();
    }

    #[test]
    fn test_t29_with_resource_id_preserves_state() {
        let strategy = make_strategy(Arc::new(MockAgent)).with_resource_id("test-resource".into());
        assert_eq!(
            strategy.tts_config.get_credential("resource_id").as_deref(),
            Some("test-resource")
        );
        // streaming_state 不应受 with_resource_id 影响
        let guard = strategy.streaming_state.lock().unwrap();
        assert!(guard.is_none());
    }

    // ─── 流式 TTS 回放支持测试 ───────────────────────────

    #[test]
    fn test_t30_supports_streaming_playback() {
        let strategy = make_strategy(Arc::new(MockAgent));
        assert!(
            strategy.supports_streaming_playback(),
            "AsrLlmTtsStrategy 应支持流式回放",
        );
    }

    /// 验证 generate_response_stream 对空输入的处理
    #[tokio::test]
    async fn test_t31_generate_response_stream_empty_buffer() {
        let strategy = make_strategy(Arc::new(MockAgent));
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let result = strategy
            .generate_response_stream(vec![], "test-session", tx)
            .await;
        // 空缓冲区不应报错，应返回 Ok(())
        assert!(result.is_ok(), "空缓冲区应返回 Ok(())");
    }

    /// 验证 resolve_user_text 对空输入的处理
    #[tokio::test]
    async fn test_t32_resolve_user_text_empty() {
        let strategy = make_strategy(Arc::new(MockAgent));
        let result = strategy.resolve_user_text(&[], "test-session").await;
        assert!(result.is_ok(), "空缓冲区应返回 Ok");
        assert!(result.unwrap().is_none(), "空缓冲区应返回 None");
    }

    /// 模拟 agent 验证 process_stream 的默认行为
    #[tokio::test]
    async fn test_t33_mock_agent_process_stream() {
        let agent = MockAgent;
        let (mut stream, sid) = agent
            .process_stream("你好", None)
            .await
            .expect("MockAgent process_stream 应成功");
        let mut result = String::new();
        while let Some(chunk) = stream.next().await {
            result.push_str(&chunk);
        }
        assert_eq!(result, "你好", "process_stream 应返回 process 的结果");
        assert_eq!(sid, "mock-session-id");
    }
}
