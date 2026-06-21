//! WebSocket 连接管理 — 处理小智硬件设备的全双工通信
//!
//! 管理设备 WebSocket 连接的整个生命周期，包括 HELLO 握手、
//! 音频数据缓冲和回声回放。

use std::time::Duration;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use uuid::Uuid;

use crate::protocol::{AudioProtocol, ProtocolError, detect_and_parse, encode_protocol2};
use crate::types::{
    AudioFrame, AudioParams, ClientMessage, ListenState, ServerMessage, SessionState, TtsState,
};

// ─── 会话结构 ──────────────────────────────────────────────

/// 本地会话状态（每连接一个实例，不涉及全局共享）
struct Session {
    device_id: String,
    session_id: String,
    audio_params: AudioParams,
    state: SessionState,
    audio_buffer: Vec<AudioFrame>,
    cumulated_timestamp: u32,
}

// ─── 公开 API ──────────────────────────────────────────────

/// 处理 WebSocket 升级请求
///
/// 从 HTTP 头中提取 `Device-Id`，传递给连接处理函数。
pub async fn handle_ws_upgrade(ws: WebSocketUpgrade, headers: HeaderMap) -> Response {
    let device_id = headers
        .get("device-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    ws.on_upgrade(move |socket| handle_ws_connection(socket, device_id))
}

// ─── 连接主循环 ────────────────────────────────────────────

/// WebSocket 连接主循环
///
/// 流程：HELLO 握手（30 秒超时）→ 消息循环 → 连接关闭
async fn handle_ws_connection(mut socket: WebSocket, device_id: String) {
    let mut session = Session {
        device_id,
        session_id: String::new(),
        audio_params: AudioParams::default(),
        state: SessionState::AwaitingHello,
        audio_buffer: Vec::new(),
        cumulated_timestamp: 0,
    };

    // ── HELLO 握手（30 秒超时） ──
    let hello_result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(ClientMessage::Hello {
                        version,
                        transport,
                        audio_params,
                        ..
                    }) = serde_json::from_str(&text)
                    {
                        session.audio_params = audio_params;
                        session.session_id = Uuid::new_v4().to_string();

                        let hello = ServerMessage::Hello {
                            version,
                            transport,
                            session_id: session.session_id.clone(),
                            audio_params: session.audio_params.clone(),
                        };
                        if send_json(&mut socket, &hello).await.is_err() {
                            return;
                        }
                        session.state = SessionState::Ready;
                        tracing::info!(
                            device_id = %session.device_id,
                            session_id = %session.session_id,
                            "HELLO handshake completed",
                        );
                        return;
                    }
                    tracing::debug!("Ignoring non-HELLO message during handshake");
                }
                Some(Ok(Message::Close(_))) | None => return,
                _ => {}
            }
        }
    })
    .await;

    if hello_result.is_err() {
        tracing::warn!(
            device_id = %session.device_id,
            "HELLO handshake timeout after 30s",
        );
        return;
    }

    // ── 消息循环 ──
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                handle_text_message(&text, &mut socket, &mut session).await;
            }
            Ok(Message::Binary(data)) => {
                handle_binary_message(&data, &mut socket, &mut session).await;
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) => {
                // axum 内部自动回复 Pong
            }
            Ok(Message::Pong(_)) => {}
            Err(err) => {
                tracing::warn!(
                    device_id = %session.device_id,
                    error = %err,
                    "WebSocket receive error",
                );
                break;
            }
        }
    }

    tracing::info!(
        device_id = %session.device_id,
        session_id = %session.session_id,
        "WebSocket connection closed",
    );
}

// ─── 文本消息分发 ──────────────────────────────────────────

