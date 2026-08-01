use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ─── 消息类型 ─────────────────────────────────────────────

/// 客户端→服务器的 JSON 文本消息
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        #[serde(default)]
        version: u32,
        #[serde(default = "default_transport")]
        transport: String,
        audio_params: AudioParams,
        #[serde(default)]
        features: Features,
    },
    Listen {
        state: ListenState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Abort,
}

fn default_transport() -> String {
    "websocket".to_string()
}

/// 服务器→客户端的 JSON 文本消息
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello {
        version: u32,
        transport: String,
        session_id: String,
        audio_params: AudioParams,
    },
    Tts {
        session_id: String,
        state: TtsState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Stt {
        session_id: String,
        text: String,
    },
    Error {
        code: String,
        message: String,
    },
}

// ─── 枚举 ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenState {
    Detect,
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TtsState {
    Start,
    #[serde(rename = "sentence_start")]
    SentenceStart,
    Stop,
}

// ─── 会话状态 ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionState {
    /// 等待 HELLO 握手
    AwaitingHello,
    /// 已握手，等待 listen 指令
    Ready,
    /// 录音中，接收音频帧
    Recording,
    /// 回放中，发送音频帧
    Playing,
}

// ─── 音频参数 ──────────────────────────────────────────────

/// 音频参数（双向协商）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioParams {
    #[serde(default = "default_audio_format")]
    pub format: String,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_channels")]
    pub channels: u8,
    #[serde(default = "default_frame_duration")]
    pub frame_duration: u32,
}

impl Default for AudioParams {
    fn default() -> Self {
        Self {
            format: default_audio_format(),
            sample_rate: default_sample_rate(),
            channels: default_channels(),
            frame_duration: default_frame_duration(),
        }
    }
}

fn default_audio_format() -> String {
    "opus".to_string()
}
const fn default_sample_rate() -> u32 {
    24000
}
const fn default_channels() -> u8 {
    1
}
const fn default_frame_duration() -> u32 {
    60
}

// ─── 设备功能 ──────────────────────────────────────────────

/// 设备功能特性（参考 xiaozhi-client 和 xiaozhi-esp32-server）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Features {
    #[serde(default)]
    pub aec: bool,
    #[serde(default)]
    pub mcp: bool,
    #[serde(default = "default_emoji")]
    pub emoji: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            aec: false,
            mcp: false,
            emoji: default_emoji(),
            extra: HashMap::new(),
        }
    }
}

const fn default_emoji() -> bool {
    true
}

// ─── OTA 类型 ──────────────────────────────────────────────

/// OTA 请求
#[derive(Debug, Deserialize)]
pub struct OtaRequest {
    pub version: u32,
    pub mac_address: String,
    pub uuid: String,
    #[serde(default)]
    pub chip_model_name: Option<String>,
    #[serde(default)]
    pub application: Option<ApplicationInfo>,
    #[serde(default)]
    pub board: Option<BoardInfo>,
    #[serde(default)]
    pub flash_size: Option<u64>,
    #[serde(default)]
    pub minimum_free_heap_size: Option<String>,
    #[serde(default)]
    pub ota: Option<OtaInfo>,
}

/// OTA 响应
#[derive(Debug, Serialize)]
pub struct OtaResponse {
    pub websocket: WebsocketInfo,
    pub server_time: ServerTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FirmwareInfo>,
    pub audio_params: AudioParams,
}

/// WebSocket 连接信息
#[derive(Debug, Serialize)]
pub struct WebsocketInfo {
    pub url: String,
    #[serde(default)]
    pub token: String,
    pub version: u32,
}

/// 服务器时间
#[derive(Debug, Serialize)]
pub struct ServerTime {
    pub timestamp: i64,
    pub timezone_offset: i32,
}

/// 固件信息
#[derive(Debug, Serialize)]
pub struct FirmwareInfo {
    pub version: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub force: bool,
}

/// 应用信息
#[derive(Debug, Deserialize)]
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub compile_time: Option<String>,
    #[serde(default)]
    pub board: Option<HashMap<String, String>>,
}

/// 板子信息
#[derive(Debug, Deserialize)]
pub struct BoardInfo {
    pub r#type: String,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default)]
    pub ssid: Option<String>,
    #[serde(default)]
    pub rssi: Option<i32>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub channel: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
}

/// OTA 信息
#[derive(Debug, Deserialize)]
pub struct OtaInfo {
    #[serde(default)]
    pub label: Option<String>,
}

// ─── 音频帧 ────────────────────────────────────────────────

/// 缓冲的音频帧
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub timestamp: u32,
    pub data: Vec<u8>,
}

/// 回放通道事件：策略层通过该事件流既发送音频帧，也发送文本消息。
///
/// 单个通道天然保持事件顺序，策略侧「先 Stt、后句子、穿插音频」的发送顺序
/// 会原样到达 ws.rs，无需额外的顺序协调。
#[derive(Debug, Clone)]
pub enum PlaybackEvent {
    /// 用户语音识别文本（ASR 结果）→ 设备 `stt` 消息
    Stt(String),
    /// LLM 回复文本（句级）→ 设备 `tts/sentence_start` 消息
    LlmSentence(String),
    /// TTS 音频帧 → Opus 二进制帧
    Audio(AudioFrame),
}
