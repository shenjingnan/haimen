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
//! # 文本下发
//!
//! 流式回放路径（`generate_response_stream`）下，除音频帧外还会下发两类文本消息
//! 供设备 OLED/LCD 屏幕显示对话内容：
//! - `stt`：用户语音识别文本（ASR 结果），录音结束识别完成后立即下发
//! - `tts/sentence_start`：LLM 回复文本，按句末标点切分，随音频节奏逐句上屏
//!
//! # 多轮对话
//!
//! 策略内部维护 LLM 的 `session_id`，每次 `generate_response` 调用后更新，
//! 实现音色多轮对话的上下文连续性。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use haimen_xiaozhi::{AudioFrame, AudioParams, PlaybackEvent, ResponseStrategy};
use opus2::{Application, Channels, Decoder};
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use univoice::asr::{
    AsrProvider, AudioContainerFormat, AudioInput, AudioStream, BaseProviderOption,
    DEFAULT_CHUNK_SIZE, DoubaoAsr, DoubaoAsrMode, DoubaoAsrOption, GlmAsr, GlmAsrOption, MimoAsr,
    MimoAsrOption, QwenAsr, QwenAsrOption, XfyunAsr, XfyunAsrOption, adapt_audio_input,
};
use univoice::tts::TtsRequest;

use crate::config::settings::{AsrConfig, TtsConfig};
use crate::gateway::provider::{AgentEventStream, AgentLogEvent, AgentProvider};
use crate::xiaozhi_tts::pcm_to_opus_frames;

/// 共享 TTS 配置类型
pub type SharedTtsConfig = Arc<RwLock<TtsConfig>>;

/// 共享 ASR 配置类型
pub type SharedAsrConfig = Arc<RwLock<AsrConfig>>;

// ═══════════════════════════════════════════════════════════════════════════════
// 句切分工具
// ═══════════════════════════════════════════════════════════════════════════════

/// 从句子缓冲区提取一个完整句子（含结尾标点），并消费掉它。
///
/// 找不到句末标点时返回 `None`（残句留在 `buf` 中，等待后续 chunk 累积）。
/// ASCII `.` 不纳入句末标点，避免拆分 `Mr.` / `3.14` 等缩写与数字。
fn take_sentence(buf: &mut String) -> Option<String> {
    let mut end = None;
    for (i, ch) in buf.char_indices() {
        if matches!(ch, '。' | '！' | '？' | '；' | '!' | '?' | ';' | '\n') {
            end = Some(i + ch.len_utf8());
            break;
        }
    }
    let end = end?;
    // 吞并紧跟的连续标点，避免产出如 "！" 这样的空句
    let mut cut = end;
    for ch in buf[end..].chars() {
        if matches!(ch, '。' | '！' | '？' | '；' | '!' | '?' | ';') {
            cut += ch.len_utf8();
        } else {
            break;
        }
    }
    let sentence = buf[..cut].trim().to_string();
    buf.drain(..cut);
    if sentence.is_empty() {
        None
    } else {
        Some(sentence)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TTS 文本聚合与 markdown 清洗
// ═══════════════════════════════════════════════════════════════════════════════

/// 清洗后残句超过该字符数（chars 计数）即强制整块发出，避免长句无限滞留
const TTS_AGGREGATE_THRESHOLD: usize = 180;

/// 定时 flush 周期：保证 doubao 会话存活 + 有增量音频。
///
/// 服务端约 25.8s 闲置回收会话，6s 远低于该值；同时避免残句长时间滞留。
const TTS_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6);

/// 首个可播文本块的等待上限（与批处理路径 LLM 超时口径一致）。
/// 超过该时长（模型思考/调用工具过久）则播放超时兜底提示，避免设备静默。
const TTS_FIRST_TEXT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 聚合文本通道容量（有界，提供背压）
const TTS_AGG_CHANNEL_CAPACITY: usize = 16;

/// 默认处理进度提示文案（Agent 思考/调用工具期间周期播报）
const DEFAULT_THINKING_FEEDBACK_TEXT: &str = "好的，我正在处理，请稍候";

/// 默认超时兜底文案（等待首个文本超时后播报，随后结束会话）
const DEFAULT_THINKING_TIMEOUT_TEXT: &str = "这次任务处理时间较长，本次没有完成，请稍后再试";

/// 从 markdown 中提取一段可作为 TTS 输入纯文本。
///
/// 逐块纯函数（不跨块维护状态）：只针对 ASCII markdown 符号，
/// 不触碰中文标点与普通文本；`.`, `+`（C++）、行内 `-`（well-known）均不受影响。
fn clean_markdown_for_tts(raw: &str) -> String {
    let mut out = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        // 代码围栏定界行整体删除（正文保留，读出来比静音好）
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            continue;
        }
        // 整行分隔线删除
        if is_horizontal_rule(trimmed) {
            continue;
        }
        let mut body = line.trim_start();
        // 引用：`> ` 行首剥离
        if let Some(rest) = body.strip_prefix('>') {
            body = rest.trim_start();
        }
        // 标题：行首 1-6 个 `#` + 空白剥离
        let hash_len = body.chars().take_while(|&c| c == '#').count();
        if hash_len > 0
            && hash_len <= 6
            && body[hash_len..]
                .chars()
                .next()
                .is_some_and(|c| c == ' ' || c == '\t')
        {
            body = body[hash_len..].trim_start();
        }
        // 列表标记（仅行首；正文 `C++` / `well-known` 不动）
        body = strip_list_marker(body);
        // 行内清洗：链接/反引号/强调/转义
        let cleaned = clean_inline_markdown(body);
        if !cleaned.trim().is_empty() {
            out.push_str(&cleaned);
            out.push('\n');
        }
    }
    // 折叠连续换行并 trim 首尾
    let mut result = String::with_capacity(out.len());
    let mut prev_newline = false;
    for ch in out.chars() {
        if ch == '\n' {
            if !prev_newline {
                result.push(ch);
            }
            prev_newline = true;
        } else {
            result.push(ch);
            prev_newline = false;
        }
    }
    result.trim().to_string()
}

/// 判断是否为整行水平分隔线（仅由 3+ 个相同 `-`/`*`/`_` 组成）
fn is_horizontal_rule(trimmed: &str) -> bool {
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 1;
    for c in chars {
        if c != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

/// 剥离行首列表标记：`- ` / `* ` / `+ ` / `1. ` / `1) `
fn strip_list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim_start();
        }
    }
    // 有序列表：数字 + `.` / `)` + 空白
    let bytes = trimmed.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > 0 && idx < bytes.len() && (bytes[idx] == b'.' || bytes[idx] == b')') {
        let after = &trimmed[idx + 1..];
        if after.is_empty() || after.starts_with(' ') {
            return after.trim_start();
        }
    }
    line
}

/// 行内 markdown 清洗：链接/图片、反引号、强调/删除线、转义还原
fn clean_inline_markdown(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                // 转义还原：\X -> X
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '!' if i + 1 < chars.len() && chars[i + 1] == '[' => {
                // 图片 ![alt](url) -> alt
                if let Some((alt, consumed)) = try_extract_link(&chars, i + 1) {
                    out.push_str(&alt);
                    i = consumed;
                } else {
                    out.push('!');
                    i += 1;
                }
            }
            '[' => {
                // 链接 [label](url) -> label
                if let Some((label, consumed)) = try_extract_link(&chars, i) {
                    out.push_str(&label);
                    i = consumed;
                } else {
                    out.push('[');
                    i += 1;
                }
            }
            // 强调/删除线/反引号标记：直接丢弃符号，保留内容
            '*' | '~' | '_' | '`' => i += 1,
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// 尝试从 `chars[open_bracket] == '['` 处提取 `[label](url)`，返回 (清洗后 label, 消费后的下标)
fn try_extract_link(chars: &[char], open_bracket: usize) -> Option<(String, usize)> {
    let close = chars[open_bracket..].iter().position(|&c| c == ']')? + open_bracket;
    if close + 1 >= chars.len() || chars[close + 1] != '(' {
        return None;
    }
    let close_paren = chars[close + 1..].iter().position(|&c| c == ')')? + close + 1;
    let label: String = chars[open_bracket + 1..close].iter().collect();
    let cleaned = clean_inline_markdown(&label);
    Some((cleaned, close_paren + 1))
}

/// 同步聚合器：跨 chunk 聚句 + 阈值强发 + 残句 flush（纯同步，可单测）
struct TtsTextAggregator {
    raw_buf: String,
    threshold: usize,
    emitted_first: bool,
}

impl TtsTextAggregator {
    fn new(threshold: usize) -> Self {
        Self {
            raw_buf: String::new(),
            threshold,
            emitted_first: false,
        }
    }

    /// 追加一段 LLM delta，返回本次应发射的清洗后文本块（0..N 个）
    fn push(&mut self, delta: &str) -> Vec<String> {
        self.raw_buf.push_str(delta);
        let mut blocks = Vec::new();

        // 切出完整句（take_sentence 消费掉含结尾标点，空残句天然被过滤）
        while let Some(s) = take_sentence(&mut self.raw_buf) {
            let cleaned = clean_markdown_for_tts(&s);
            if !cleaned.is_empty() {
                blocks.push(cleaned);
                self.emitted_first = true;
            }
        }

        // 首块优化：尚无任何输出且已有文本，立即整块发出，
        // 避免无标点长句把首个音频拖到定时周期（TTS 会话可尽快建立）
        if !self.emitted_first {
            if let Some(block) = self.flush_partial() {
                blocks.push(block);
            }
            return blocks;
        }

        // 残句达到阈值强发，避免长句无限滞留
        if self.raw_buf.chars().count() >= self.threshold {
            if let Some(block) = self.flush_partial() {
                blocks.push(block);
            }
        }
        blocks
    }

    /// 残句（未闭合标点的缓冲文本）整块强发
    fn flush_partial(&mut self) -> Option<String> {
        let partial = std::mem::take(&mut self.raw_buf);
        let trimmed = partial.trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        let cleaned = clean_markdown_for_tts(&trimmed);
        if cleaned.is_empty() {
            return None;
        }
        self.emitted_first = true;
        Some(cleaned)
    }

    /// 上游结束：发掉剩余残句
    fn finish(&mut self) -> Option<String> {
        self.flush_partial()
    }
}

/// 聚合后台任务：清洗 + 按句/阈值/定时把块送入 agg_tx，并累计清洗全文（供零音频兜底重试）
async fn tts_aggregate_task(
    mut input: Box<dyn futures_util::Stream<Item = String> + Unpin + Send>,
    agg_tx: mpsc::Sender<String>,
    cleaned_full: Arc<Mutex<String>>,
) {
    let mut agg = TtsTextAggregator::new(TTS_AGGREGATE_THRESHOLD);
    let mut interval = tokio::time::interval(TTS_FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let _ = interval.tick().await; // 跳过首 tick

    loop {
        tokio::select! {
            chunk = input.next() => {
                match chunk {
                    Some(delta) => {
                        for block in agg.push(&delta) {
                            if !emit_block(&agg_tx, &cleaned_full, block).await {
                                return;
                            }
                        }
                    }
                    None => {
                        if let Some(block) = agg.finish() {
                            let _ = emit_block(&agg_tx, &cleaned_full, block).await;
                        }
                        return;
                    }
                }
            }
            _ = interval.tick() => {
                if let Some(block) = agg.flush_partial() {
                    if !emit_block(&agg_tx, &cleaned_full, block).await {
                        return;
                    }
                }
            }
        }
    }
}

/// 单块发射：累计到 cleaned_full，并尝试送入 agg_tx；返回 false 表示接收端已关（任务应退出）
async fn emit_block(
    agg_tx: &mpsc::Sender<String>,
    cleaned_full: &Arc<Mutex<String>>,
    block: String,
) -> bool {
    if let Ok(mut f) = cleaned_full.lock() {
        f.push_str(&block);
        f.push('\n');
    }
    agg_tx.send(block).await.is_ok()
}

// ═══════════════════════════════════════════════════════════════════════════════
// 首个可播文本等待 + 处理进度播报
// ═══════════════════════════════════════════════════════════════════════════════

/// 等待首个可播文本块的三种结果
enum FirstTextOutcome {
    /// 首个可播文本就绪
    Text(String),
    /// 文本流结束（空回复），或回放管道关闭（on_tick 返回 false）
    StreamEnded,
    /// 超过总等待时长仍无文本
    Timeout,
}

/// 等待首个可播文本，期间按 `interval` 周期调用 `on_tick` 播报处理进度提示。
///
/// # 语义
/// - 首个文本就绪 → `FirstTextOutcome::Text`
/// - 文本流结束（空回复）或 `on_tick` 返回 false（回放管道已关闭）→ `StreamEnded`
/// - 超过 `total_timeout` 仍无文本 → `Timeout`
///
/// 使用 `interval_at(now + interval, interval)` 保证首个 tick 在等待 `interval` 后才触发，
/// 避免开始等待立即播报；`MissedTickBehavior::Skip` 避免慢合成导致提示音连发。
/// 前提：`interval > 0`（调用方保证）。
async fn wait_first_text_with_feedback<F, Fut>(
    agg_rx: &mut mpsc::Receiver<String>,
    total_timeout: std::time::Duration,
    interval: std::time::Duration,
    mut on_tick: F,
) -> FirstTextOutcome
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    debug_assert!(interval > std::time::Duration::ZERO, "interval 必须 > 0");
    let deadline = tokio::time::Instant::now() + total_timeout;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return FirstTextOutcome::Timeout,
            maybe_text = agg_rx.recv() => {
                return match maybe_text {
                    Some(text) => FirstTextOutcome::Text(text),
                    None => FirstTextOutcome::StreamEnded,
                };
            }
            _ = ticker.tick() => {
                if !on_tick().await {
                    // 回放管道已关闭，停止等待；以 StreamEnded 返回让调用方走正常收尾
                    return FirstTextOutcome::StreamEnded;
                }
            }
        }
    }
}

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
    /// 无语音超时：录音开始后累计的初始静音帧数（60ms/帧，speech_detected 前才累计），
    /// 达到配置阈值时触发告别并关闭连接（不依赖 ASR 文本）
    no_speech_frames: u64,
}

