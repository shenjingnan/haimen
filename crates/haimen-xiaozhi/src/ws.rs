//! WebSocket 连接管理 — 处理小智硬件设备的全双工通信
//!
//! 管理设备 WebSocket 连接的整个生命周期，包括 HELLO 握手、
//! 音频数据缓冲和回声回放（通过可替换的 [`ResponseStrategy`] 策略）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use uuid::Uuid;

use crate::protocol::{AudioProtocol, ProtocolError, detect_and_parse, encode_protocol2};
use crate::strategy::ResponseStrategy;
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
    /// 录音截止时刻（5 秒超时用），None 表示未在录音
    recording_deadline: Option<Instant>,
    /// 响应策略：决定录音结束后如何生成回放音频
    strategy: Arc<dyn ResponseStrategy>,
}

// ─── 公开 API ──────────────────────────────────────────────

/// 处理 WebSocket 升级请求
///
/// 从 HTTP 头中提取 `Device-Id`，连同响应策略一起传递给连接处理函数。
pub async fn handle_ws_upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    strategy: Arc<dyn ResponseStrategy>,
) -> Response {
    let device_id = headers
        .get("device-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    ws.on_upgrade(move |socket| handle_ws_connection(socket, device_id, strategy))
}

// ─── 连接主循环 ────────────────────────────────────────────

/// WebSocket 连接主循环
///
/// 流程：HELLO 握手（30 秒超时）→ 消息循环 → 连接关闭
async fn handle_ws_connection(
    mut socket: WebSocket,
    device_id: String,
    strategy: Arc<dyn ResponseStrategy>,
) {
    let mut session = Session {
        device_id,
        session_id: String::new(),
        audio_params: AudioParams::default(),
        state: SessionState::AwaitingHello,
        audio_buffer: Vec::new(),
        cumulated_timestamp: 0,
        recording_deadline: None,
        strategy,
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

                        let server_params =
                            session.strategy.hello_audio_params(&session.audio_params);
                        let hello = ServerMessage::Hello {
                            version,
                            transport,
                            session_id: session.session_id.clone(),
                            audio_params: server_params,
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
    loop {
        // 录音状态下，用 select! 竞争消息和 5 秒超时
        let msg = if session.state == SessionState::Recording {
            // 使用 Recording 启动时保存的截止时刻，确保 5 秒固定时长
            let deadline = session
                .recording_deadline
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(5));
            let tokio_deadline = tokio::time::Instant::from_std(deadline);

            tokio::select! {
                msg = socket.recv() => msg,
                _ = tokio::time::sleep_until(tokio_deadline) => {
                    tracing::info!(
                        device_id = %session.device_id,
                        strategy = session.strategy.name(),
                        "Recording 5s timeout, triggering strategy playback",
                    );
                    session.recording_deadline = None;
                    strategy_playback(&mut socket, &mut session).await;
                    continue;
                }
            }
        } else {
            match socket.recv().await {
                Some(msg) => Some(msg),
                None => {
                    tracing::info!(
                        device_id = %session.device_id,
                        "WebSocket connection closed",
                    );
                    return;
                }
            }
        };

        match msg {
            Some(Ok(Message::Text(text))) => {
                handle_text_message(&text, &mut socket, &mut session).await;
            }
            Some(Ok(Message::Binary(data))) => {
                handle_binary_message(&data, &mut socket, &mut session).await;
            }
            Some(Ok(Message::Close(_))) => break,
            Some(Ok(Message::Ping(_))) => {
                // axum 内部自动回复 Pong
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Err(err)) => {
                tracing::warn!(
                    device_id = %session.device_id,
                    error = %err,
                    "WebSocket receive error",
                );
                break;
            }
            None => break,
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
                "Listen::Start — recording started (5s timeout)",
            );
            session.audio_buffer.clear();
            session.cumulated_timestamp = 0;
            session.recording_deadline = Some(Instant::now() + Duration::from_secs(5));
            session.state = SessionState::Recording;

            // 如果策略支持流式 ASR，通知其录音开始
            if session.strategy.supports_streaming_asr() {
                if let Err(e) = session
                    .strategy
                    .on_recording_start(&session.session_id)
                    .await
                {
                    tracing::warn!(
                        device_id = %session.device_id,
                        session_id = %session.session_id,
                        error = %e,
                        "Streaming ASR on_recording_start 失败，将继续使用批处理模式",
                    );
                }
            }
        }
        ListenState::Stop => {
            tracing::debug!(
                device_id = %session.device_id,
                "Listen::Stop — recording stopped, starting strategy playback",
            );
            session.recording_deadline = None;
            strategy_playback(socket, session).await;
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
    session.recording_deadline = None;
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
            let frame = AudioFrame {
                timestamp,
                data: payload,
            };

            // 如果策略支持流式 ASR，将帧实时喂入 ASR 管道
            if session.strategy.supports_streaming_asr() {
                if let Err(e) = session.strategy.on_audio_frame(&frame).await {
                    tracing::warn!(
                        device_id = %session.device_id,
                        error = %e,
                        "Streaming ASR on_audio_frame 失败，将继续缓冲音频",
                    );
                }
            }

            session.audio_buffer.push(frame);
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

// ─── 通用回放 ──────────────────────────────────────────────

/// 通用回放：将音频帧发送到设备
///
/// 与具体策略无关，任何策略产生的 AudioFrame 都通过此函数发送。
///
/// ## 流程
/// 1. 会话状态切换为 `Playing`
/// 2. 发送 `TTS::Start` 通知设备开始播放
/// 3. 前 5 帧免延迟发送（预缓冲优化）
/// 4. 后续帧间隔 60ms，通过 `tokio::select!` 监听中断
/// 5. 发送 `TTS::Stop`，状态恢复为 `Ready`
///
/// ## 中断处理
/// - `Listen::Start` 或 `Abort`：立即停止播放，发送 `TTS::Stop`
/// - 连接关闭：立即退出
async fn playback_frames(socket: &mut WebSocket, session: &mut Session, frames: Vec<AudioFrame>) {
    session.state = SessionState::Playing;

    if frames.is_empty() {
        tracing::debug!("Playback skipped: empty frames");
        session.state = SessionState::Ready;
        return;
    }

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
        session.state = SessionState::Ready;
        return;
    }

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
        strategy = session.strategy.name(),
        "Playback completed",
    );
}

/// 策略回放：通过当前策略生成音频帧并发送给设备
///
/// 根据策略能力自动选择：
/// - 流式回放（边合成边播放）→ [`generate_response_stream`] + [`playback_frames_stream`]
/// - 批处理回放 → [`generate_response`] + [`playback_frames`]
async fn strategy_playback(socket: &mut WebSocket, session: &mut Session) {
    session.recording_deadline = None;
    let buffer = std::mem::take(&mut session.audio_buffer);

    if session.strategy.supports_streaming_playback() {
        // ── 流式回放路径 ──
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<AudioFrame>(16);
        let strategy = session.strategy.clone();
        let session_id = session.session_id.clone();

        // 后台生成音频帧
        let gen_handle: tokio::task::JoinHandle<Result<(), String>> = tokio::spawn(async move {
            strategy
                .generate_response_stream(buffer, &session_id, frame_tx)
                .await
        });

        // 逐帧播放
        playback_frames_stream(socket, session, frame_rx, gen_handle).await;
    } else {
        // ── 批处理回放路径 ──
        match session
            .strategy
            .generate_response(buffer, &session.session_id)
            .await
        {
            Ok(frames) => {
                playback_frames(socket, session, frames).await;
            }
            Err(e) => {
                tracing::warn!(
                    device_id = %session.device_id,
                    strategy = session.strategy.name(),
                    error = %e,
                    "Strategy failed to generate response",
                );
                session.state = SessionState::Ready;
            }
        }
    }
}

/// 流式回放：从 channel 逐帧读取音频帧并发送给设备
///
/// 与 [`playback_frames`] 功能相同（含中断处理），但帧是边生成边到达的。
///
/// 包含预缓冲机制：在开始播放前先收集一定数量的帧，
/// 确保播放启动后帧供给平滑，避免因帧到达延迟导致的卡顿。
///
/// # 参数
///
/// * `socket` — WebSocket 连接
/// * `session` — 当前会话
/// * `frame_rx` — 接收端，音频帧逐个到达
/// * `gen_handle` — 后台生成任务，用于等待生成完成和检测错误
async fn playback_frames_stream(
    socket: &mut WebSocket,
    session: &mut Session,
    mut frame_rx: tokio::sync::mpsc::Receiver<AudioFrame>,
    mut gen_handle: tokio::task::JoinHandle<Result<(), String>>,
) {
    session.state = SessionState::Playing;

    // ── Phase 1: 预缓冲 ─────────────────────────────────────────
    // 在开始播放前先收集若干帧，给 TTS 生成足够的 head start
    // 10 帧 × 60ms = 600ms 预缓冲，在设备播放完这 600ms 内容之前
    // TTS 有充足时间生成后续音频

    const PREBUFFER_COUNT: usize = 10;
    let mut prebuffer: Vec<AudioFrame> = Vec::with_capacity(PREBUFFER_COUNT);
    let mut gen_done = false;

    while prebuffer.len() < PREBUFFER_COUNT {
        tokio::select! {
            frame = frame_rx.recv() => {
                match frame {
                    Some(frame) => prebuffer.push(frame),
                    None => {
                        // 生成任务已经结束，不管预缓冲了多少帧都直接播放
                        break;
                    }
                }
            }
            result = &mut gen_handle => {
                gen_done = true;
                match result {
                    Ok(Ok(())) => {
                        // 生成正常完成
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            error = %e,
                            "Streaming generation error during prebuffer",
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Streaming generation task panicked during prebuffer",
                        );
                    }
                }
                break;
            }
        }
    }

    if prebuffer.is_empty() {
        tracing::warn!("Prebuffer is empty, skipping playback");
        session.state = SessionState::Ready;
        return;
    }

    // ── Phase 2: 开始播放 + 发送预缓冲帧 ───────────────────────

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
        tracing::warn!("Failed to send TTS::Start, aborting streaming playback");
        session.state = SessionState::Ready;
        return;
    }

    let mut frame_count: usize = 0;

    // 预缓冲帧全部立即发送（免延迟）
    for frame in &prebuffer {
        let encoded = encode_protocol2(&frame.data, frame.timestamp);
        if socket.send(Message::Binary(encoded.into())).await.is_err() {
            tracing::warn!("Streaming playback: connection lost during prebuffer send");
            session.state = SessionState::Ready;
            return;
        }
        frame_count += 1;
    }

    tracing::debug!(
        device_id = %session.device_id,
        prebuffer = prebuffer.len(),
        "Prebuffer sent, starting steady-state playback",
    );

    // ── Phase 3: 稳态播放 ──────────────────────────────────────
    // 预缓冲帧已发出，设备有 600ms 的音频可播放。
    // 后续帧按 60ms 间隔逐个发送，保持与设备播放节奏同步。
    // 因为有预缓冲做 head start，即使个别帧迟到几毫秒也不会卡顿。

    loop {
        tokio::select! {
            frame = frame_rx.recv(), if !gen_done => {
                match frame {
                    Some(frame) => {
                        frame_count += 1;

                        let encoded = encode_protocol2(&frame.data, frame.timestamp);
                        if socket.send(Message::Binary(encoded.into())).await.is_err() {
                            tracing::warn!("Streaming playback: connection lost");
                            session.state = SessionState::Ready;
                            return;
                        }

                        // 60ms 帧间隔 + 中断监听（与批处理模式一致）
                        if interruptible_sleep(socket, session).await {
                            session.state = SessionState::Ready;
                            return;
                        }
                    }
                    None => {
                        gen_done = true;
                    }
                }
            }
            result = &mut gen_handle, if !gen_done => {
                gen_done = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "Streaming generation error (draining)");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Streaming generation panicked");
                    }
                }
            }
        }

        if gen_done {
            // Drain 剩余帧（仍在 channel 中的已完成帧）
            while let Some(frame) = frame_rx.recv().await {
                frame_count += 1;
                let encoded = encode_protocol2(&frame.data, frame.timestamp);
                if socket.send(Message::Binary(encoded.into())).await.is_err() {
                    break;
                }
                if interruptible_sleep(socket, session).await {
                    session.state = SessionState::Ready;
                    return;
                }
            }
            break;
        }
    }

    // 发送 TTS::Stop
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
        frame_count = frame_count,
        strategy = session.strategy.name(),
        "Streaming playback completed",
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

/// 等待 60ms 帧间隔，同时监听设备中断信号
///
/// 返回 `true` 表示需要中断播放
async fn interruptible_sleep(socket: &mut WebSocket, session: &mut Session) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(60)) => {
            false // 正常等待，继续播放
        }
        msg = socket.recv() => {
            match msg {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(cmd) = serde_json::from_str::<ClientMessage>(&text) {
                        if matches!(cmd, ClientMessage::Listen { state: ListenState::Start, .. } | ClientMessage::Abort) {
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
                            return true;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    tracing::debug!("Connection closed during playback");
                    session.state = SessionState::Ready;
                    return true;
                }
                _ => {}
            }
            false
        }
    }
}