/// 分发 JSON 文本消息到对应的处理逻辑
async fn handle_text_message(text: &str, socket: &mut WebSocket, session: &mut Session) {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(cmd) => match cmd {
            ClientMessage::Hello { .. } => {
                tracing::warn!(
                    device_id = %session.device_id,
                    "Unexpected duplicate HELLO after handshake",
                );
            }
            ClientMessage::Listen { state, mode, text } => {
                handle_listen(state, mode, text, socket, session).await;
            }
            ClientMessage::Abort => {
                handle_abort(socket, session).await;
            }
        },
        Err(e) => {
            tracing::warn!(
                device_id = %session.device_id,
                error = %e,
                "Failed to parse client message",
            );
            let _ = send_json(
                socket,
                &ServerMessage::Error {
                    code: "parse_error".to_string(),
                    message: format!("JSON 解析失败: {}", e),
                },
            )
            .await;
        }
    }
}

// ─── Listen 状态机 ─────────────────────────────────────────

/// 处理 Listen 指令（Detect / Start / Stop）
async fn handle_listen(
    state: ListenState,
    _mode: Option<String>,
    _text: Option<String>,
    socket: &mut WebSocket,
    session: &mut Session,
) {
    match state {
        ListenState::Detect => {
            tracing::debug!(
                device_id = %session.device_id,
                "Listen::Detect — wake word detected, resetting",
            );
            session.audio_buffer.clear();
            session.state = SessionState::Ready;
        }
        ListenState::Start => {
            tracing::debug!(
                device_id = %session.device_id,
                "Listen::Start — recording started",
            );
            session.audio_buffer.clear();
            session.cumulated_timestamp = 0;
            session.state = SessionState::Recording;
        }
        ListenState::Stop => {
            tracing::debug!(
                device_id = %session.device_id,
                "Listen::Stop — recording stopped, starting echo playback",
            );
            echo_playback(socket, session).await;
        }
    }
}

// ─── Abort ─────────────────────────────────────────────────

/// 处理 Abort 中断指令
async fn handle_abort(socket: &mut WebSocket, session: &mut Session) {
    tracing::debug!(
        device_id = %session.device_id,
        "Abort — clearing buffer and stopping playback",
    );
    session.audio_buffer.clear();
    let _ = send_json(
        socket,
        &ServerMessage::Tts {
            session_id: session.session_id.clone(),
            state: TtsState::Stop,
            text: None,
        },
    )
    .await;
    session.state = SessionState::Ready;
}

// ─── 二进制音频消息 ────────────────────────────────────────

/// 处理二进制音频消息
///
/// 仅在 `Recording` 状态下缓冲二进制数据。
async fn handle_binary_message(data: &[u8], socket: &mut WebSocket, session: &mut Session) {
    if session.state != SessionState::Recording {
        tracing::trace!(
            device_id = %session.device_id,
            state = ?session.state,
            "Ignoring binary data — not in Recording state",
        );
        return;
    }
    buffer_audio(data, socket, session).await;
}

// ─── 音频缓冲 ──────────────────────────────────────────────

/// 缓冲音频帧
///
/// 自动检测音频协议（Protocol2 / Protocol3 / RawOpus），
/// 提取时间戳和有效载荷后存入 `audio_buffer`。
async fn buffer_audio(data: &[u8], socket: &mut WebSocket, session: &mut Session) {
    match detect_and_parse(data) {
        Ok(protocol) => {
            let (timestamp, payload) = match protocol {
                AudioProtocol::Protocol2 { timestamp, payload } => (timestamp, payload),
                AudioProtocol::Protocol3 { payload } => {
                    let ts = session.cumulated_timestamp;
                    session.cumulated_timestamp += session.audio_params.frame_duration;
                    (ts, payload)
                }
                AudioProtocol::RawOpus(data) => {
                    let ts = session.cumulated_timestamp;
                    session.cumulated_timestamp += session.audio_params.frame_duration;
                    (ts, data)
                }
            };
            session.audio_buffer.push(AudioFrame {
                timestamp,
                data: payload,
            });
        }
        Err(ProtocolError::UnknownProtocol) => {
            let _ = send_json(
                socket,
                &ServerMessage::Error {
                    code: "unknown_protocol".to_string(),
                    message: format!("无法识别的音频协议: {} 字节", data.len()),
                },
            )
            .await;
        }
        Err(e) => {
            tracing::debug!(
                error = ?e,
                data_len = data.len(),
                "Audio protocol parse error (non-fatal)",
            );
        }
    }
}