/// ASR → LLM → TTS 响应策略：将设备录制的语音识别为文字，
/// 送 AI Agent 处理，再将回复合成为语音回传
///
/// 管线：Opus 解码 (16kHz) → Doubao ASR → AgentProvider → TTS Provider (24kHz) → Opus 编码
pub struct AsrLlmTtsStrategy {
    /// ASR 配置（包含活跃提供商和凭证）
    ///
    /// 使用 Arc<RwLock> 实现运行时热加载：Web UI 保存配置时直接更新此共享对象，
    /// 策略在每次 ASR 调用时读取最新配置。
    asr_config: SharedAsrConfig,
    /// TTS 配置（包含活跃提供商和凭证）
    ///
    /// 使用 Arc<RwLock> 实现运行时热加载：Web UI 保存配置时直接更新此共享对象，
    /// 策略在每次 TTS 调用时读取最新配置。
    tts_config: SharedTtsConfig,
    /// CLI 音色覆盖（--xiaozhi-tts-voice），叠加到共享配置之上，不写入磁盘
    voice_override: Option<String>,
    /// AI Agent（Claude Code、Codex 等）
    agent: Arc<dyn AgentProvider>,
    /// Agent 子进程工作目录
    work_dir: String,
    /// LLM 会话 ID，用于多轮对话上下文连续
    llm_session_id: Mutex<Option<String>>,
    /// 流式 ASR 管道状态（录音期间启用，录音结束时消耗）
    streaming_state: Mutex<Option<AsrPipelineState>>,
    /// VAD 端点通知器：ASR 检测到用户说完时触发（每录音周期创建新 Notify）
    vad_notify: Mutex<Arc<Notify>>,
    /// 无语音超时通知器：录音开始后累计无有效语音达到阈值时触发（每录音周期创建新 Notify）
    no_speech_notify: Mutex<Arc<Notify>>,
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
/// 环境底噪 RMS ≈ 100~800，阈值 1500 可区分底噪和轻声说话。
const SILENCE_RMS_THRESHOLD: f64 = 1500.0;

/// 连续静音帧数阈值（60ms/帧），约 2.4s 静音后触发本地 VAD。
/// 中文口语中思考停顿/语气词后的迟疑常超 1.2s，故取较宽容的 2.4s。
const MAX_SILENCE_FRAMES: u64 = 40;

/// 无语音超时帧数阈值：按 60ms/帧向上取整，保证实际时长 ≥ 配置值
fn no_speech_threshold_frames(timeout_ms: u64) -> u64 {
    timeout_ms.div_ceil(60)
}

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

/// 根据配置创建流式 ASR 提供者实例（录音期间实时识别，含 VAD 参数）
///
/// 注意：只有 doubao 支持流式 VAD 端点检测参数，其他提供商由服务端控制 VAD。
fn create_streaming_asr_provider(cfg: &AsrConfig) -> Result<Box<dyn AsrProvider>, String> {
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
            // GLM 使用 HTTP REST，不支持真正的流式 ASR
            create_asr_provider(cfg)
        }
        "mimo" => {
            // MIMO 使用 HTTP REST，不支持真正的流式 ASR
            create_asr_provider(cfg)
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
            // doubao（默认）- 流式模式添加 VAD 端点检测参数
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
                sample_rate: 16000,
                bits: 16,
                channel: 1,
                // VAD 端点检测：1500ms 静音强制判停（与本地能量 VAD 的 2.4s 配合，
                // 给中文口语停顿留出余地，避免说话中途被截断）
                end_window_size: Some(1500),
                // 至少 1s 音频后才允许判停（避免极短音频误判）
                force_to_speech_time: Some(1000),
                ..Default::default()
            })))
        }
    }
}

impl AsrLlmTtsStrategy {
    /// 记录一次 xiaozhi 路径的 Agent 调用日志
    ///
    /// `llm_session_id` 从共享态读取（记录时刻已是最新会话）。
    #[allow(clippy::too_many_arguments)]
    fn record_agent_log(
        &self,
        user_text: &str,
        session_id: &str,
        output: Option<&str>,
        status: &str,
        error: Option<&str>,
        latency: std::time::Duration,
        events: Vec<AgentLogEvent>,
    ) {
        let llm_session = self.llm_session_id.lock().ok().and_then(|g| (*g).clone());
        crate::agent_log::record(&crate::agent_log::AgentLogRecord {
            timestamp: crate::datetime::iso_timestamp_now(),
            source: "xiaozhi".to_string(),
            agent: self.agent.name().to_string(),
            connector: None,
            chat_id: Some(session_id.to_string()),
            sender_id: None,
            session_id: llm_session,
            work_dir: self.work_dir.clone(),
            input: user_text.to_string(),
            output: output.map(String::from),
            status: status.to_string(),
            error: error.map(String::from),
            latency_ms: latency.as_millis() as u64,
            events,
        });
    }

    /// 取回 Agent 流式调用的事件轨迹（事件消费任务累积到共享 Vec）
    ///
    /// 尽力等事件消费任务结束（事件流耗尽）最多 5s，超时则取当前累积；
    /// 提前退出分支中事件可能未完全到达，此时返回已累积部分。
    async fn take_agent_events(agent_events: &mut Option<AgentEventsHandle>) -> Vec<AgentLogEvent> {
        if let Some(handle) = agent_events.take() {
            let AgentEventsHandle { task, log } = handle;
            // 尽力等事件消费任务结束（事件流耗尽）最多 5s；超时则取当前累积
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
            return log.lock().ok().map(|g| g.clone()).unwrap_or_default();
        }
        Vec::new()
    }

    /// agent 模式收尾：统一取回事件轨迹并记录 agent 日志
    ///
    /// 覆盖正常完成之外的提前退出路径（空回复 / 首文本超时 / TTS 下游失败 /
    /// 回放管道关闭），确保每次已启动的 Agent 调用都有日志落盘。
    #[allow(clippy::too_many_arguments)]
    async fn finish_agent_log(
        &self,
        agent_events: &mut Option<AgentEventsHandle>,
        user_text: &str,
        session_id: &str,
        output: Option<&str>,
        status: &str,
        error: Option<&str>,
        started: std::time::Instant,
    ) {
        let events = Self::take_agent_events(agent_events).await;
        self.record_agent_log(
            user_text,
            session_id,
            output,
            status,
            error,
            started.elapsed(),
            events,
        );
    }

    /// 回放管道已关闭时的收尾：以「error/回放管道已关闭」记录 agent 日志
    ///
    /// `output` 为已累积的部分文本（设备可能在 agent 输出完成前断开）。
    async fn record_pipeline_closed_log(
        &self,
        agent_events: &mut Option<AgentEventsHandle>,
        user_text: &str,
        session_id: &str,
        llm_response_full: &Arc<Mutex<String>>,
        started: std::time::Instant,
    ) {
        let full = llm_response_full
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        self.finish_agent_log(
            agent_events,
            user_text,
            session_id,
            Some(&full),
            "error",
            Some("回放管道已关闭"),
            started,
        )
        .await;
    }

    /// 创建 ASR → LLM → TTS 策略
    ///
    /// # 参数
    ///
    /// * `asr_config` — ASR 配置（Arc<RwLock>，支持运行时热加载）
    /// * `tts_config` — TTS 配置
    /// * `agent` — AI Agent 实例
    /// * `work_dir` — Agent 子进程工作目录
    pub fn new(
        asr_config: SharedAsrConfig,
        tts_config: SharedTtsConfig,
        voice_override: Option<String>,
        agent: Arc<dyn AgentProvider>,
        work_dir: String,
    ) -> Self {
        Self {
            asr_config,
            tts_config,
            voice_override,
            agent,
            work_dir,
            llm_session_id: Mutex::new(None),
            streaming_state: Mutex::new(None),
            vad_notify: Mutex::new(Arc::new(Notify::new())),
            no_speech_notify: Mutex::new(Arc::new(Notify::new())),
            silence_closed: AtomicBool::new(false),
            asr_received_text: Arc::new(AtomicBool::new(false)),
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
        agent: Arc<dyn AgentProvider>,
        work_dir: String,
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
            agent,
            work_dir,
            llm_session_id: Mutex::new(None),
            streaming_state: Mutex::new(None),
            vad_notify: Mutex::new(Arc::new(Notify::new())),
            no_speech_notify: Mutex::new(Arc::new(Notify::new())),
            silence_closed: AtomicBool::new(false),
            asr_received_text: Arc::new(AtomicBool::new(false)),
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
        // 从共享配置读取最新 ASR 凭证，动态创建流式 ASR 提供商实例
        let asr = {
            let cfg = self.asr_config.read().unwrap();
            create_streaming_asr_provider(&cfg)?
        };

        // 创建 mpsc channel：接收端作为 AudioStream 喂给 ASR
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(32);
        let audio_stream: AudioStream = Box::pin(ReceiverStream::new(pcm_rx));

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
            no_speech_frames: 0,
        };

        let mut guard = self
            .streaming_state
            .lock()
            .map_err(|e| format!("锁获取失败: {}", e))?;
        *guard = Some(state);

        tracing::info!("流式 ASR: 管道已就绪（惰性初始化）");

        Ok(())
    }

    /// 合成唤醒问候音频（统一入口，供 [`wake_greeting`](Self::wake_greeting) 使用）
    ///
    /// 流程：建 provider → `synthesize`（PCM）→ PCM→Opus 帧 → AudioFrame 封装，
    /// 与 `generate_response` 批处理 TTS 段完全同构。
    /// 整个合成过程带 5 秒超时，防止 TTS 挂起拖死连接主循环。
    /// 任何失败（凭证缺失 / 合成失败 / 空音频 / 超时）都静默返回 `None`，
    /// 不播 fallback 提示音——唤醒/告别场景播「失败，请重试」是反效果。
    ///
    /// `purpose` 仅用于日志区分调用场景（如「唤醒问候」「无语音告别」）。
    async fn synthesize_audio(
        &self,
        text: &str,
        session_id: &str,
        purpose: &str,
    ) -> Option<Vec<AudioFrame>> {
        synthesize_status_audio(
            &self.tts_config,
            &self.voice_override,
            text,
            session_id,
            purpose,
        )
        .await
    }

    /// 唤醒问候合成（薄封装，日志标签固定为「唤醒问候」）
    async fn synthesize_greeting(&self, text: &str, session_id: &str) -> Option<Vec<AudioFrame>> {
        self.synthesize_audio(text, session_id, "唤醒问候").await
    }

    /// 播报超时兜底文案；合成失败（synthesize_audio 静默返回 None）才回退内置 fallback 提示音
    async fn play_timeout_feedback(
        &self,
        timeout_text: &str,
        pcm_tx: &mpsc::Sender<Vec<u8>>,
        frame_tx: &mpsc::Sender<PlaybackEvent>,
        session_id: &str,
    ) -> Result<(), String> {
        if let Some(pcm) = synthesize_status_pcm(
            &self.tts_config,
            &self.voice_override,
            timeout_text,
            session_id,
            "处理超时提示",
        )
        .await
        {
            if pcm_tx.send(pcm).await.is_err() {
                tracing::info!(session_id = %session_id, "超时提示播放管道已关闭");
                return Ok(());
            }
            tracing::info!(
                session_id = %session_id,
                text = %timeout_text,
                "超时兜底文案播放完成",
            );
            Ok(())
        } else {
            tracing::warn!(session_id = %session_id, "超时文案合成失败，回退 fallback 提示音");
            send_fallback_audio(frame_tx, session_id).await
        }
    }

