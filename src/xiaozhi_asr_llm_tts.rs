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

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use haimen_xiaozhi::{AudioFrame, AudioParams, ResponseStrategy};
use opus2::{Channels, Decoder};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use univoice::asr::{
    AsrProvider, AudioInput, AudioStream, BaseProviderOption, DEFAULT_CHUNK_SIZE, DoubaoAsr,
    DoubaoAsrMode, DoubaoAsrOption, adapt_audio_input,
};
use univoice::tts::provider::{DoubaoTts, DoubaoTtsOption};
use univoice::tts::{BaseTtsOption, TtsProvider, TtsRequest};

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
}

/// ASR → LLM → TTS 响应策略：将设备录制的语音识别为文字，
/// 送 AI Agent 处理，再将回复合成为语音回传
///
/// 管线：Opus 解码 (16kHz) → Doubao ASR → AgentProvider → Doubao TTS (24kHz) → Opus 编码
pub struct AsrLlmTtsStrategy {
    /// 火山引擎 App Key
    app_key: String,
    /// 火山引擎 Access Token
    access_token: String,
    /// TTS 音色（None 使用环境变量或默认值）
    voice: Option<String>,
    /// 火山引擎 Resource ID（用于声音克隆等）
    resource_id: Option<String>,
    /// 火山引擎 Cluster（用于推导 resource_id）
    cluster: Option<String>,
    /// AI Agent（Claude Code、Codex 等）
    agent: Arc<dyn AgentProvider>,
    /// LLM 会话 ID，用于多轮对话上下文连续
    llm_session_id: Mutex<Option<String>>,
    /// 流式 ASR 管道状态（录音期间启用，录音结束时消耗）
    streaming_state: Mutex<Option<AsrPipelineState>>,
}

impl AsrLlmTtsStrategy {
    /// 创建 ASR → LLM → TTS 策略
    ///
    /// # 参数
    ///
    /// * `app_key` — 火山引擎 App Key
    /// * `access_token` — 火山引擎 Access Token
    /// * `voice` — TTS 音色（None 从环境变量或默认值读取）
    /// * `agent` — AI Agent 实例
    pub fn new(
        app_key: String,
        access_token: String,
        voice: Option<String>,
        agent: Arc<dyn AgentProvider>,
    ) -> Self {
        let voice = voice
            .or_else(|| std::env::var("DOUBAO_VOICE_TYPE").ok())
            .or_else(|| Some("zh_female_xiaohe_uranus_bigtts".into()));
        let cluster = std::env::var("DOUBAO_CLUSTER").ok();

        Self {
            app_key,
            access_token,
            voice,
            resource_id: None,
            cluster,
            agent,
            llm_session_id: Mutex::new(None),
            streaming_state: Mutex::new(None),
        }
    }

    /// 设置 Resource ID（声音克隆等场景）
    pub fn with_resource_id(mut self, resource_id: String) -> Self {
        self.resource_id = Some(resource_id);
        self
    }

    /// 将 cluster 映射为 resource_id
    ///
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
                tracing::info!("流式 ASR 成功，识别文本长度: {}", text.len());
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