// ─── 回声回放 ──────────────────────────────────────────────

/// 回声回放：将缓冲的音频帧发送回设备
///
/// ## 流程
/// 1. 发送 `TTS::Start` 通知设备开始播放
/// 2. 前 5 帧免延迟发送（预缓冲优化）
/// 3. 后续帧间隔 60ms，通过 `tokio::select!` 监听中断
/// 4. 发送 `TTS::Stop`，清除缓冲区，状态恢复为 `Ready`
///
/// ## 中断处理
/// - `Listen::Start` 或 `Abort`：立即停止播放，发送 `TTS::Stop`
/// - 连接关闭：立即退出
async fn echo_playback(socket: &mut WebSocket, session: &mut Session) {
    if session.audio_buffer.is_empty() {
        tracing::debug!("Echo playback skipped: empty audio buffer");
        return;
    }

    session.state = SessionState::Playing;

    // 发送 TTS::Start
    if send_json(
        socket,
        &ServerMessage::Tts {
            session_id: session.session_id.clone(),
            state: TtsState::Start,
            text: None,
        },
    )
    .await
    .is_err()
    {
        tracing::warn!("Failed to send TTS::Start, aborting playback");
        session.audio_buffer.clear();
        session.state = SessionState::Ready;
        return;
    }

    let frames: Vec<AudioFrame> = session.audio_buffer.drain(..).collect();
    let total = frames.len();

    for (i, frame) in frames.iter().enumerate() {
        let encoded = encode_protocol2(&frame.data, frame.timestamp);

        if socket.send(Message::Binary(encoded.into())).await.is_err() {
            tracing::warn!("Failed to send audio frame, connection lost");
            session.state = SessionState::Ready;
            return;
        }

        // 前 5 帧免延迟（预缓冲优化）
        // 最后一帧免延迟（TTS.Stop 紧跟其后）
        if i < 5 || i + 1 >= total {
            continue;
        }

        // 60ms 帧间隔 + 中断监听
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(60)) => {
                // 正常等待，继续下一帧
            }
            msg = socket.recv() => {
                let interrupted = match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<ClientMessage>(&text) {
                            matches!(cmd, ClientMessage::Listen { state: ListenState::Start, .. } | ClientMessage::Abort)
                        } else {
                            false
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        tracing::debug!("Connection closed during playback");
                        session.state = SessionState::Ready;
                        return;
                    }
                    _ => false,
                };

                if interrupted {
                    tracing::debug!("Playback interrupted by client command");
                    let _ = send_json(
                        socket,
                        &ServerMessage::Tts {
                            session_id: session.session_id.clone(),
                            state: TtsState::Stop,
                            text: None,
                        },
                    )
                    .await;
                    session.state = SessionState::Ready;
                    return;
                }
            }
        }
    }

    // 所有帧发送完毕，发送 TTS::Stop
    let _ = send_json(
        socket,
        &ServerMessage::Tts {
            session_id: session.session_id.clone(),
            state: TtsState::Stop,
            text: None,
        },
    )
    .await;

    session.state = SessionState::Ready;
    tracing::debug!(
        device_id = %session.device_id,
        frame_count = total,
        "Echo playback completed",
    );
}

// ─── 工具函数 ──────────────────────────────────────────────

/// 通过 WebSocket 发送 JSON 格式的 ServerMessage
async fn send_json(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|e| {
        tracing::error!(error = %e, "Failed to serialize ServerMessage");
    })?;
    socket.send(Message::Text(text.into())).await.map_err(|e| {
        tracing::warn!(error = %e, "Failed to send WebSocket message");
    })?;
    Ok(())
}