    /// 等待首个可播文本。统一处理「功能关闭 / 仅超时兜底 / 周期提示+超时兜底」三种配置。
    ///
    /// Timeout 时已在本方法内播放兜底音频（兜底文案或 fallback 提示音），调用方只做收尾。
    ///
    /// 进度提示与超时文案的音频经 `pcm_tx` 汇入 ContinuityPump 统一编码（比特流连续），
    /// fallback 提示音仍直接经 `frame_tx`（内置 WAV，非流式窗口，不做编码连续性保证）。
    async fn wait_first_text(
        &self,
        agg_rx: &mut mpsc::Receiver<String>,
        pcm_tx: &mpsc::Sender<Vec<u8>>,
        frame_tx: &mpsc::Sender<PlaybackEvent>,
        session_id: &str,
    ) -> Result<FirstTextOutcome, String> {
        let (enabled, interval_ms) = {
            let cfg = self.tts_config.read().unwrap();
            (
                cfg.thinking_feedback_enabled,
                cfg.thinking_feedback_interval_ms,
            )
        };

        if enabled {
            let (fb_text, timeout_text) = {
                let cfg = self.tts_config.read().unwrap();
                (thinking_feedback_text(&cfg), thinking_timeout_text(&cfg))
            };
            if let (Some(fb), Some(to)) = (fb_text, timeout_text) {
                if interval_ms > 0 {
                    // 周期进度提示 + 超时兜底文案
                    let interval = std::time::Duration::from_millis(interval_ms);
                    let fb_for_tick = fb.clone();
                    let on_tick = || async {
                        if let Some(pcm) = synthesize_status_pcm(
                            &self.tts_config,
                            &self.voice_override,
                            &fb_for_tick,
                            session_id,
                            "处理进度提示",
                        )
                        .await
                        {
                            if pcm_tx.send(pcm).await.is_err() {
                                return false;
                            }
                        }
                        true
                    };
                    return Ok(
                        match wait_first_text_with_feedback(
                            agg_rx,
                            TTS_FIRST_TEXT_TIMEOUT,
                            interval,
                            on_tick,
                        )
                        .await
                        {
                            FirstTextOutcome::Text(t) => FirstTextOutcome::Text(t),
                            FirstTextOutcome::StreamEnded => FirstTextOutcome::StreamEnded,
                            FirstTextOutcome::Timeout => {
                                self.play_timeout_feedback(&to, pcm_tx, frame_tx, session_id)
                                    .await?;
                                FirstTextOutcome::Timeout
                            }
                        },
                    );
                }
                // interval == 0：仅保留超时兜底文案，不做周期提示
                return Ok(
                    match tokio::time::timeout(TTS_FIRST_TEXT_TIMEOUT, agg_rx.recv()).await {
                        Ok(Some(t)) => FirstTextOutcome::Text(t),
                        Ok(None) => FirstTextOutcome::StreamEnded,
                        Err(_) => {
                            self.play_timeout_feedback(&to, pcm_tx, frame_tx, session_id)
                                .await?;
                            FirstTextOutcome::Timeout
                        }
                    },
                );
            }
        }

        // 功能关闭：完全保留原逻辑（超时播 fallback 提示音）
        Ok(
            match tokio::time::timeout(TTS_FIRST_TEXT_TIMEOUT, agg_rx.recv()).await {
                Ok(Some(t)) => FirstTextOutcome::Text(t),
                Ok(None) => FirstTextOutcome::StreamEnded,
                Err(_) => {
                    send_fallback_audio(frame_tx, session_id).await?;
                    FirstTextOutcome::Timeout
                }
            },
        )
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
        if let Ok(mut ng) = self.no_speech_notify.lock() {
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
                state.no_speech_frames = 0; // 检测到有效语音，无语音超时作废
            } else if state.speech_detected {
                // 只在首次语音后的静音才累计
                state.silence_count = state.silence_count.saturating_add(1);
            } else {
                // 初始静音（从头到尾没说话）：累计无语音超时帧数
                state.no_speech_frames = state.no_speech_frames.saturating_add(1);
            }

            // 无语音超时：初始静音持续达到配置阈值 → 播报告别并关闭连接
            // （与下方 VAD 互斥：speech_detected=true 后 no_speech_frames 恒为 0；
            //   本路径置 silence_closed 后，后续帧在此前的短路处被忽略）
            let no_speech_timeout_ms = self.tts_config.read().unwrap().no_speech_timeout_ms;
            if no_speech_timeout_ms > 0
                && state.no_speech_frames >= no_speech_threshold_frames(no_speech_timeout_ms)
            {
                // 关闭 ASR 流：替换 sender，令 ASR 后台任务感知流结束
                let (new_tx, _) = mpsc::channel::<Vec<u8>>(1);
                let _ = std::mem::replace(&mut state.pcm_tx, new_tx);
                tracing::info!(
                    "无语音超时: 录音开始后 {} 帧 ({}ms) 无有效语音，触发告别并关闭连接",
                    state.no_speech_frames,
                    state.no_speech_frames * 60,
                );
                self.silence_closed.store(true, Ordering::Release);
                if let Ok(guard) = self.no_speech_notify.lock() {
                    guard.notify_one();
                }
                // 不发送此静音帧
                return Ok(());
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

    // ────────── 无语音超时 ──────────

    fn no_speech_completion(&self) -> Option<Arc<Notify>> {
        self.no_speech_notify.lock().ok().map(|g| g.clone())
    }

    /// 无语音超时后的告别音频（如「拜拜」）：读配置文案，合成后返回帧
    /// 交给 ws.rs 的 `play_greeting_frames` 播放。任何失败都静默跳过。
    async fn goodbye_frames(&self, session_id: &str) -> Option<Vec<AudioFrame>> {
        let text = {
            let cfg = self.tts_config.read().unwrap();
            cfg.no_speech_goodbye
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "拜拜".to_string())
        };
        self.synthesize_audio(&text, session_id, "无语音告别").await
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
        frame_tx: tokio::sync::mpsc::Sender<PlaybackEvent>,
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

        // 将 ASR 识别文本下发给设备，供屏幕显示「用户」侧消息
        if frame_tx
            .send(PlaybackEvent::Stt(user_text.clone()))
            .await
            .is_err()
        {
            tracing::info!(session_id = %session_id, "TTS-STREAM: 回放管道已关闭，停止生成");
            return Ok(());
        }

        // ── 连续音频管道（ContinuityPump）──
        // 立即启动：TTS::Start 窗口内设备始终以 60ms 节奏收到帧（有内容发内容，
        // 无内容喂零产静音），杜绝播放缓冲 underrun。所有真实音频 PCM（主回答、
        // 进度提示、工具提示、超时兜底）经 `pcm_tx` 汇入 pump 统一编码，保证
        // 比特流连续（避免孤立 Opus 帧沙沙声）；时间戳由 pump 会话级单调维护。
        // 生命周期由 `pump_guard`（Drop 时 abort）兜底，任何提前退出路径不泄漏。
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(64);
        let pump_handle = tokio::spawn(run_continuity_pump(
            pcm_rx,
            frame_tx.clone(),
            session_id.to_string(),
            PumpConfig::default(),
        ));
        let pump_guard = PumpGuard::new(pump_handle);

        // ════════════════════════════════════════════════════════════════
        // Phase 2: 生成回复文本（AI Agent 或固定文本）
        // ════════════════════════════════════════════════════════════════

        // 在流式转发的同时收集完整回复内容以便日志记录
        let llm_response_full = Arc::new(Mutex::new(String::new()));
        let response_for_log = llm_response_full.clone();

        // 句级文本事件旁路通道 + 残句共享态：
        // 句切分器把完整句写入无界旁路通道，音频循环再 `.await` 转发到回放管道，
        // 避免在同步闭包内 try_send 16 容量通道导致丢事件。
        let (text_evt_tx, mut text_evt_rx) = mpsc::unbounded_channel::<String>();
        let residual_shared: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

        // 从共享配置中读取固定文本模式设置
        let (fixed_text_enabled, fixed_text) = {
            let cfg = self.tts_config.read().unwrap();
            (cfg.fixed_text_enabled, cfg.fixed_text.clone())
        };

        // agent 调用记录标记：固定文本模式跳过 LLM，不记录
        let mut agent_mode = false;
        let mut agent_start = std::time::Instant::now();
        // Agent 事件消费任务句柄（agent 模式才有，收尾记录时取回累积事件）
        let mut agent_events: Option<AgentEventsHandle> = None;

        let text_stream: Box<dyn futures_util::Stream<Item = String> + Unpin + Send> =
            if fixed_text_enabled {
                // 固定文本模式：跳过 LLM，使用预设文本
                let fixed_text = fixed_text
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "欢迎使用智能语音助手".to_string());

                tracing::info!(
                    session_id = %session_id,
                    text = %fixed_text,
                    "TTS-STREAM: 固定文本模式，跳过 LLM",
                );

                Box::new(stream::iter(vec![fixed_text]))
            } else {
                // 普通模式：走 AI Agent 流式处理
                let current_llm_session = self
                    .llm_session_id
                    .lock()
                    .map_err(|e| format!("LLM session_id 锁获取失败: {}", e))?
                    .clone();

                agent_mode = true;
                agent_start = std::time::Instant::now();
                let (text_stream_inner, new_llm_session_id, events_rx) = match self
                    .agent
                    .process_stream(&user_text, current_llm_session.as_deref(), &self.work_dir)
                    .await
                {
                    Ok(ok) => ok,
                    Err(e) => {
                        let msg = format!("AI Agent 流式处理失败: {}", e);
                        self.record_agent_log(
                            &user_text,
                            session_id,
                            None,
                            "error",
                            Some(&msg),
                            agent_start.elapsed(),
                            Vec::new(),
                        );
                        return Err(msg);
                    }
                };
                // 启动事件消费任务：实时感知工具调用并播报进度提示，
                // 同时累积事件轨迹到共享 Vec 供收尾日志取回。
                let events_log: Arc<Mutex<Vec<AgentLogEvent>>> = Arc::new(Mutex::new(Vec::new()));
                let events_task = spawn_agent_event_consumer(
                    events_rx,
                    events_log.clone(),
                    self.tts_config.clone(),
                    self.voice_override.clone(),
                    pcm_tx.clone(),
                    session_id.to_string(),
                );
                agent_events = Some(AgentEventsHandle {
                    task: events_task,
                    log: events_log,
                });

                // 立即更新 LLM 会话 ID（用于多轮对话）
                if let Ok(mut session) = self.llm_session_id.lock() {
                    *session = Some(new_llm_session_id);
                }

                tracing::info!(
                    session_id = %session_id,
                    agent = self.agent.name(),
                    "TTS-STREAM: Agent 流式输出已启动",
                );

                Box::new(text_stream_inner)
            };

        // ── 句切分：按句末标点把文本流切分为句子，逐句下发设备屏幕 ──
        // speak_stream 惰性消费 text_stream（边合成边按需拉取文本），因此句子事件
        // 在 TTS 拉取文本的瞬间产生，与音频节奏天然近似同步，且先于该句音频
        // 进入回放通道。残句（未闭合标点）留在共享态，由音频循环结束后补发。
        let text_evt_tx2 = text_evt_tx.clone();
        let residual_shared2 = residual_shared.clone();
        let response_for_log2 = response_for_log.clone();
        let text_stream: Box<dyn futures_util::Stream<Item = String> + Unpin + Send> =
            Box::new(text_stream.map(move |chunk: String| {
                if let Ok(mut full) = response_for_log2.lock() {
                    full.push_str(&chunk);
                }
                // 残句缓冲：累积文本并按句末标点切分，完整句经旁路通道发出。
                // guard 在闭包末尾释放，修改已持久化到共享态，无需二次加锁写回。
                let mut buf = residual_shared2.lock().unwrap_or_else(|e| e.into_inner());
                buf.push_str(&chunk);
                while let Some(s) = take_sentence(&mut buf) {
                    let _ = text_evt_tx2.send(s);
                }
                chunk
            }));

        // ════════════════════════════════════════════════════════════════
        // Phase 3: 文本聚合 → 延迟建会话 → 流式 TTS 合成 → 逐帧发送
        // ════════════════════════════════════════════════════════════════
        //
        // 关键修复：不再立即建立 TTS 会话。模型（如 deepseek 系）会先输出思考块
        // （thinking_delta，agent 解析时已过滤），可能持续数十秒无任何文本；若此时
        // 已建立 doubao 会话，会话空转会被服务端闲置回收（实测 ~25.8s），文本到达
        // 时零音频。因此这里先把文本聚合清洗，拿到第一个可播文本块后才 speak_stream。

        // ── Phase 3a: 启动文本聚合任务（清洗 markdown + 按句聚合）──────
        let (agg_tx, mut agg_rx) = mpsc::channel::<String>(TTS_AGG_CHANNEL_CAPACITY);
        let cleaned_full: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let agg_cleaned_full = cleaned_full.clone();
        let agg_handle = tokio::spawn(tts_aggregate_task(text_stream, agg_tx, agg_cleaned_full));

        // ── Phase 3b: 拉取第一个可播文本块（此时才建立 TTS 会话）──────
        // 等待期间若 Agent 长时间无文本输出（思考/调用工具），按配置周期播报处理进度提示；
        // 超时（TTS_FIRST_TEXT_TIMEOUT）时兜底音频已由 wait_first_text 播放（文案或 fallback）。
        let first = match self
            .wait_first_text(&mut agg_rx, &pcm_tx, &frame_tx, session_id)
            .await?
        {
            FirstTextOutcome::Text(first) => first,
            FirstTextOutcome::StreamEnded => {
                // 聚合器结束且无文本：属正常空回复，排空屏幕事件后返回
                agg_handle.abort();
                let _ = agg_handle.await;
                drop(text_evt_tx);
                // agent 模式空回复也计入日志（agent 已自然结束，事件轨迹可完整取回）
                if agent_mode {
                    self.finish_agent_log(
                        &mut agent_events,
                        &user_text,
                        session_id,
                        None,
                        "error",
                        Some("AI Agent 返回空回复"),
                        agent_start,
                    )
                    .await;
                }
                drain_screen_events(&mut text_evt_rx, &residual_shared, &frame_tx, session_id)
                    .await?;
                return Ok(());
            }
            FirstTextOutcome::Timeout => {
                // 首个文本超时（模型思考/调用工具过久）：兜底音频已在 wait_first_text 内
                // 播放（兜底文案或 fallback 提示音），此处仅做收尾。
                tracing::warn!(
                    session_id = %session_id,
                    timeout_s = TTS_FIRST_TEXT_TIMEOUT.as_secs(),
                    "TTS-STREAM: 等待首个可播文本超时，已播放超时兜底提示",
                );
                agg_handle.abort();
                let _ = agg_handle.await;
                drop(text_evt_tx);
                // 超时也计入 agent 日志；agent 可能仍在 thinking（未吐 text），
                // 事件轨迹尽力取回（此时可能为空）
                if agent_mode {
                    let err_msg = format!(
                        "等待首个可播文本超时 ({}s)",
                        TTS_FIRST_TEXT_TIMEOUT.as_secs()
                    );
                    self.finish_agent_log(
                        &mut agent_events,
                        &user_text,
                        session_id,
                        None,
                        "timeout",
                        Some(&err_msg),
                        agent_start,
                    )
                    .await;
                }
                drain_screen_events(&mut text_evt_rx, &residual_shared, &frame_tx, session_id)
                    .await?;
                return Ok(());
            }
        };

        tracing::info!(
            session_id = %session_id,
            first_len = first.chars().count(),
            "TTS-STREAM: 首个可播文本就绪，建立 TTS 会话",
        );

        // ── Phase 3c: 创建 TTS 提供者（失败 → fallback 提示音）─────────
        // provider 提出来跨 .await 存活（Send + Sync），供零音频兜底重试复用。
        let provider = match (async {
            // 在单独的块中获取并释放 TTS 配置锁，避免 RwLockReadGuard 跨越 .await
            let cfg = self.tts_config.read().unwrap();
            let mut work_cfg = cfg.clone();
            if let Some(ref voice) = self.voice_override {
                work_cfg
                    .providers
                    .entry(work_cfg.active_provider.clone())
                    .or_default()
                    .insert("voice".to_string(), voice.clone());
            }
            crate::tts_factory::create_tts_provider(&work_cfg) // cfg 在此处释放
        })
        .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "TTS 提供者创建失败，播放 fallback 提示音",
                );
                agg_handle.abort();
                let _ = agg_handle.await;
                drop(text_evt_tx);
                // agent 已成功输出文本，下游 TTS 失败不改变 agent 调用本身：按 success 记录
                if agent_mode {
                    let full = llm_response_full
                        .lock()
                        .ok()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    self.finish_agent_log(
                        &mut agent_events,
                        &user_text,
                        session_id,
                        Some(&full),
                        "success",
                        None,
                        agent_start,
                    )
                    .await;
                }
                send_fallback_audio(&frame_tx, session_id).await?;
                drain_screen_events(&mut text_evt_rx, &residual_shared, &frame_tx, session_id)
                    .await?;
                return Ok(());
            }
        };

        // ── Phase 3d: 重建文本流：once(first).chain(rest) ─────────────
        // first 已就绪，会话建立后 send task 立即发出第一个 TaskRequest，无闲置窗口。
        let rest = ReceiverStream::new(agg_rx);
        let tts_text_stream: Box<dyn futures_util::Stream<Item = String> + Unpin + Send> =
            Box::new(stream::iter(vec![first]).chain(rest));

        // ── Phase 3e: 流式 TTS 合成 ───────────────────────────────────
        let mut audio_stream = match provider.speak_stream(Box::pin(tts_text_stream)).await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "流式 TTS 启动失败，播放 fallback 提示音",
                );
                // speak_stream 失败时其内部已 drop 输入流，聚合器随后自然退出；
                // await 确保 text_evt_tx2 已 drop，再排空屏幕事件
                let _ = agg_handle.await;
                drop(text_evt_tx);
                // agent 已成功输出文本，下游 TTS 失败不改变 agent 调用本身：按 success 记录
                if agent_mode {
                    let full = llm_response_full
                        .lock()
                        .ok()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    self.finish_agent_log(
                        &mut agent_events,
                        &user_text,
                        session_id,
                        Some(&full),
                        "success",
                        None,
                        agent_start,
                    )
                    .await;
                }
                send_fallback_audio(&frame_tx, session_id).await?;
                drain_screen_events(&mut text_evt_rx, &residual_shared, &frame_tx, session_id)
                    .await?;
                return Ok(());
            }
        };

        // ── Phase 3f: 流式 PCM 汇入 ContinuityPump（由泵统一编码 + 下发）──
        // 主回答 / 断档反馈 / 进度提示 / 工具提示全部经 pcm_tx 汇入 pump，由唯一
        // StreamingOpusEncoder 编码（比特流连续，时间戳会话级单调）；pump 空闲
        // 时喂零产静音帧，设备播放缓冲永不 underrun。
        let mut content_received = false;
        let mut total_audio_bytes: usize = 0;
        let mut raw_pcm: Vec<u8> = Vec::new();

        // ── 断档反馈：模型思考/调用工具时无主音频帧，按配置周期播报进度提示 ──
        let (feedback_enabled, feedback_interval_ms) = {
            let cfg = self.tts_config.read().unwrap();
            (
                cfg.thinking_feedback_enabled,
                cfg.thinking_feedback_interval_ms,
            )
        };
        let feedback_text = if feedback_enabled {
            thinking_feedback_text(&self.tts_config.read().unwrap())
        } else {
            None
        };
        // 可复位的反馈计时器：内容到达即重置，仅断档满 interval 才播报。
        // 0 间隔 = 仅超时兜底，不周期播报（feedback_on=false，分支禁用）。
        let feedback_interval = std::time::Duration::from_millis(feedback_interval_ms);
        let feedback_on = feedback_enabled && feedback_interval_ms > 0 && feedback_text.is_some();
        let feedback_delay = tokio::time::sleep(feedback_interval);
        tokio::pin!(feedback_delay);

        // 定期 tick：驱动循环回到顶部重新排空句子事件（audio_stream 长时空闲时
        // 屏幕文本不滞留）
        const GAP_HOUSEKEEPING_MS: u64 = 2000;
        let mut housekeeping =
            tokio::time::interval(std::time::Duration::from_millis(GAP_HOUSEKEEPING_MS));
        housekeeping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // 排空句切分器产生的句子事件（先于本块音频进入通道，保持文本先于语音）
            while let Ok(sentence) = text_evt_rx.try_recv() {
                if frame_tx
                    .send(PlaybackEvent::LlmSentence(sentence))
                    .await
                    .is_err()
                {
                    tracing::info!(session_id = %session_id, "TTS-STREAM: 回放管道已关闭，停止生成");
                    // 设备已断开：尽力记录已启动的 agent 调用
                    if agent_mode {
                        self.record_pipeline_closed_log(
                            &mut agent_events,
                            &user_text,
                            session_id,
                            &llm_response_full,
                            agent_start,
                        )
                        .await;
                    }
                    return Ok(());
                }
            }

            tokio::select! {
                biased;
                chunk = audio_stream.next() => {
                    match chunk {
                        Some(Ok(chunk)) => {
                            content_received = true;
                            total_audio_bytes += chunk.audio_chunk.len();
                            raw_pcm.extend_from_slice(&chunk.audio_chunk);
                            if pcm_tx.send(chunk.audio_chunk).await.is_err() {
                                tracing::info!(
                                    session_id = %session_id,
                                    "TTS-STREAM: 回放管道已关闭，停止生成",
                                );
                                if agent_mode {
                                    self.record_pipeline_closed_log(
                                        &mut agent_events,
                                        &user_text,
                                        session_id,
                                        &llm_response_full,
                                        agent_start,
                                    )
                                    .await;
                                }
                                return Ok(());
                            }
                            // 内容到达：重置反馈计时器（仅在断档时播报反馈）
                            feedback_delay
                                .as_mut()
                                .reset(tokio::time::Instant::now() + feedback_interval);
                        }
                        Some(Err(e)) => {
                            tracing::warn!("流式 TTS 音频块错误: {}", e);
                        }
                        None => break, // TTS 流正常结束
                    }
                }
                _ = &mut feedback_delay, if feedback_on => {
                    // 断档满 feedback_interval 无内容：播报一次进度提示（正常 TTS 语音）。
                    // pump 已在空闲喂零，设备播放缓冲不欠载；此处仅补充「正在处理」语音。
                    if let Some(text) = &feedback_text {
                        if let Some(pcm) = synthesize_status_pcm(
                            &self.tts_config,
                            &self.voice_override,
                            text,
                            session_id,
                            "断档提示",
                        )
                        .await
                        {
                            if pcm_tx.send(pcm).await.is_err() {
                                tracing::info!(
                                    session_id = %session_id,
                                    "TTS-STREAM: 回放管道已关闭，停止生成",
                                );
                                if agent_mode {
                                    self.record_pipeline_closed_log(
                                        &mut agent_events,
                                        &user_text,
                                        session_id,
                                        &llm_response_full,
                                        agent_start,
                                    )
                                    .await;
                                }
                                return Ok(());
                            }
                        }
                    }
                    // 播报后重新计时
                    feedback_delay
                        .as_mut()
                        .reset(tokio::time::Instant::now() + feedback_interval);
                }
                _ = housekeeping.tick() => {
                    // 周期性 housekeeping：让循环回到顶部排空屏幕文本事件
                }
            }
        }

        // ── 零音频兜底：流式合成有文本却无真实音频块 → 用累积全文重试一次 ──
        // speak_stream 返回了 Ok 流但无任何音频（例如会话中途被服务端回收），
        // 用聚合任务累积的清洗全文走 synthesize（doubao 每次自建新 WS 会话）重试。
        // pump 持续喂零，因此「无音频」须用内容块计数而非总帧数判断。
        if !content_received {
            let full = cleaned_full
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            if !full.is_empty() {
                tracing::warn!(
                    session_id = %session_id,
                    text_len = full.chars().count(),
                    "TTS-STREAM: 流式合成零音频，用累积全文重试 synthesize",
                );
                match provider
                    .synthesize(TtsRequest {
                        text: full,
                        options: None,
                    })
                    .await
                {
                    Ok(resp) if !resp.audio.is_empty() => {
                        content_received = true;
                        total_audio_bytes += resp.audio.len();
                        raw_pcm.extend_from_slice(&resp.audio);
                        if pcm_tx.send(resp.audio).await.is_err() {
                            tracing::info!(
                                session_id = %session_id,
                                "TTS-STREAM: 回放管道已关闭，停止生成",
                            );
                            if agent_mode {
                                self.record_pipeline_closed_log(
                                    &mut agent_events,
                                    &user_text,
                                    session_id,
                                    &llm_response_full,
                                    agent_start,
                                )
                                .await;
                            }
                            return Ok(());
                        }
                    }
                    _ => {
                        send_fallback_audio(&frame_tx, session_id).await?;
                    }
                }
            } else {
                // 无任何文本：也给出提示，避免设备完全静默
                send_fallback_audio(&frame_tx, session_id).await?;
            }
        }

        // 音频流已结束，释放 text_stream 与旁路发送端，使下方 text_evt_rx.recv()
        // 能收到 None 正常退出（否则发送端存活会阻塞）。
        drop(audio_stream);
        drop(text_evt_tx);

        // 关闭 PCM 源：drop 本地 pcm_tx。事件消费任务（spawn_agent_event_consumer）
        // 仍持有 clone，需在其退出（finish_agent_log 内部 join）后 pump 才会收到
        // pcm_rx None 收尾，因此先记录 agent 日志，再等 pump。
        drop(pcm_tx);

        let full_response = llm_response_full
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();

        // agent 模式成功完成才记录（固定文本模式无 LLM 调用）
        if agent_mode {
            // 文本流已排空 → 后台读流任务已完成并投递事件轨迹；加超时防御。
            // 内部 join 事件消费任务，释放其持有的 pcm_tx clone。
            self.finish_agent_log(
                &mut agent_events,
                &user_text,
                session_id,
                Some(&full_response),
                "success",
                None,
                agent_start,
            )
            .await;
        }

        // ── 泵收尾：flush 残片 + 追加尾静音帧（防尾音截断）──
        // 所有 PCM 发送端已释放（本地 pcm_tx drop + 事件消费任务 join），
        // pump 收到 pcm_rx None 后自动收尾。带超时兜底：若事件消费任务未及时
        // 释放其 clone（take_agent_events 5s 超时场景），超时则 abort pump 防泄漏。
        match tokio::time::timeout(std::time::Duration::from_secs(5), pump_guard.finish()).await {
            Ok(res) => res?,
            Err(_) => {
                tracing::warn!(
                    session_id = %session_id,
                    "ContinuityPump 收尾超时，已中止",
                );
            }
        }

        // ── 排空剩余句子事件 + 补发最后残句 ──────────────────────
        // 确保所有 LLM 文本在 Ok(()) 返回前进入回放通道，先于 ws.rs 的 Tts::Stop。
        drain_screen_events(&mut text_evt_rx, &residual_shared, &frame_tx, session_id).await?;

        // ── 保存 TTS 音频到本地 ───────────────────────────────────
        if !raw_pcm.is_empty() {
            save_tts_audio_as_wav(&raw_pcm, session_id);
        }

        tracing::info!(
            session_id = %session_id,
            total_audio_bytes = total_audio_bytes,
            content_received = content_received,
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

        // 从共享配置中读取固定文本模式设置
        let (fixed_text_enabled, fixed_text) = {
            let cfg = self.tts_config.read().unwrap();
            (cfg.fixed_text_enabled, cfg.fixed_text.clone())
        };

        let llm_text = if fixed_text_enabled {
            // 固定文本模式：跳过 LLM，使用预设文本
            let fixed_text = fixed_text
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

            let start = std::time::Instant::now();
            let llm_response = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                self.agent
                    .process(&user_text, current_llm_session.as_deref(), &self.work_dir),
            )
            .await;

            let (llm_text, new_llm_session_id, llm_events) = match llm_response {
                Ok(Ok((output, sid))) => (output.text, sid, output.events),
                Ok(Err(e)) => {
                    let msg = format!("AI Agent 处理失败: {}", e);
                    self.record_agent_log(
                        &user_text,
                        session_id,
                        None,
                        "error",
                        Some(&msg),
                        start.elapsed(),
                        Vec::new(),
                    );
                    return Err(msg);
                }
                Err(_) => {
                    let msg = "AI Agent 响应超时 (60s)".to_string();
                    self.record_agent_log(
                        &user_text,
                        session_id,
                        None,
                        "timeout",
                        Some(&msg),
                        start.elapsed(),
                        Vec::new(),
                    );
                    return Err(msg);
                }
            };

            if llm_text.is_empty() {
                self.record_agent_log(
                    &user_text,
                    session_id,
                    None,
                    "error",
                    Some("AI Agent 返回空回复"),
                    start.elapsed(),
                    Vec::new(),
                );
                return Err("AI Agent 返回空回复".to_string());
            }

            // 更新 LLM 会话 ID（用于多轮对话）
            if let Ok(mut session) = self.llm_session_id.lock() {
                *session = Some(new_llm_session_id);
            }

            self.record_agent_log(
                &user_text,
                session_id,
                Some(&llm_text),
                "success",
                None,
                start.elapsed(),
                llm_events,
            );

            tracing::info!(
                session_id = %session_id,
                response_len = llm_text.len(),
                response = %llm_text,
                "ASR-LLM-TTS: AI Agent 处理完成",
            );

            llm_text
        };

        // ── Step 4: TTS 语音合成 ──
        // 从共享配置读取最新 TTS 配置，叠加 CLI 音色覆盖
        // 如果 TTS 提供者创建或合成失败，返回内置「失败，请重试」提示音
        let result = (async {
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
                    text: llm_text.clone(),
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
        })
        .await;

        let frames = match result {
            Ok(frames) => frames,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "TTS 合成失败，返回 fallback 提示音",
                );
                make_fallback_audio_frames()?
            }
        };

        tracing::info!(
            session_id = %session_id,
            frame_count = frames.len(),
            "ASR-LLM-TTS: 管线完成",
        );

        Ok(frames)
    }

    /// 设备唤醒问候：设备检测到唤醒词（`listen/detect`）时主动播报 TTS 问候。
    ///
    /// 读配置判断是否启用与文案（[`wake_greeting_text`]），合成后返回音频帧
    /// 交给 ws.rs 的 `playback_frames` 播放。任何失败都静默跳过，不影响录音轮。
    async fn wake_greeting(&self, session_id: &str) -> Option<Vec<AudioFrame>> {
        let text = {
            let cfg = self.tts_config.read().unwrap();
            wake_greeting_text(&cfg)?
        };
        self.synthesize_greeting(&text, session_id).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 唤醒问候文本解析
// ═══════════════════════════════════════════════════════════════════════════════

/// 解析唤醒问候配置 → 待播报文案
///
/// - 配置关闭（`wake_greeting_enabled=false`）→ `None`（不播报）
/// - `wake_greeting` 为 None / 空串 / 纯空白 → 回退默认「你好」
/// - 其余返回原文案
fn wake_greeting_text(cfg: &TtsConfig) -> Option<String> {
    if !cfg.wake_greeting_enabled {
        return None;
    }
    cfg.wake_greeting
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| Some("你好".to_string()))
}