    /// 录音开始时启动流式 ASR 管道
    ///
    /// 创建 Opus 解码器 + mpsc channel，后台启动 Doubao ASR `listen_stream`，
    /// 等待 `on_audio_frame` 推入解码后的 PCM 数据。
    async fn on_recording_start(&self, session_id: &str) -> Result<(), String> {
        tracing::info!(
            session_id = %session_id,
            "流式 ASR: 启动双向流式管道",
        );

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
            ..Default::default()
        });

        // 后台任务：消费 PCM 流，收集 ASR 识别结果
        let asr_handle: tokio::task::JoinHandle<Result<String, String>> =
            tokio::spawn(async move {
                let mut stream = asr
                    .listen_stream(audio_stream)
                    .await
                    .map_err(|e| format!("流式 ASR 启动失败: {}", e))?;

                let mut full_text = String::new();
                let mut chunk_count = 0;

                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => {
                            chunk_count += 1;
                            let tag = if chunk.is_final { "最终" } else { "中间" };
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

                            if chunk.is_final && !chunk.text.is_empty() {
                                full_text.push_str(&chunk.text);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("🎤 [ASR 错误] {}", e);
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
            });

        // 创建会话级 Opus 解码器
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
        };

        let mut guard = self
            .streaming_state
            .lock()
            .map_err(|e| format!("锁获取失败: {}", e))?;
        *guard = Some(state);

        tracing::info!(
            session_id = %session_id,
            "流式 ASR: 管道已就绪",
        );

        Ok(())
    }

    /// 每收到一帧 Opus 数据时，实时解码并喂入 ASR 管道
    async fn on_audio_frame(&self, frame: &AudioFrame) -> Result<(), String> {
        if frame.data.is_empty() {
            return Ok(());
        }

        // 在锁内完成解码，提取 pcm_bytes 和 pcm_tx 后释放锁
        let (pcm_bytes, pcm_tx) = {
            let mut guard = self
                .streaming_state
                .lock()
                .map_err(|e| format!("锁获取失败: {}", e))?;
            let state = guard
                .as_mut()
                .ok_or_else(|| "流式 ASR 未启动".to_string())?;

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

            (pcm_bytes, state.pcm_tx.clone())
        }; // MutexGuard 在此处释放

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
        // Phase 2: AI Agent 流式处理
        // ════════════════════════════════════════════════════════════════

        // 读取当前 LLM 会话 ID
        let current_llm_session = self
            .llm_session_id
            .lock()
            .map_err(|e| format!("LLM session_id 锁获取失败: {}", e))?
            .clone();

        let (text_stream, new_llm_session_id) = self
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

        // ════════════════════════════════════════════════════════════════
        // Phase 3: 流式 TTS 合成 → Opus 编码 → 逐帧发送
        // ════════════════════════════════════════════════════════════════

        let resource_id = self.resolve_resource_id();
        let voice = self.voice.clone().map(Into::into);

        tracing::info!(
            session_id = %session_id,
            voice = ?voice,
            resource_id = %resource_id,
            "TTS-STREAM: 开始流式语音合成",
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

        let mut audio_stream = tts
            .speak_stream(text_stream)
            .await
            .map_err(|e| format!("流式 TTS 启动失败: {}", e))?;

        let mut timestamp: u32 = 0;
        let mut total_audio_bytes: usize = 0;

        while let Some(result) = audio_stream.next().await {
            match result {
                Ok(chunk) => {
                    total_audio_bytes += chunk.audio_chunk.len();

                    // 将 PCM 音频块编码为 Opus 帧（24kHz, 60ms）
                    let opus_frames = pcm_to_opus_frames(&chunk.audio_chunk, 24000, 60)
                        .map_err(|e| format!("Opus 编码失败: {}", e))?;

                    for opus in opus_frames {
                        let frame = AudioFrame {
                            timestamp,
                            data: opus,
                        };
                        if frame_tx.send(frame).await.is_err() {
                            // 接收端已关闭（如播放被中断），安全退出
                            tracing::info!(
                                session_id = %session_id,
                                "TTS-STREAM: 回放管道已关闭，停止生成",
                            );
                            return Ok(());
                        }
                        timestamp = timestamp.wrapping_add(60);
                    }
                }
                Err(e) => {
                    tracing::warn!("流式 TTS 音频块错误: {}", e);
                }
            }
        }

        tracing::info!(
            session_id = %session_id,
            total_audio_bytes = total_audio_bytes,
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
        // Phase 2: AI Agent 处理（流式与批处理共用）
        // ════════════════════════════════════════════════════════════════
        tracing::info!(
            session_id = %session_id,
            text = %user_text,
            "ASR-LLM-TTS: ASR 识别完成",
        );

        // ── Step 3: AI Agent 处理 ──
        tracing::info!(
            session_id = %session_id,
            agent = self.agent.name(),
            "ASR-LLM-TTS: 开始 AI Agent 处理",
        );

        // 读取当前 LLM 会话 ID
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
            "ASR-LLM-TTS: AI Agent 处理完成",
        );

        // ── Step 4: Doubao TTS 语音合成 ──
        let resource_id = self.resolve_resource_id();
        let voice = self.voice.clone().map(Into::into);

        tracing::info!(
            session_id = %session_id,
            voice = ?voice,
            resource_id = %resource_id,
            "ASR-LLM-TTS: 开始语音合成",
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

    fn make_strategy(agent: Arc<dyn AgentProvider>) -> AsrLlmTtsStrategy {
        AsrLlmTtsStrategy::new("app_key".into(), "access_token".into(), None, agent)
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
        assert_eq!(strategy.resource_id, Some("seed-tts-1.0".into()));
    }

    #[test]
    fn test_t11_resolve_resource_id_default() {
        let strategy = make_strategy(Arc::new(MockAgent));
        assert_eq!(strategy.resolve_resource_id(), "seed-tts-2.0");
    }

    #[test]
    fn test_t12_resolve_resource_id_custom() {
        let strategy =
            make_strategy(Arc::new(MockAgent)).with_resource_id("custom-resource".into());
        assert_eq!(strategy.resolve_resource_id(), "custom-resource");
    }

    #[test]
    fn test_t13_voice_default_when_none() {
        let strategy = make_strategy(Arc::new(MockAgent));
        assert_eq!(
            strategy.voice.as_deref(),
            Some("zh_female_xiaohe_uranus_bigtts")
        );
    }

    #[test]
    fn test_t14_voice_uses_env_var() {
        unsafe {
            std::env::set_var("DOUBAO_VOICE_TYPE", "zh_female_vv_uranus_bigtts");
        }
        let strategy = make_strategy(Arc::new(MockAgent));
        assert_eq!(
            strategy.voice.as_deref(),
            Some("zh_female_vv_uranus_bigtts")
        );
        unsafe {
            std::env::remove_var("DOUBAO_VOICE_TYPE");
        }
    }

    #[test]
    fn test_t15_voice_cli_overrides_env() {
        unsafe {
            std::env::set_var("DOUBAO_VOICE_TYPE", "env_voice");
        }
        let strategy = AsrLlmTtsStrategy::new(
            "k".into(),
            "t".into(),
            Some("cli_voice".into()),
            Arc::new(MockAgent),
        );
        assert_eq!(strategy.voice.as_deref(), Some("cli_voice"));
        unsafe {
            std::env::remove_var("DOUBAO_VOICE_TYPE");
        }
    }

    #[test]
    fn test_t16_resolve_resource_id_volcano_icl() {
        unsafe {
            std::env::set_var("DOUBAO_CLUSTER", "volcano_icl");
        }
        let strategy = make_strategy(Arc::new(MockAgent));
        assert_eq!(strategy.resolve_resource_id(), "seed-tts-1.0");
        unsafe {
            std::env::remove_var("DOUBAO_CLUSTER");
        }
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
        assert_eq!(strategy.resource_id, Some("test-resource".into()));
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