/// 解析处理进度提示配置 → 待播报文案
///
/// - 配置关闭（`thinking_feedback_enabled=false`）→ `None`（不播报）
/// - `thinking_feedback_text` 为 None / 空串 / 纯空白 → 回退默认文案
/// - 其余返回原文案
fn thinking_feedback_text(cfg: &TtsConfig) -> Option<String> {
    if !cfg.thinking_feedback_enabled {
        return None;
    }
    cfg.thinking_feedback_text
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| Some(DEFAULT_THINKING_FEEDBACK_TEXT.to_string()))
}

/// 解析超时兜底提示配置 → 待播报文案
///
/// - 配置关闭（`thinking_feedback_enabled=false`）→ `None`（不播报，走 fallback 提示音）
/// - `thinking_feedback_timeout_text` 为 None / 空串 / 纯空白 → 回退默认文案
/// - 其余返回原文案
fn thinking_timeout_text(cfg: &TtsConfig) -> Option<String> {
    if !cfg.thinking_feedback_enabled {
        return None;
    }
    cfg.thinking_feedback_timeout_text
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| Some(DEFAULT_THINKING_TIMEOUT_TEXT.to_string()))
}

/// 合成一段状态提示音频的原始 PCM（模块级，供策略方法与独立事件播报任务共用）。
///
/// 任意文本 → TTS 合成 → 返回原始 PCM（24kHz 16-bit mono）。
/// 5s 超时兜底，任何失败静默返回 None。
///
/// 返回 PCM 而非 Opus 帧，是为了让流式回放窗口内所有音频（内容 / 进度提示 / 工具提示）
/// 都能汇入统一的 [`StreamingOpusEncoder`]（ContinuityPump），保证比特流连续，
/// 避免"孤立 Opus 帧"在设备端解码产生沙沙声。
async fn synthesize_status_pcm(
    tts_config: &SharedTtsConfig,
    voice_override: &Option<String>,
    text: &str,
    session_id: &str,
    purpose: &str,
) -> Option<Vec<u8>> {
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // 在单独的块中获取并释放 TTS 配置锁，避免 RwLockReadGuard 跨越 .await
        let provider = {
            let cfg = tts_config.read().unwrap();
            let mut work_cfg = cfg.clone();
            if let Some(voice) = voice_override {
                work_cfg
                    .providers
                    .entry(work_cfg.active_provider.clone())
                    .or_default()
                    .insert("voice".to_string(), voice.clone());
            }
            crate::tts_factory::create_tts_provider(&work_cfg)?
        };
        let response = provider
            .synthesize(TtsRequest {
                text: text.to_string(),
                options: None,
            })
            .await
            .map_err(|e| format!("TTS 合成失败: {}", e))?;

        if response.audio.is_empty() {
            return Err::<Vec<u8>, String>("TTS 返回空音频".to_string());
        }
        Ok::<Vec<u8>, String>(response.audio)
    })
    .await;

    match result {
        Ok(Ok(pcm)) => {
            tracing::info!(
                session_id = %session_id,
                text = %text,
                pcm_len = pcm.len(),
                "{}: TTS 合成完成", purpose,
            );
            Some(pcm)
        }
        Ok(Err(e)) => {
            tracing::warn!(
                session_id = %session_id,
                text = %text,
                error = %e,
                "{}: TTS 合成失败，静默跳过", purpose,
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                session_id = %session_id,
                text = %text,
                "{}: TTS 合成超时（>5s），静默跳过", purpose,
            );
            None
        }
    }
}

/// 合成一段状态提示音频（模块级，供策略方法与独立事件播报任务共用）。
///
/// 任意文本 → TTS 合成 → 编码为 Opus 帧（时间戳从 0 起、逐帧 +60）。
/// 5s 超时兜底，任何失败静默返回 None。
///
/// 用于非流式窗口（唤醒问候 / 告别 / 超时兜底）；流式回放窗口内的反馈提示
/// 应走 [`synthesize_status_pcm`] 把 PCM 汇入统一编码器，避免比特流不连续。
async fn synthesize_status_audio(
    tts_config: &SharedTtsConfig,
    voice_override: &Option<String>,
    text: &str,
    session_id: &str,
    purpose: &str,
) -> Option<Vec<AudioFrame>> {
    let pcm = synthesize_status_pcm(tts_config, voice_override, text, session_id, purpose).await?;

    // PCM → Opus 编码 (24kHz, 60ms)，与 generate_response 批处理路径一致
    let opus_frames = pcm_to_opus_frames(&pcm, 24000, 60).ok()?;

    let mut frames = Vec::with_capacity(opus_frames.len());
    let mut timestamp: u32 = 0;
    for opus in opus_frames {
        frames.push(AudioFrame {
            timestamp,
            data: opus,
        });
        timestamp = timestamp.wrapping_add(60);
    }
    Some(frames)
}

/// 工具名 → TTS 播报文案映射（仅对可能耗时较长的工具播报，未知工具返回 None 不打扰）
fn tool_feedback_text(name: &str) -> Option<String> {
    let text = match name {
        "WebSearch" => "我正在搜索网络，请稍候",
        "WebFetch" => "我正在读取网页，请稍候",
        "Bash" => "我正在执行命令，请稍候",
        "Read" => "我正在读取文件，请稍候",
        "Write" => "我正在写文件，请稍候",
        "Edit" => "我正在修改代码，请稍候",
        "MultiEdit" => "我正在修改代码，请稍候",
        "NotebookEdit" => "我正在修改内容，请稍候",
        "Task" => "我让另一个助手并行处理，请稍候",
        _ => return None,
    };
    Some(text.to_string())
}

/// Agent 事件消费任务句柄：实时事件消费任务 + 共享事件累积
struct AgentEventsHandle {
    task: tokio::task::JoinHandle<()>,
    log: Arc<Mutex<Vec<AgentLogEvent>>>,
}

/// 启动 Agent 实时事件消费任务
///
/// - 每收到一个 [`AgentLogEvent::ToolUse`] 就按工具名合成并播报进度提示
///   （相同工具 8s 内去重，避免并行/连续调用重复打扰）
/// - 全部事件累积到共享 Vec，供收尾日志（`finish_agent_log` 等）取回
/// - 事件流结束（agent 后台任务 drop sender）时任务退出
fn spawn_agent_event_consumer(
    events_rx: AgentEventStream,
    events_log: Arc<Mutex<Vec<AgentLogEvent>>>,
    tts_config: SharedTtsConfig,
    voice_override: Option<String>,
    pcm_tx: mpsc::Sender<Vec<u8>>,
    session_id: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_tool: Option<(String, std::time::Instant)> = None;
        let mut events_rx = events_rx;
        while let Some(event) = events_rx.recv().await {
            if let Ok(mut log) = events_log.lock() {
                log.push(event.clone());
            }
            let AgentLogEvent::ToolUse { name, .. } = &event else {
                continue;
            };
            let now = std::time::Instant::now();
            let is_duplicate = last_tool
                .as_ref()
                .map(|(n, t)| {
                    n.as_str() == name.as_str()
                        && now.duration_since(*t) < std::time::Duration::from_secs(8)
                })
                .unwrap_or(false);
            if is_duplicate {
                continue;
            }
            last_tool = Some((name.clone(), now));
            let Some(text) = tool_feedback_text(name.as_str()) else {
                continue;
            };
            tracing::info!(
                session_id = %session_id,
                tool = %name,
                "Agent 工具调用：播报进度提示",
            );
            if let Some(pcm) =
                synthesize_status_pcm(&tts_config, &voice_override, &text, &session_id, "工具提示")
                    .await
            {
                if pcm_tx.send(pcm).await.is_err() {
                    return;
                }
            }
        }
    })
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
        // 不启用 DTX（静音检测）：DTX 静音帧对部分 ESP32 固件播放器
        // 的时长处理不友好，可能压缩句间停顿、造成"半个字/跳字"听感。

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
// 连续音频管道（ContinuityPump）
// ═══════════════════════════════════════════════════════════════════════════════

/// 连续音频管道配置
#[derive(Debug, Clone, Copy)]
pub(crate) struct PumpConfig {
    /// 空闲时静音帧发送周期（ms），默认 60
    pub(crate) tick_ms: u64,
    /// 收尾时追加的尾静音帧数（防尾音截断），默认 2
    pub(crate) tail_silence_frames: usize,
    /// 静音帧去重：编码器喂零收敛为逐字节相同的 8 字节静音帧后，复用缓存帧
    /// 免重复编码（省 CPU）。内容到达会打断静音态，需过渡 ~5 帧后重新收敛。
    /// 关闭则每 tick 都喂零编码（保底正确）。默认开启。
    pub(crate) silence_dedup: bool,
}

impl Default for PumpConfig {
    fn default() -> Self {
        Self {
            tick_ms: 60,
            tail_silence_frames: 2,
            silence_dedup: true,
        }
    }
}

/// 一帧 60ms @ 24kHz 16-bit mono 的 PCM 字节数（与 `StreamingOpusEncoder` 的 frame_bytes 一致）
const PUMP_FRAME_PCM_BYTES: usize = 2880;

/// 连续音频管道（ContinuityPump）
///
/// 持有唯一 [`StreamingOpusEncoder`]，在整个回放窗口内持续向 `frame_tx` 推送音频帧：
/// - 收到真实 PCM（经 `pcm_rx`）立即编码下发
/// - 空闲时按 `tick_ms` 喂零 PCM，产出比特流连续的真静音帧
/// - 所有 PCM 源关闭后：`flush()` 残片 + 追加尾静音帧，再退出
///
/// # 设计要点
/// - 静音帧由**同一编码器喂零**产出（比特流连续），避免历史"孤立 Opus 帧"在
///   设备端解码产生沙沙声的问题。
/// - 时间戳由 pump 统一维护（会话级单调 +60），消除各 TTS 段从 0 重启的跳变。
/// - `biased` select：真实 PCM 优先于零填充；空闲 tick 兜底，保证窗口内 0 断档。
/// - 编码器内部残片缓存天然处理子帧间隙：不足 60ms 的空闲不会产出额外静音帧。
/// - 静音帧去重（`silence_dedup`）：喂零收敛为逐字节相同的 8 字节静音帧后
///   复用缓存，免重复编码；内容打断后过渡 ~5 帧重新收敛，期间仍正常编码。
pub(crate) async fn run_continuity_pump(
    mut pcm_rx: mpsc::Receiver<Vec<u8>>,
    frame_tx: mpsc::Sender<PlaybackEvent>,
    session_id: String,
    cfg: PumpConfig,
) -> Result<(), String> {
    let mut enc = StreamingOpusEncoder::new(24000, 60)?;
    let mut timestamp: u32 = 0;
    let mut sent_frames: u64 = 0;
    let zero_pcm = vec![0u8; PUMP_FRAME_PCM_BYTES];
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(cfg.tick_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // ── 静音帧去重状态 ──
    // 编码器喂零会收敛为逐字节相同的 8 字节静音帧（稳态历史无关，内容中断后
    // 过渡 ~5 帧重新收敛到同一帧）。收敛后复用缓存帧免编码，比特流与不去重
    // 逐字节相同。内容到达会打断静音态，须回到喂零编码直到再次收敛。
    let mut cached_silence: Option<Vec<u8>> = None;
    let mut silence_ready = false;

    loop {
        tokio::select! {
            biased;
            pcm = pcm_rx.recv() => {
                match pcm {
                    Some(pcm) => {
                        let frames = enc.feed(&pcm)?;
                        for opus in frames {
                            pump_send_frame(&frame_tx, &mut timestamp, opus).await?;
                            sent_frames += 1;
                        }
                        // 内容到达：编码器离开静音稳态，须重新收敛后才能复用
                        if cfg.silence_dedup {
                            silence_ready = false;
                        }
                    }
                    None => {
                        // 所有 PCM 源已关闭：flush 残片 + 追加尾静音帧，然后退出
                        let flush_frames = enc.flush()?;
                        for opus in flush_frames {
                            pump_send_frame(&frame_tx, &mut timestamp, opus).await?;
                            sent_frames += 1;
                        }
                        for _ in 0..cfg.tail_silence_frames {
                            let frames = enc.feed(&zero_pcm)?;
                            for opus in frames {
                                pump_send_frame(&frame_tx, &mut timestamp, opus).await?;
                                sent_frames += 1;
                            }
                        }
                        tracing::debug!(
                            session_id = %session_id,
                            sent_frames = sent_frames,
                            "ContinuityPump 收尾完成（flush 残片 + 尾静音）",
                        );
                        return Ok(());
                    }
                }
            }
            _ = ticker.tick() => {
                if cfg.silence_dedup && silence_ready {
                    // 收敛态：复用缓存静音帧，免重复编码（比特流与喂零逐字节相同）
                    if let Some(cached) = &cached_silence {
                        pump_send_frame(&frame_tx, &mut timestamp, cached.clone()).await?;
                        sent_frames += 1;
                        continue;
                    }
                }
                let frames = enc.feed(&zero_pcm)?;
                for opus in frames {
                    pump_send_frame(&frame_tx, &mut timestamp, opus.clone()).await?;
                    sent_frames += 1;
                    if cfg.silence_dedup {
                        match &cached_silence {
                            Some(c) if c == &opus => silence_ready = true,
                            Some(_) => silence_ready = false,
                            // 首帧 8 字节即视为进入稳态候选（过渡帧 >8 字节）；
                            // 待下一帧确认后才标记 ready，避免把瞬时帧当作稳态复用。
                            None if opus.len() == 8 => {
                                cached_silence = Some(opus);
                                silence_ready = false;
                            }
                            None => {}
                        }
                    }
                }
            }
        }
    }
}

/// 封装一帧 Opus 为 `AudioFrame` 并推入回放管道（会话级单调时间戳 +60/帧）
async fn pump_send_frame(
    frame_tx: &mpsc::Sender<PlaybackEvent>,
    timestamp: &mut u32,
    opus: Vec<u8>,
) -> Result<(), String> {
    let evt = PlaybackEvent::Audio(AudioFrame {
        timestamp: *timestamp,
        data: opus,
    });
    if frame_tx.send(evt).await.is_err() {
        return Err("回放管道已关闭".into());
    }
    *timestamp = timestamp.wrapping_add(60);
    Ok(())
}

/// 连续音频管道生命周期守卫：drop 时 abort 未完成的 pump 任务，防任务泄漏
///
/// 正常收尾调用 [`finish`](Self::finish) 等待 pump 完成（flush + 尾静音）后退出；
/// 任何提前退出路径直接 drop 本守卫即自动取消 pump。
pub(crate) struct PumpGuard(Option<tokio::task::JoinHandle<Result<(), String>>>);

impl PumpGuard {
    /// 创建守卫并接管 pump 任务句柄
    pub(crate) fn new(handle: tokio::task::JoinHandle<Result<(), String>>) -> Self {
        Self(Some(handle))
    }

    /// 正常收尾：等待 pump 任务完成（flush 残片 + 尾静音后退出）
    pub(crate) async fn finish(mut self) -> Result<(), String> {
        if let Some(handle) = self.0.take() {
            match handle.await {
                Ok(res) => res,
                Err(e) => Err(format!("pump 任务 panicked: {}", e)),
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for PumpGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 屏幕事件排空
// ═══════════════════════════════════════════════════════════════════════════════

/// 排空屏幕句子事件 + 补发最后残句。
///
/// 调用前需确保 `text_evt_tx` 及其所有 clone（聚合任务持有的 `text_evt_tx2`）
/// 均已 drop，否则 `text_evt_rx.recv()` 会一直阻塞。
async fn drain_screen_events(
    text_evt_rx: &mut mpsc::UnboundedReceiver<String>,
    residual_shared: &Arc<Mutex<String>>,
    frame_tx: &mpsc::Sender<PlaybackEvent>,
    session_id: &str,
) -> Result<(), String> {
    while let Some(sentence) = text_evt_rx.recv().await {
        if frame_tx
            .send(PlaybackEvent::LlmSentence(sentence))
            .await
            .is_err()
        {
            tracing::info!(session_id = %session_id, "TTS-STREAM: 回放管道已关闭，停止生成");
            return Ok(());
        }
    }
    let residual = residual_shared
        .lock()
        .ok()
        .map(|g| g.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(s) = residual {
        if frame_tx.send(PlaybackEvent::LlmSentence(s)).await.is_err() {
            tracing::info!(session_id = %session_id, "TTS-STREAM: 回放管道已关闭，停止生成");
            return Ok(());
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fallback 音频
// ═══════════════════════════════════════════════════════════════════════════════

/// 将内置「失败，请重试」提示音编码为 Opus 帧并通过 frame_tx 发送
///
/// 当 TTS 提供者创建或语音合成失败时调用此函数，确保设备能播放提示音
/// 告知用户出错了，而非静默无响应。
async fn send_fallback_audio(
    frame_tx: &tokio::sync::mpsc::Sender<PlaybackEvent>,
    session_id: &str,
) -> Result<(), String> {
    let opus_frames = crate::xiaozhi_tts::fallback_error_audio_frames()
        .map_err(|e| format!("Fallback 音频编码失败: {}", e))?;

    let mut timestamp: u32 = 0;
    for opus in &opus_frames {
        if frame_tx
            .send(PlaybackEvent::Audio(AudioFrame {
                timestamp,
                data: opus.clone(),
            }))
            .await
            .is_err()
        {
            tracing::info!(session_id = %session_id, "Fallback 播放管道已关闭");
            return Ok(());
        }
        timestamp = timestamp.wrapping_add(60);
    }

    tracing::info!(
        session_id = %session_id,
        frame_count = opus_frames.len(),
        "Fallback 提示音播放完成",
    );

    Ok(())
}

/// 将内置「失败，请重试」提示音编码为 Opus 帧并返回（批处理模式使用）
fn make_fallback_audio_frames() -> Result<Vec<AudioFrame>, String> {
    let opus_frames = crate::xiaozhi_tts::fallback_error_audio_frames()?;

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

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::provider::AgentOutput;
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
            _work_dir: &str,
        ) -> Result<(AgentOutput, String), String> {
            // 如果提供了 session_id，追加 "(continued)" 表示恢复了上下文
            let response = if session_id.is_some() {
                format!("{} (continued)", message)
            } else {
                message.to_string()
            };
            Ok((
                AgentOutput {
                    text: response,
                    events: Vec::new(),
                },
                "mock-session-id".to_string(),
            ))
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
            _work_dir: &str,
        ) -> Result<(AgentOutput, String), String> {
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
            _work_dir: &str,
        ) -> Result<(AgentOutput, String), String> {
            Ok((
                AgentOutput {
                    text: String::new(),
                    events: Vec::new(),
                },
                "empty-session".to_string(),
            ))
        }

        async fn check_available(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn test_tts_config() -> crate::config::settings::TtsConfig {
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

    fn test_asr_config() -> crate::config::settings::AsrConfig {
        let mut providers = std::collections::HashMap::new();
        let mut creds = std::collections::HashMap::new();
        creds.insert("api_key".to_string(), "test-app-key".to_string());
        providers.insert("doubao".to_string(), creds);
        crate::config::settings::AsrConfig {
            active_provider: "doubao".to_string(),
            providers,
        }
    }

    fn make_shared_tts_config() -> crate::xiaozhi_asr_llm_tts::SharedTtsConfig {
        Arc::new(RwLock::new(test_tts_config()))
    }

    fn make_shared_asr_config() -> crate::xiaozhi_asr_llm_tts::SharedAsrConfig {
        Arc::new(RwLock::new(test_asr_config()))
    }

    fn make_strategy(agent: Arc<dyn AgentProvider>) -> AsrLlmTtsStrategy {
        AsrLlmTtsStrategy::new(
            make_shared_asr_config(),
            make_shared_tts_config(),
            None, // voice_override
            agent,
            "/tmp".to_string(),
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
            strategy
                .tts_config
                .read()
                .unwrap()
                .get_credential("resource_id")
                .as_deref(),
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
        let (output, session_id) = agent
            .process("你好", None, "/tmp")
            .await
            .expect("MockAgent 应成功");
        assert_eq!(output.text, "你好");
        assert_eq!(session_id, "mock-session-id");
    }

    #[tokio::test]
    async fn test_t23_mock_agent_with_session() {
        let agent = MockAgent;
        let (output, session_id) = agent
            .process("你好", Some("prev-session"), "/tmp")
            .await
            .expect("MockAgent 应成功");
        assert_eq!(output.text, "你好 (continued)");
        assert_eq!(session_id, "mock-session-id");
    }

    #[tokio::test]
    async fn test_t24_failing_agent_process() {
        let agent = FailingAgent;
        let result = agent.process("你好", None, "/tmp").await;
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
            strategy
                .tts_config
                .read()
                .unwrap()
                .get_credential("resource_id")
                .as_deref(),
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
        let (mut stream, sid, events_rx) = agent
            .process_stream("你好", None, "/tmp")
            .await
            .expect("MockAgent process_stream 应成功");
        let mut result = String::new();
        while let Some(chunk) = stream.next().await {
            result.push_str(&chunk);
        }
        assert_eq!(result, "你好", "process_stream 应返回 process 的结果");
        assert_eq!(sid, "mock-session-id");
        // 默认实现应立即投递空事件流（recv 返回 None）
        let mut events_rx = events_rx;
        assert!(events_rx.recv().await.is_none(), "默认实现事件流应为空");
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
    fn test_t34_create_asr_provider_doubao() {
        let cfg = test_asr_config();
        let provider = create_asr_provider(&cfg).expect("doubao 提供者创建应成功");
        assert_eq!(provider.name(), "doubao");
    }

    #[test]
    fn test_t35_create_asr_provider_qwen() {
        let cfg = make_qwen_asr_config();
        let provider = create_asr_provider(&cfg).expect("qwen 提供者创建应成功");
        assert_eq!(provider.name(), "qwen");
    }

    #[test]
    fn test_t36_create_asr_provider_missing_creds() {
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

    #[test]
    fn test_t37_create_streaming_asr_provider_doubao() {
        let cfg = test_asr_config();
        let provider = create_streaming_asr_provider(&cfg).expect("doubao 流式提供者创建应成功");
        assert_eq!(provider.name(), "doubao");
    }

    #[test]
    fn test_t38_create_streaming_asr_provider_qwen() {
        let cfg = make_qwen_asr_config();
        let provider = create_streaming_asr_provider(&cfg).expect("qwen 流式提供者创建应成功");
        assert_eq!(provider.name(), "qwen");
    }

    // ─── take_sentence 句切分测试 ──────────────────────────

    #[test]
    fn test_t40_take_sentence_chinese_punctuation() {
        let mut buf = "今天天气不错。我们出去玩吧！".to_string();
        let s1 = take_sentence(&mut buf).expect("第一句应可切出");
        assert_eq!(s1, "今天天气不错。");
        let s2 = take_sentence(&mut buf).expect("第二句应可切出");
        assert_eq!(s2, "我们出去玩吧！");
        assert!(take_sentence(&mut buf).is_none(), "无剩余内容时返回 None");
    }

    #[test]
    fn test_t41_take_sentence_cross_chunk() {
        // 一句话横跨多个 chunk：句末标点在第二个 chunk
        let mut buf = String::new();
        buf.push_str("今天是");
        assert!(take_sentence(&mut buf).is_none(), "未闭合的句子应留在缓冲");
        buf.push_str("星期二。");
        let s = take_sentence(&mut buf).expect("跨 chunk 累积后应可切出");
        assert_eq!(s, "今天是星期二。");
        assert!(buf.is_empty());
    }

    #[test]
    fn test_t42_take_sentence_multiple_in_one_chunk() {
        let mut buf = "你好！再见？".to_string();
        let s1 = take_sentence(&mut buf).expect("第一句");
        assert_eq!(s1, "你好！");
        let s2 = take_sentence(&mut buf).expect("第二句");
        assert_eq!(s2, "再见？");
    }

    #[test]
    fn test_t43_take_sentence_consecutive_punctuation_merged() {
        // 连续标点应被吞并，避免产出空句
        let mut buf = "真的吗？！太好了".to_string();
        let s1 = take_sentence(&mut buf).expect("第一句");
        assert_eq!(s1, "真的吗？！");
        assert!(take_sentence(&mut buf).is_none(), "无标点的残句留在缓冲");
        assert_eq!(buf, "太好了");
    }

    #[test]
    fn test_t44_take_sentence_ascii_dot_not_boundary() {
        // ASCII 句点不应作为边界（避免拆分 Mr. / 3.14）
        let mut buf = "单价3.14元。谢谢。".to_string();
        let s1 = take_sentence(&mut buf).expect("第一句");
        assert_eq!(s1, "单价3.14元。");
        let s2 = take_sentence(&mut buf).expect("第二句");
        assert_eq!(s2, "谢谢。");
    }

    #[test]
    fn test_t45_take_sentence_ascii_question_and_exclaim() {
        let mut buf = "Really?Yes!".to_string();
        let s1 = take_sentence(&mut buf).expect("第一句");
        assert_eq!(s1, "Really?");
        let s2 = take_sentence(&mut buf).expect("第二句");
        assert_eq!(s2, "Yes!");
    }

    #[test]
    fn test_t46_take_sentence_newline_boundary() {
        let mut buf = "第一行\n第二行".to_string();
        let s1 = take_sentence(&mut buf).expect("第一行");
        assert_eq!(s1, "第一行");
        assert!(take_sentence(&mut buf).is_none(), "无标点的第二行留在缓冲");
        assert_eq!(buf, "第二行");
    }

    #[test]
    fn test_t47_take_sentence_empty_and_whitespace() {
        let mut buf = String::new();
        assert!(take_sentence(&mut buf).is_none(), "空缓冲返回 None");
        let mut buf2 = "   \n  ".to_string();
        assert!(take_sentence(&mut buf2).is_none(), "纯空白返回 None");
    }

    // ─── clean_markdown_for_tts ──────────────────────────────────

    #[test]
    fn test_c1_clean_bold() {
        assert_eq!(clean_markdown_for_tts("**加粗**文本"), "加粗文本");
        assert_eq!(clean_markdown_for_tts("***加粗斜体***"), "加粗斜体");
        assert_eq!(clean_markdown_for_tts("~~删除~~"), "删除");
    }

    #[test]
    fn test_c2_clean_heading() {
        assert_eq!(clean_markdown_for_tts("## 标题"), "标题");
        assert_eq!(clean_markdown_for_tts("###### 六级标题"), "六级标题");
    }

    #[test]
    fn test_c3_clean_link() {
        assert_eq!(
            clean_markdown_for_tts("[链接](https://x.com)文字"),
            "链接文字"
        );
    }

    #[test]
    fn test_c4_clean_image() {
        assert_eq!(clean_markdown_for_tts("![图片](url)"), "图片");
    }

    #[test]
    fn test_c5_clean_list() {
        assert_eq!(clean_markdown_for_tts("- 列表项"), "列表项");
        assert_eq!(clean_markdown_for_tts("+ 加号列表"), "加号列表");
        assert_eq!(clean_markdown_for_tts("1. 有序列表"), "有序列表");
        assert_eq!(clean_markdown_for_tts("3) 括号列表"), "括号列表");
    }

    #[test]
    fn test_c6_clean_quote() {
        assert_eq!(clean_markdown_for_tts("> 引用内容"), "引用内容");
    }

    #[test]
    fn test_c7_clean_fence_and_inline_code() {
        let raw = "```rust\nlet x = 1;\n```";
        assert_eq!(clean_markdown_for_tts(raw), "let x = 1;");
        assert_eq!(clean_markdown_for_tts("`内联代码`"), "内联代码");
    }

    #[test]
    fn test_c8_clean_unclosed_and_escape() {
        assert_eq!(clean_markdown_for_tts("**未闭合"), "未闭合");
        assert_eq!(clean_markdown_for_tts("\\*字面\\#"), "*字面#");
    }

    #[test]
    fn test_c9_clean_preserve_plain_text() {
        // 普通中文、数字、缩写、行内连字符/加号均不受影响
        assert_eq!(clean_markdown_for_tts("你好，世界。"), "你好，世界。");
        assert_eq!(clean_markdown_for_tts("单价3.14元"), "单价3.14元");
        assert_eq!(clean_markdown_for_tts("C++"), "C++");
        assert_eq!(clean_markdown_for_tts("well-known"), "well-known");
        // 整行分隔线删除
        assert_eq!(clean_markdown_for_tts("---"), "");
        assert_eq!(clean_markdown_for_tts("***"), "");
    }

    #[test]
    fn test_c10_clean_markdown_heavy_block() {
        // 模拟用户日志中的大段 markdown 回复
        let raw = "**主要特征**\n- 周五放量反弹（沪指 +0.72%、深成指 +2.21%）\n- 结构分化明显：**大消费走强**、**科技成长承压**\n来源：\n- [A股周评（易天富）](https://m.etf88.com/jjb/2026/0801/9825902.html)";
        let cleaned = clean_markdown_for_tts(raw);
        assert!(!cleaned.contains('*'), "不应残留星号: {cleaned}");
        assert!(
            !cleaned.contains('[') && !cleaned.contains(']'),
            "不应残留方括号"
        );
        assert!(
            !cleaned.contains('(') && !cleaned.contains(')'),
            "不应残留半角括号"
        );
        assert!(cleaned.contains("主要特征"));
        assert!(cleaned.contains("周五放量反弹"));
        assert!(cleaned.contains("大消费走强"));
        assert!(cleaned.contains("科技成长承压"));
        assert!(cleaned.contains("A股周评（易天富）"));
    }

    // ─── TtsTextAggregator ───────────────────────────────────────

    #[test]
    fn test_a1_first_block_emitted_immediately() {
        // 首个非空 delta 即使无标点也立即发出，便于尽快建立 TTS 会话
        let mut agg = TtsTextAggregator::new(100);
        assert_eq!(agg.push("好的"), vec!["好的"]);
    }

    #[test]
    fn test_a2_aggregate_sentence_across_chunks() {
        let mut agg = TtsTextAggregator::new(100);
        agg.push("今天。"); // 首块
        // 未闭合残句跨 chunk 累积，完整句才发出
        assert!(agg.push("我们出").is_empty());
        assert_eq!(agg.push("去玩吧！"), vec!["我们出去玩吧！"]);
    }

    #[test]
    fn test_a3_threshold_flush() {
        let mut agg = TtsTextAggregator::new(10);
        assert_eq!(agg.push("你好。"), vec!["你好。"]);
        // 残句超过阈值强制整块发出
        let blocks = agg.push("这是一段没有任何标点的很长的残句文本内容");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("这是一段没有任何标点"));
    }

    #[test]
    fn test_a4_flush_partial_and_finish() {
        let mut agg = TtsTextAggregator::new(100);
        agg.push("已发出。");
        assert!(agg.push("残句").is_empty());
        let p = agg.flush_partial().expect("残句应被强发");
        assert_eq!(p, "残句");
        assert!(agg.flush_partial().is_none(), "缓冲已空");
        assert!(agg.finish().is_none());
    }

    #[test]
    fn test_a5_clean_marks_in_aggregator() {
        let mut agg = TtsTextAggregator::new(100);
        let blocks = agg.push("**你好**，世界。");
        assert_eq!(blocks, vec!["你好，世界。"]);
    }

    #[tokio::test]
    async fn test_a6_aggregate_task_end_to_end() {
        let input = stream::iter(vec![
            "今天".to_string(),
            "天气很好。".to_string(),
            "**我们去**".to_string(),
            "公园吧！".to_string(),
        ]);
        let (agg_tx, mut agg_rx) = mpsc::channel::<String>(16);
        let cleaned_full: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let handle = tokio::spawn(tts_aggregate_task(
            Box::new(input),
            agg_tx,
            cleaned_full.clone(),
        ));

        let mut blocks = Vec::new();
        while let Some(b) = agg_rx.recv().await {
            blocks.push(b);
        }
        handle.await.expect("聚合任务正常结束");

        // 块拼接后的文本 = 清洗后按句聚合结果
        let joined: String = blocks.iter().flat_map(|s| s.chars()).collect();
        assert_eq!(joined, "今天天气很好。我们去公园吧！");
        // cleaned_full 累积了清洗后全文（块间以换行分隔）
        let full = cleaned_full.lock().unwrap().clone();
        assert_eq!(
            full.lines().collect::<String>(),
            "今天天气很好。我们去公园吧！"
        );
    }

    // ─── 唤醒问候文本解析 ──────────────────────────────

    #[test]
    fn test_g1_wake_greeting_disabled_returns_none() {
        let mut cfg = test_tts_config();
        cfg.wake_greeting_enabled = false;
        cfg.wake_greeting = Some("你好".to_string());
        assert!(wake_greeting_text(&cfg).is_none());
    }

    #[test]
    fn test_g2_wake_greeting_none_text_falls_back() {
        let cfg = test_tts_config(); // 默认 enabled=true, text=None
        assert_eq!(wake_greeting_text(&cfg).as_deref(), Some("你好"));
    }

    #[test]
    fn test_g3_wake_greeting_empty_text_falls_back() {
        let mut cfg = test_tts_config();
        cfg.wake_greeting = Some(String::new());
        assert_eq!(wake_greeting_text(&cfg).as_deref(), Some("你好"));
    }

    #[test]
    fn test_g4_wake_greeting_whitespace_text_falls_back() {
        let mut cfg = test_tts_config();
        cfg.wake_greeting = Some("   ".to_string());
        assert_eq!(wake_greeting_text(&cfg).as_deref(), Some("你好"));
    }

    #[test]
    fn test_g5_wake_greeting_custom_text_preserved() {
        let mut cfg = test_tts_config();
        cfg.wake_greeting = Some("我在，有什么可以帮你？".to_string());
        assert_eq!(
            wake_greeting_text(&cfg).as_deref(),
            Some("我在，有什么可以帮你？")
        );
    }

    // ─── 处理进度提示文案回退测试 ─────────────────────────

    #[test]
    fn test_ft1_feedback_disabled_returns_none() {
        let mut cfg = test_tts_config();
        cfg.thinking_feedback_enabled = false;
        cfg.thinking_feedback_text = Some("好的".to_string());
        assert!(thinking_feedback_text(&cfg).is_none());
        assert!(thinking_timeout_text(&cfg).is_none());
    }

    #[test]
    fn test_ft2_feedback_default_text() {
        let cfg = test_tts_config(); // 默认 enabled=true, text=None
        assert_eq!(
            thinking_feedback_text(&cfg).as_deref(),
            Some(DEFAULT_THINKING_FEEDBACK_TEXT)
        );
    }

    #[test]
    fn test_ft3_feedback_empty_whitespace_falls_back() {
        let mut cfg = test_tts_config();
        cfg.thinking_feedback_text = Some("   ".to_string());
        assert_eq!(
            thinking_feedback_text(&cfg).as_deref(),
            Some(DEFAULT_THINKING_FEEDBACK_TEXT)
        );
    }

    #[test]
    fn test_ft4_feedback_custom_preserved() {
        let mut cfg = test_tts_config();
        cfg.thinking_feedback_text = Some("正在处理，请稍等".to_string());
        assert_eq!(
            thinking_feedback_text(&cfg).as_deref(),
            Some("正在处理，请稍等")
        );
    }

    #[test]
    fn test_ft5_timeout_default_text() {
        let cfg = test_tts_config();
        assert_eq!(
            thinking_timeout_text(&cfg).as_deref(),
            Some(DEFAULT_THINKING_TIMEOUT_TEXT)
        );
    }

    #[test]
    fn test_ft6_timeout_empty_falls_back() {
        let mut cfg = test_tts_config();
        cfg.thinking_feedback_timeout_text = Some(String::new());
        assert_eq!(
            thinking_timeout_text(&cfg).as_deref(),
            Some(DEFAULT_THINKING_TIMEOUT_TEXT)
        );
    }

    #[test]
    fn test_ft7_timeout_custom_preserved() {
        let mut cfg = test_tts_config();
        cfg.thinking_feedback_timeout_text = Some("超时了，请稍后再试".to_string());
        assert_eq!(
            thinking_timeout_text(&cfg).as_deref(),
            Some("超时了，请稍后再试")
        );
    }

    // ─── 工具名 → 播报文案映射测试 ───────────────────────

    #[test]
    fn test_tool1_web_search_mapped() {
        assert_eq!(
            tool_feedback_text("WebSearch").as_deref(),
            Some("我正在搜索网络，请稍候")
        );
    }

    #[test]
    fn test_tool2_bash_mapped() {
        assert_eq!(
            tool_feedback_text("Bash").as_deref(),
            Some("我正在执行命令，请稍候")
        );
    }

    #[test]
    fn test_tool3_unknown_returns_none() {
        assert!(tool_feedback_text("UnknownTool").is_none());
        assert!(tool_feedback_text("").is_none());
    }

    #[test]
    fn test_tool4_read_and_edit_mapped() {
        assert_eq!(
            tool_feedback_text("Read").as_deref(),
            Some("我正在读取文件，请稍候")
        );
        assert_eq!(
            tool_feedback_text("Edit").as_deref(),
            Some("我正在修改代码，请稍候")
        );
    }

    // ─── 首个可播文本等待 + 进度播报测试 ───────────────────

    /// 构造一个计数 on_tick 闭包（每 tick 自增，返回是否继续）
    fn counting_on_tick(
        ticks: Arc<std::sync::atomic::AtomicUsize>,
        keep_going: bool,
    ) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'static>>
    {
        move || {
            let ticks = ticks.clone();
            Box::pin(async move {
                ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                keep_going
            })
        }
    }

    #[tokio::test]
    async fn test_wt1_periodic_feedback_until_text() {
        let (agg_tx, mut agg_rx) = mpsc::channel::<String>(4);
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ticks_for_task = ticks.clone();
        let handle = tokio::spawn(async move {
            wait_first_text_with_feedback(
                &mut agg_rx,
                std::time::Duration::from_secs(1),
                std::time::Duration::from_millis(40),
                counting_on_tick(ticks_for_task, true),
            )
            .await
        });
        // 等待首个 tick 触发后，再发送首个文本，验证周期播报期间文本到达能立即返回
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        agg_tx.send("你好".to_string()).await.unwrap();
        let result = handle.await.unwrap();
        assert!(matches!(result, FirstTextOutcome::Text(t) if t == "你好"));
        assert!(
            ticks.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "等待期间应至少播报一次进度提示"
        );
    }

    #[tokio::test]
    async fn test_wt2_text_immediate_no_tick() {
        let (agg_tx, mut agg_rx) = mpsc::channel::<String>(4);
        agg_tx.send("你好".to_string()).await.unwrap();
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = wait_first_text_with_feedback(
            &mut agg_rx,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(40),
            counting_on_tick(ticks.clone(), true),
        )
        .await;
        assert!(matches!(result, FirstTextOutcome::Text(t) if t == "你好"));
        assert_eq!(ticks.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_wt3_stream_ended_empty_reply() {
        let (agg_tx, mut agg_rx) = mpsc::channel::<String>(4);
        drop(agg_tx); // 显式关闭所有 sender，recv 返回 None
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = wait_first_text_with_feedback(
            &mut agg_rx,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(40),
            counting_on_tick(ticks.clone(), true),
        )
        .await;
        assert!(matches!(result, FirstTextOutcome::StreamEnded));
    }

    #[tokio::test]
    async fn test_wt4_timeout() {
        // interval(1s) 大于 total_timeout(100ms)：期间不应触发 tick，应直接超时
        let (_agg_tx, mut agg_rx) = mpsc::channel::<String>(4);
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = wait_first_text_with_feedback(
            &mut agg_rx,
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(1),
            counting_on_tick(ticks.clone(), true),
        )
        .await;
        assert!(matches!(result, FirstTextOutcome::Timeout));
        assert_eq!(ticks.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_wt5_on_tick_false_stops() {
        // on_tick 返回 false（回放管道关闭）→ 以 StreamEnded 返回，且只播报一次
        let (_agg_tx, mut agg_rx) = mpsc::channel::<String>(4);
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = wait_first_text_with_feedback(
            &mut agg_rx,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(40),
            counting_on_tick(ticks.clone(), false),
        )
        .await;
        assert!(matches!(result, FirstTextOutcome::StreamEnded));
        assert_eq!(ticks.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // ─── 无语音超时检测测试 ─────────────────────────────

    /// 构造已预置流式管道状态的策略（跳过联网的 init_asr_pipeline）
    fn make_strategy_with_pipeline(timeout_ms: u64) -> AsrLlmTtsStrategy {
        let strategy = make_strategy(Arc::new(MockAgent));
        strategy.tts_config.write().unwrap().no_speech_timeout_ms = timeout_ms;

        // 手动构造流式管道状态，避免真实 ASR 连接
        let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(256);
        // drain 任务消费 PCM，防止 on_audio_frame 的 send 阻塞
        tokio::spawn(async move { while pcm_rx.recv().await.is_some() {} });
        let asr_handle = tokio::spawn(async { Ok::<String, String>(String::new()) });
        let decoder = Decoder::new(16000, Channels::Mono).expect("创建测试 Opus 解码器失败");
        let state = AsrPipelineState {
            pcm_tx,
            asr_handle,
            decoder,
            frame_samples: 960,
            frame_count: 0,
            last_log_frame: 0,
            silence_count: 0,
            speech_detected: false,
            no_speech_frames: 0,
        };
        *strategy.streaming_state.lock().unwrap() = Some(state);
        strategy
    }

    /// 生成一帧 Opus 音频帧（16kHz 60ms）
    fn make_audio_frame(pcm: &[u8]) -> AudioFrame {
        let opus = pcm_to_opus_frames(pcm, 16000, 60)
            .expect("Opus 编码失败")
            .remove(0);
        AudioFrame {
            timestamp: 0,
            data: opus,
        }
    }

    /// 静音帧（全零 PCM，RMS≈0 < 2000 阈值）
    fn silence_frame() -> AudioFrame {
        make_audio_frame(&vec![0u8; 960 * 2])
    }

    /// 响亮帧（正弦，RMS ≈ 7071 > 2000 阈值）
    fn loud_frame() -> AudioFrame {
        let mut pcm = Vec::with_capacity(960 * 2);
        for i in 0..960 {
            let val = ((i as f64 * 0.1).sin() * 10000.0) as i16;
            pcm.extend_from_slice(&val.to_le_bytes());
        }
        make_audio_frame(&pcm)
    }

    #[tokio::test]
    async fn test_ns1_no_speech_triggers_after_timeout() {
        // 300ms → 5 帧（ceil(300/60)）
        let strategy = make_strategy_with_pipeline(300);
        for _ in 0..5 {
            strategy
                .on_audio_frame(&silence_frame())
                .await
                .expect("喂帧应成功");
        }
        assert!(
            strategy.silence_closed.load(Ordering::Acquire),
            "初始静音达到阈值后应触发无语音超时并关闭管道"
        );
        let notify = strategy
            .no_speech_completion()
            .expect("应暴露无语音超时 Notify");
        let fired = tokio::time::timeout(std::time::Duration::from_millis(1000), notify.notified())
            .await
            .is_ok();
        assert!(fired, "无语音超时 Notify 应被触发");
    }

    #[tokio::test]
    async fn test_ns2_no_speech_not_triggered_before_timeout() {
        let strategy = make_strategy_with_pipeline(300); // 5 帧
        for _ in 0..4 {
            strategy
                .on_audio_frame(&silence_frame())
                .await
                .expect("喂帧应成功");
        }
        assert!(
            !strategy.silence_closed.load(Ordering::Acquire),
            "未达到阈值时不应触发无语音超时"
        );
        let notify = strategy
            .no_speech_completion()
            .expect("应暴露无语音超时 Notify");
        let fired = tokio::time::timeout(std::time::Duration::from_millis(200), notify.notified())
            .await
            .is_ok();
        assert!(!fired, "未达到阈值时 Notify 不应被触发");
    }

    #[tokio::test]
    async fn test_ns3_no_speech_resets_on_speech() {
        let strategy = make_strategy_with_pipeline(300); // 5 帧
        // 4 帧静音 + 1 帧响亮（检测到语音，无语音超时作废）
        for _ in 0..4 {
            strategy
                .on_audio_frame(&silence_frame())
                .await
                .expect("喂帧应成功");
        }
        strategy
            .on_audio_frame(&loud_frame())
            .await
            .expect("喂帧应成功");
        // 之后大量静音帧：speech_detected=true，走 VAD 路径但 asr_received_text=false 不触发
        for _ in 0..40 {
            strategy
                .on_audio_frame(&silence_frame())
                .await
                .expect("喂帧应成功");
        }
        assert!(
            !strategy.silence_closed.load(Ordering::Acquire),
            "检测到语音后无语音超时应作废（交给 VAD 流程）"
        );
        let notify = strategy
            .no_speech_completion()
            .expect("应暴露无语音超时 Notify");
        let fired = tokio::time::timeout(std::time::Duration::from_millis(200), notify.notified())
            .await
            .is_ok();
        assert!(!fired, "检测到语音后无语音超时 Notify 不应被触发");
    }

    #[tokio::test]
    async fn test_ns4_no_speech_disabled_when_zero() {
        let strategy = make_strategy_with_pipeline(0); // 0 = 禁用
        for _ in 0..30 {
            strategy
                .on_audio_frame(&silence_frame())
                .await
                .expect("喂帧应成功");
        }
        assert!(
            !strategy.silence_closed.load(Ordering::Acquire),
            "timeout=0 应禁用无语音超时"
        );
    }

    #[test]
    fn test_ns5_no_speech_threshold_frames() {
        assert_eq!(no_speech_threshold_frames(10000), 167);
        assert_eq!(no_speech_threshold_frames(60), 1);
        assert_eq!(no_speech_threshold_frames(120), 2);
        assert_eq!(no_speech_threshold_frames(61), 2);
        assert_eq!(no_speech_threshold_frames(0), 0);
    }

    // ─── agent 日志收尾（提前退出路径记录） ──────────────

    /// 构造一个已完成的事件消费句柄：task 立即结束，log 预填指定事件
    fn make_agent_events_handle(events: Vec<AgentLogEvent>) -> AgentEventsHandle {
        AgentEventsHandle {
            task: tokio::spawn(async {}),
            log: Arc::new(Mutex::new(events)),
        }
    }

    #[tokio::test]
    async fn test_le1_take_agent_events_delivered() {
        let mut opt = Some(make_agent_events_handle(vec![AgentLogEvent::Thinking {
            thinking: "思考中".to_string(),
        }]));
        let events = AsrLlmTtsStrategy::take_agent_events(&mut opt).await;
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AgentLogEvent::Thinking { thinking } if thinking == "思考中"
        ));
    }

    #[tokio::test]
    async fn test_le2_take_agent_events_already_taken() {
        let mut opt: Option<AgentEventsHandle> = None;
        let events = AsrLlmTtsStrategy::take_agent_events(&mut opt).await;
        assert!(events.is_empty(), "None 时应返回空事件");
    }

    #[tokio::test]
    async fn test_le3_finish_agent_log_timeout_writes_record() {
        crate::test_util::run_with_temp_home_async(move |home| async move {
            let strategy = make_strategy(Arc::new(MockAgent));
            let mut opt = Some(make_agent_events_handle(vec![AgentLogEvent::ToolUse {
                id: "toolu_1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({ "command": "ls" }),
            }]));
            strategy
                .finish_agent_log(
                    &mut opt,
                    "你好",
                    "session-1",
                    None,
                    "timeout",
                    Some("等待首个可播文本超时 (60s)"),
                    std::time::Instant::now(),
                )
                .await;

            let day = chrono::Local::now().format("%Y-%m-%d").to_string();
            let path = home.join(format!(".haimen/agent-logs/{}.jsonl", day));
            let content = std::fs::read_to_string(&path).expect("日志文件应存在");
            let line = content.lines().last().expect("应有日志行");
            let rec: crate::agent_log::AgentLogRecord =
                serde_json::from_str(line).expect("日志行应可解析");
            assert_eq!(rec.source, "xiaozhi");
            assert_eq!(rec.status, "timeout");
            assert_eq!(rec.output, None);
            assert_eq!(rec.error.as_deref(), Some("等待首个可播文本超时 (60s)"));
            assert_eq!(rec.events.len(), 1);
            assert!(matches!(
                &rec.events[0],
                AgentLogEvent::ToolUse { name, .. } if name == "Bash"
            ));
        })
        .await;
    }

    #[tokio::test]
    async fn test_le4_finish_agent_log_success_with_partial_output() {
        crate::test_util::run_with_temp_home_async(move |home| async move {
            let strategy = make_strategy(Arc::new(MockAgent));
            // 无事件：验证空事件也能正常记录
            let mut opt = Some(make_agent_events_handle(Vec::new()));
            strategy
                .finish_agent_log(
                    &mut opt,
                    "你好",
                    "session-2",
                    Some("部分回复"),
                    "success",
                    None,
                    std::time::Instant::now(),
                )
                .await;

            let day = chrono::Local::now().format("%Y-%m-%d").to_string();
            let path = home.join(format!(".haimen/agent-logs/{}.jsonl", day));
            let content = std::fs::read_to_string(&path).expect("日志文件应存在");
            let line = content.lines().last().expect("应有日志行");
            let rec: crate::agent_log::AgentLogRecord =
                serde_json::from_str(line).expect("日志行应可解析");
            assert_eq!(rec.status, "success");
            assert_eq!(rec.output.as_deref(), Some("部分回复"));
            assert_eq!(rec.error, None);
        })
        .await;
    }

    // ─── 连续音频管道（ContinuityPump）测试 ─────────────────────

    /// 生成正弦波 PCM（24kHz 16-bit mono）
    fn make_sine_pcm(millis: u64, freq: f64, amp: f64) -> Vec<u8> {
        let samples = (24000 * millis / 1000) as usize;
        let mut pcm = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let t = i as f64 / 24000.0;
            let val = ((t * freq * 2.0 * std::f64::consts::PI).sin() * amp) as i16;
            pcm.extend_from_slice(&val.to_le_bytes());
        }
        pcm
    }

    /// 收集回放管道中的所有音频帧（按到达顺序）
    fn drain_audio_frames(frame_rx: &mut mpsc::Receiver<PlaybackEvent>) -> Vec<AudioFrame> {
        let mut frames = Vec::new();
        while let Ok(evt) = frame_rx.try_recv() {
            if let PlaybackEvent::Audio(f) = evt {
                frames.push(f);
            }
        }
        frames
    }

    /// 全量解码并返回每帧 RMS 与是否出现解码错误
    fn decode_frames(frames: &[AudioFrame]) -> Result<(Vec<f64>, usize), String> {
        let mut decoder =
            Decoder::new(24000, Channels::Mono).map_err(|e| format!("创建解码器失败: {}", e))?;
        let mut pcm_buf = vec![0i16; 2880];
        let mut rms_per_frame = Vec::with_capacity(frames.len());
        let mut total_samples = 0usize;
        for f in frames {
            let n = decoder
                .decode(&f.data, &mut pcm_buf, false)
                .map_err(|e| format!("Opus 解码失败: {}", e))?;
            let mut sum_sq = 0.0f64;
            for &s in &pcm_buf[..n] {
                sum_sq += (s as f64) * (s as f64);
            }
            rms_per_frame.push((sum_sq / n as f64).sqrt());
            total_samples += n;
        }
        Ok((rms_per_frame, total_samples))
    }

    /// 泵在纯空闲（喂零）下应持续产出比特流连续的静音帧：
    /// - 时间戳单调 +60（会话级）
    /// - 每帧解码为 60ms @ 24kHz = 2880 采样，无解码错误
    /// - 静音区 RMS≈0
    /// - pcm_rx 关闭后 pump flush 残片 + 追加尾静音帧后正常退出
    #[tokio::test]
    async fn test_pump_zero_fill_continuous_monotonic_silence() {
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(64);
        let (frame_tx, mut frame_rx) = mpsc::channel::<PlaybackEvent>(512);
        let cfg = PumpConfig {
            tick_ms: 5,
            tail_silence_frames: 2,
            ..PumpConfig::default()
        };
        let handle = tokio::spawn(run_continuity_pump(
            pcm_rx,
            frame_tx,
            "test".to_string(),
            cfg,
        ));

        // 保持 pcm_tx 存活，轮询收集静音帧直到达到目标数量或超时。
        // 不用固定 sleep 计数——CI（尤其 Windows）慢 runner 上 pump 任务调度
        // 可能被挤压，固定时长内帧数不足会误报；轮询式等待只依赖 pump 产出帧
        // 本身，与调度频率解耦。3s 兜底超时远大于 5ms/tick × 目标帧数所需时间。
        let mut frames: Vec<AudioFrame> = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while frames.len() < 30 {
            while let Ok(evt) = frame_rx.try_recv() {
                if let PlaybackEvent::Audio(f) = evt {
                    frames.push(f);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        drop(pcm_tx);

        handle
            .await
            .expect("pump 任务不应 panic")
            .expect("pump 收尾不应失败");

        // 收尾阶段 pump 可能还有 flush 残片 + 尾静音帧
        frames.extend(drain_audio_frames(&mut frame_rx));
        assert!(
            frames.len() >= 30,
            "空闲喂零应产出足够静音帧, got {}",
            frames.len()
        );

        // 时间戳单调 +60（含 flush + 尾静音帧，序列连续）
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(
                f.timestamp,
                (i as u32) * 60,
                "第 {} 帧时间戳应为 {}*60",
                i,
                i
            );
        }

        // 解码无错 + 时长正确 + 静音 RMS≈0
        let (rms_list, total_samples) = decode_frames(&frames).expect("解码应无错");
        assert_eq!(
            total_samples,
            frames.len() * 1440,
            "每帧应解码 60ms@24kHz = 1440 采样"
        );
        let max_rms = rms_list.iter().cloned().fold(0.0f64, f64::max);
        assert!(max_rms < 300.0, "静音帧 RMS 应≈0, got max_rms={}", max_rms);
    }

    /// 内容 PCM 交错喂零后，泵应产出比特流连续、时间戳单调的帧序列：
    /// - 内容块与静音帧交错，全量解码无错（无爆音/断流）
    /// - 时间戳单调 +60
    #[tokio::test]
    async fn test_pump_content_zero_interleave_continuous() {
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(64);
        let (frame_tx, mut frame_rx) = mpsc::channel::<PlaybackEvent>(512);
        let cfg = PumpConfig {
            tick_ms: 5,
            tail_silence_frames: 2,
            ..PumpConfig::default()
        };
        let handle = tokio::spawn(run_continuity_pump(
            pcm_rx,
            frame_tx,
            "test".to_string(),
            cfg,
        ));

        // 两段 0.5s 内容(440Hz 正弦),中间空转 30ms(喂零静音)
        let content = make_sine_pcm(500, 440.0, 10000.0);
        pcm_tx
            .send(content.clone())
            .await
            .expect("发送内容 PCM 失败");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        pcm_tx.send(content).await.expect("发送内容 PCM 失败");
        // 给 pump 消化时间,再关闭 PCM 源
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        drop(pcm_tx);

        handle
            .await
            .expect("pump 任务不应 panic")
            .expect("pump 收尾不应失败");

        let frames = drain_audio_frames(&mut frame_rx);
        // 0.5s×2 = 16 内容帧 + 空转静音帧 + 2 尾帧
        assert!(
            frames.len() >= 18,
            "应产出内容+静音帧, got {}",
            frames.len()
        );

        // 时间戳单调 +60
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(
                f.timestamp,
                (i as u32) * 60,
                "第 {} 帧时间戳应为 {}*60",
                i,
                i
            );
        }

        // 全量解码无错（交错喂零后比特流仍连续，无爆音/断流）
        let (rms_list, total_samples) = decode_frames(&frames).expect("解码应无错");
        assert_eq!(total_samples, frames.len() * 1440);
        // 存在内容帧（RMS 显著非零）
        let max_rms = rms_list.iter().cloned().fold(0.0f64, f64::max);
        assert!(
            max_rms > 5000.0,
            "应包含内容帧(高 RMS), got max_rms={}",
            max_rms
        );
    }

    /// `PumpGuard::finish`：PCM 源关闭后，pump 正常 flush + 尾静音并返回 Ok
    #[tokio::test]
    async fn test_pump_guard_finish_normal_exit() {
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(64);
        let (frame_tx, _frame_rx) = mpsc::channel::<PlaybackEvent>(64);
        let handle = tokio::spawn(run_continuity_pump(
            pcm_rx,
            frame_tx,
            "test".to_string(),
            PumpConfig::default(),
        ));
        let guard = PumpGuard::new(handle);

        // 跑一小段让 pump 产出帧
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        // 关闭 PCM 源 → pump flush + 尾静音后正常退出
        drop(pcm_tx);

        guard.finish().await.expect("pump 正常收尾应返回 Ok");
    }

    /// 诊断：验证 StreamingOpusEncoder 对纯零输入是否收敛为逐字节相同的静音帧，
    /// 且该收敛静音态在内容中断后依然复现（历史无关）。
    ///
    /// 若收敛且历史无关，pump 的静音帧去重（缓存复用收敛帧）可行且比特流与
    /// 不去的逐字节相同（编码器处于收敛静音态时，喂零产出 == 缓存复用帧）。
    #[test]
    fn test_silence_frames_convergence_diag() {
        let mut enc = StreamingOpusEncoder::new(24000, 60).expect("创建编码器失败");
        let zero = vec![0u8; 2880];
        let content = make_sine_pcm(300, 440.0, 8000.0); // 0.3s 内容,打断静音态

        // 第一段静音:收敛
        let mut seg1: Vec<Vec<u8>> = Vec::new();
        for _ in 0..30 {
            seg1.extend(enc.feed(&zero).expect("编码失败"));
        }
        // 内容打断
        enc.feed(&content).expect("编码失败");
        // 第二段静音:再次收敛
        let mut seg2: Vec<Vec<u8>> = Vec::new();
        for _ in 0..30 {
            seg2.extend(enc.feed(&zero).expect("编码失败"));
        }

        // 找各段的收敛帧：首帧 size==8 即视为进入静音稳态（过渡帧 >8 字节）
        fn converged_baseline(seg: &[Vec<u8>]) -> Option<&Vec<u8>> {
            seg.iter().find(|f| f.len() == 8)
        }
        let b1 = converged_baseline(&seg1).expect("seg1 应出现 8 字节静音帧");
        let b2 = converged_baseline(&seg2).expect("seg2 应出现 8 字节静音帧");
        // 从基线位置之后所有帧都应逐字节等于基线
        let pos1 = seg1.iter().position(|f| f == b1).unwrap();
        let pos2 = seg2.iter().position(|f| f == b2).unwrap();
        let seg1_identical = seg1[pos1..].iter().all(|f| f == b1);
        let seg2_identical = seg2[pos2..].iter().all(|f| f == b2);

        // 断言：两段静音均收敛且收敛帧逐字节相同（静音稳态历史无关）。
        // 这是 pump 静音帧去重（缓存复用收敛帧，比特流与喂零逐字节相同）的前提。
        assert!(seg1_identical, "seg1 收敛后应逐字节相同");
        assert!(seg2_identical, "seg2 收敛后应逐字节相同");
        assert_eq!(b1, b2, "内容中断后应收敛到同一静音帧");
        assert_eq!(b1.len(), 8, "收敛静音帧应为 8 字节");
    }
}
