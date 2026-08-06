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
    AudioFrame, AudioParams, ClientMessage, ListenState, PlaybackEvent, ServerMessage,
    SessionState, TtsState,
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
        // 录音状态下，用 select! 竞争消息、VAD 完成信号和安全超时
        let msg = if session.state == SessionState::Recording {
            let deadline = session
                .recording_deadline
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(30));
            let tokio_deadline = tokio::time::Instant::from_std(deadline);

            // 获取 VAD 完成信号（如果策略支持流式 ASR + VAD）
            let vad_completion = session.strategy.vad_completion();
            let has_vad = vad_completion.is_some();

            // VAD 等待 future（需在 select! 前 pin 住）
            let vad_fut = async move {
                if let Some(notify) = vad_completion {
                    notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::pin!(vad_fut);

            // 获取无语音超时信号（策略可选）
            let no_speech_completion = session.strategy.no_speech_completion();
            let has_no_speech = no_speech_completion.is_some();

            let no_speech_fut = async move {
                if let Some(notify) = no_speech_completion {
                    notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::pin!(no_speech_fut);

            tokio::select! {
                msg = socket.recv() => msg,
                _ = tokio::time::sleep_until(tokio_deadline) => {
                    tracing::info!(
                        device_id = %session.device_id,
                        strategy = session.strategy.name(),
                        "Recording safety timeout (30s), triggering strategy playback",
                    );
                    session.recording_deadline = None;
                    strategy_playback(&mut socket, &mut session).await;
                    continue;
                }
                _ = &mut vad_fut, if has_vad => {
                    tracing::info!(
                        device_id = %session.device_id,
                        strategy = session.strategy.name(),
                        "VAD detected end of speech, triggering strategy playback",
                    );
                    session.recording_deadline = None;
                    strategy_playback(&mut socket, &mut session).await;
                    continue;
                }
                _ = &mut no_speech_fut, if has_no_speech => {
                    tracing::info!(
                        device_id = %session.device_id,
                        strategy = session.strategy.name(),
                        "No-speech timeout, playing goodbye and closing connection",
                    );
                    session.recording_deadline = None;
                    // 合成并播放「拜拜」（最长 5s）；失败/空帧则跳过播放，
                    // 但无论如何都要关闭连接结束这段无语音对话
                    if let Some(frames) = session.strategy.goodbye_frames(&session.session_id).await {
                        if !frames.is_empty() {
                            play_greeting_frames(&mut socket, &mut session, frames).await;
                        }
                    }
                    // axum 0.8 WebSocket 无 close()：发规范 Close 帧后 return（drop socket 完成关闭）
                    let _ = socket.send(Message::Close(None)).await;
                    return;
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

            // 主动播报唤醒问候：策略决定是否合成（默认 no-op 返回 None）。
            // 使用 play_greeting_frames（不监听中断）：设备唤醒后紧接着的
            // listen/start（录音轮）不会打断问候，会完整播完；设备消息
            // 留在 socket 缓冲中，播放结束后按序处理，用户语音不丢失。
            if let Some(frames) = session.strategy.wake_greeting(&session.session_id).await {
                if !frames.is_empty() {
                    play_greeting_frames(socket, session, frames).await;
                }
            }
        }
        ListenState::Start => {
            tracing::debug!(
                device_id = %session.device_id,
                "Listen::Start — recording started",
            );
            enter_recording(session).await;
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

// ─── 录音状态迁移 ──────────────────────────────────────────

/// 将会话切换到录音状态（`Listen::Start` 的公共逻辑）
///
/// 供 [`handle_listen`] 和播放中断路径（[`handle_playback_interrupt`]）复用，
/// 保证「开始录音」这一状态迁移在任何入口下行为一致。
async fn enter_recording(session: &mut Session) {
    session.audio_buffer.clear();
    session.cumulated_timestamp = 0;
    session.recording_deadline = None;
    session.state = SessionState::Recording;

    // 如果策略支持流式 ASR，通知其新一轮录音开始
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
        if let Some(interrupt) = wait_frame_interrupt(socket).await {
            handle_playback_interrupt(socket, session, interrupt).await;
            return;
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

/// 主动问候回放：将音频帧发送到设备（不监听设备中断）
///
/// 与 [`playback_frames`] 不同，此路径**不读取 socket**：
/// - 不用 `wait_frame_interrupt` 监听 `listen/start`/`abort`，避免唤醒问候
///   被设备紧接着开始的录音轮（`listen/start`）打断。
/// - 设备在播放期间发来的消息（`listen/start`、音频帧）留在 socket 接收缓冲中，
///   播放结束后由主循环按序处理（进入录音、缓冲音频），用户语音不丢失，
///   只是延后 ~1 帧 × 帧数 的处理时间。
/// - 帧按 60ms 节奏下发（与设备播放速率同步）；最后一帧后再等一帧时长才发
///   `TTS::Stop`，避免尾帧被立即停播截断。
///
/// 适用于「服务端主动播报」场景（如唤醒问候）。若需支持设备打断播放，
/// 应使用 [`playback_frames`]。
async fn play_greeting_frames(
    socket: &mut WebSocket,
    session: &mut Session,
    frames: Vec<AudioFrame>,
) {
    session.state = SessionState::Playing;

    if frames.is_empty() {
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
        session.state = SessionState::Ready;
        return;
    }

    let total = frames.len();
    for frame in frames.iter() {
        let encoded = encode_protocol2(&frame.data, frame.timestamp);
        if socket.send(Message::Binary(encoded.into())).await.is_err() {
            session.state = SessionState::Ready;
            return;
        }

        // 帧间隔 60ms（与设备播放节奏同步）；最后一帧后也等一帧时长，
        // 让设备播完尾帧再收到 TTS::Stop，避免截断最后一个字。
        // 期间不读取 socket：设备消息排队，播放结束后由主循环按序处理。
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    // 所有帧播放完毕，发送 TTS::Stop
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
        "Greeting playback completed",
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
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<PlaybackEvent>(16);
        let strategy = session.strategy.clone();
        let session_id = session.session_id.clone();

        // 后台生成音频帧（中断时由 playback_frames_stream 内的守卫自动取消）
        let gen_handle = tokio::spawn(async move {
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
    mut frame_rx: tokio::sync::mpsc::Receiver<PlaybackEvent>,
    mut gen_handle: tokio::task::JoinHandle<Result<(), String>>,
) {
    // 所有提前退出路径都会取消生成任务，避免播放被中断后 ASR/LLM/TTS 继续白跑
    let mut cancel_on_drop = CancelGuard(Some(gen_handle.abort_handle()));

    session.state = SessionState::Playing;

    // ── 诊断：帧到达间隔追踪 ──
    // 记录相邻 Audio 帧到达 frame_rx 的时间间隔，间隔超过阈值即视为 TTS 合成
    // 断档（固件播放缓冲会 underrun → 跳帧，听感"半个字/几个字压缩在一起"）。
    // 连续音频管道（ContinuityPump）接入后，帧到达间隔应恒 ≤60ms（无断档），
    // 本诊断退化为"连续供给被破坏"的回归探测器：汇总日志 max_gap_ms / warn_count
    // 均应为 0 附近，若出现大 gap 说明连续供给被破坏，需回归排查。
    let mut last_frame_arrival: Option<Instant> = None;
    let mut gap_warn_count: usize = 0;
    let mut max_gap_ms: u128 = 0;

    // ── Phase 1: 预缓冲 ─────────────────────────────────────────
    // 在开始播放前先收集若干帧，给 TTS 生成足够的 head start。
    // 10 帧 × 60ms = 600ms 预缓冲：实测 TTS 合成速率 ~7-8x 实时，
    // 600ms 已足以覆盖网络抖动。预缓冲不宜过大——Tts::Start 后预缓冲帧
    // 是一次性连发的，若超出固件 jitter buffer 容量会溢出丢帧
    // （同样表现为"半个字/语速压缩"）。
    //
    // 文本事件不参与预缓冲计数：
    // - Stt 立即转发（固件对 stt 的处理不依赖状态机，任何状态都会写屏）
    // - LlmSentence 缓存到 pending_sentences，待 Tts::Start 后统一 flush，
    //   避免助手文本先于设备进入 Speaking 态上屏

    const PREBUFFER_COUNT: usize = 10;
    let mut prebuffer: Vec<AudioFrame> = Vec::with_capacity(PREBUFFER_COUNT);
    let mut pending_sentences: Vec<String> = Vec::new();
    let mut gen_done = false;

    while prebuffer.len() < PREBUFFER_COUNT {
        tokio::select! {
            event = frame_rx.recv() => {
                match event {
                    Some(event) => {
                        match &event {
                            PlaybackEvent::Stt(text) => {
                                if !send_text_event(socket, session, &PlaybackEvent::Stt(text.clone())).await {
                                    session.state = SessionState::Ready;
                                    return;
                                }
                            }
                            PlaybackEvent::LlmSentence(text) => {
                                pending_sentences.push(text.clone());
                            }
                            PlaybackEvent::Audio(frame) => {
                                record_frame_arrival_gap(
                                    session,
                                    &mut last_frame_arrival,
                                    &mut gap_warn_count,
                                    &mut max_gap_ms,
                                    "prebuffer",
                                );
                                prebuffer.push(frame.clone());
                            }
                        }
                    }
                    None => {
                        // 生成任务已经结束，不管预缓冲了多少帧都直接播放
                        break;
                    }
                }
            }
            result = &mut gen_handle => {
                gen_done = true;
                cancel_on_drop.disarm();
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

    // 无音频但有文本时：直接下发缓存的句子文本后返回
    if prebuffer.is_empty() {
        for sentence in &pending_sentences {
            if !send_text_event(
                socket,
                session,
                &PlaybackEvent::LlmSentence(sentence.clone()),
            )
            .await
            {
                break;
            }
        }
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

    // Tts::Start 后立即 flush 预缓冲期间缓存的句子文本
    for sentence in pending_sentences {
        if !send_text_event(socket, session, &PlaybackEvent::LlmSentence(sentence)).await {
            session.state = SessionState::Ready;
            return;
        }
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

    // 发送时钟：预缓冲连发结束后，稳态帧从 T0+60ms 起，此后每 60ms 一帧。
    // 用绝对时刻网格对齐（而非每次重新 sleep 60ms），且每帧发送完成后
    // next_send_at 累加 60ms（而非取当前时刻）——否则帧间隔会变成
    // 60ms+发送耗时，逐帧累积变慢，固件缓冲被逐渐耗尽后触发追赶跳帧
    // （听感"开头正常、后面内容挤在一起"）。
    let mut next_send_at = std::time::Instant::now() + std::time::Duration::from_millis(60);

    tracing::debug!(
        device_id = %session.device_id,
        prebuffer = prebuffer.len(),
        "Prebuffer sent, starting steady-state playback",
    );

    // ── Phase 3: 稳态播放 ──────────────────────────────────────
    // 预缓冲帧已发出，设备有约 600ms 的音频可播放。
    // 后续帧按固定 60ms 间隔发送（由 next_send_at 发送时钟对齐），
    // 保持与设备播放节奏同步；合成慢导致帧断供时由预缓冲兜底。
    // 发送时钟避免 socket 有消息返回时 wait_frame_interrupt 提前结束、
    // 压缩帧间隔导致的突发快发（会冲击设备 jitter buffer 引发丢帧）。

    loop {
        tokio::select! {
            event = frame_rx.recv(), if !gen_done => {
                match event {
                    Some(event) => {
                        match &event {
                            PlaybackEvent::Stt(_) | PlaybackEvent::LlmSentence(_) => {
                                // 稳态阶段文本事件立即转发（Tts::Start 已发出）
                                if !send_text_event(socket, session, &event).await {
                                    session.state = SessionState::Ready;
                                    return;
                                }
                            }
                            PlaybackEvent::Audio(frame) => {
                                frame_count += 1;

                                // 诊断：帧到达间隔（合成节奏）
                                record_frame_arrival_gap(
                                    session,
                                    &mut last_frame_arrival,
                                    &mut gap_warn_count,
                                    &mut max_gap_ms,
                                    "steady",
                                );

                                // 等待至发送时钟对齐（距上次发送 60ms），期间监听中断
                                if let Some(interrupt) =
                                    wait_until_send_slot(socket, &mut next_send_at).await
                                {
                                    handle_playback_interrupt(socket, session, interrupt).await;
                                    return;
                                }

                                let encoded = encode_protocol2(&frame.data, frame.timestamp);
                                if socket.send(Message::Binary(encoded.into())).await.is_err() {
                                    tracing::warn!("Streaming playback: connection lost");
                                    session.state = SessionState::Ready;
                                    return;
                                }
                                // 发送时钟累加 60ms（而非取当前时刻），发送耗时由
                                // 下一帧等待时追回，保证平均节奏严格 60ms、不累积变慢。
                                next_send_at += std::time::Duration::from_millis(60);
                            }
                        }
                    }
                    None => {
                        gen_done = true;
                    }
                }
            }
            result = &mut gen_handle, if !gen_done => {
                gen_done = true;
                cancel_on_drop.disarm();
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
            // Drain 剩余事件（仍在 channel 中的已完成事件）
            while let Some(event) = frame_rx.recv().await {
                match &event {
                    PlaybackEvent::Stt(_) | PlaybackEvent::LlmSentence(_) => {
                        if !send_text_event(socket, session, &event).await {
                            break;
                        }
                    }
                    PlaybackEvent::Audio(frame) => {
                        frame_count += 1;
                        // 诊断：帧到达间隔（合成节奏）
                        record_frame_arrival_gap(
                            session,
                            &mut last_frame_arrival,
                            &mut gap_warn_count,
                            &mut max_gap_ms,
                            "drain",
                        );
                        // drain 阶段同样受发送时钟约束，避免突发快发
                        if let Some(interrupt) =
                            wait_until_send_slot(socket, &mut next_send_at).await
                        {
                            handle_playback_interrupt(socket, session, interrupt).await;
                            return;
                        }
                        let encoded = encode_protocol2(&frame.data, frame.timestamp);
                        if socket.send(Message::Binary(encoded.into())).await.is_err() {
                            break;
                        }
                        // 发送时钟累加 60ms，与稳态分支一致，drain 阶段不累积变慢
                        next_send_at += std::time::Duration::from_millis(60);
                    }
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

    // 帧供给诊断汇总：连续音频管道正常时 max_gap_ms 应远小于 300ms（帧间隔恒 60ms）、
    // gap_warn_count=0。max_gap_ms 明显偏大或 warn_count>0 说明连续供给被破坏（回归信号）。
    if gap_warn_count > 0 {
        tracing::warn!(
            device_id = %session.device_id,
            frame_count = frame_count,
            audio_ms = frame_count as u64 * 60u64,
            max_gap_ms = max_gap_ms,
            gap_warn_count = gap_warn_count,
            "流式回放诊断汇总: 播放期间发生 {} 次合成断档（帧到达间隔 > 300ms，max_gap={}ms）",
            gap_warn_count,
            max_gap_ms,
        );
    } else {
        tracing::debug!(
            device_id = %session.device_id,
            frame_count = frame_count,
            audio_ms = frame_count as u64 * 60u64,
            max_gap_ms = max_gap_ms,
            "流式回放诊断汇总: 连续供给正常（无断档，max_gap={}ms）",
            max_gap_ms,
        );
    }
}

// ─── 工具函数 ──────────────────────────────────────────────

/// 发送文本事件对应的设备消息（`stt` / `tts/sentence_start`）
///
/// 返回 `false` 表示发送失败（连接已断开），调用方应停止回放并复位状态。
async fn send_text_event(socket: &mut WebSocket, session: &Session, event: &PlaybackEvent) -> bool {
    match event {
        PlaybackEvent::Stt(text) => send_json(
            socket,
            &ServerMessage::Stt {
                session_id: session.session_id.clone(),
                text: text.clone(),
            },
        )
        .await
        .is_ok(),
        PlaybackEvent::LlmSentence(text) => send_json(
            socket,
            &ServerMessage::Tts {
                session_id: session.session_id.clone(),
                state: TtsState::SentenceStart,
                text: Some(text.clone()),
            },
        )
        .await
        .is_ok(),
        PlaybackEvent::Audio(_) => true,
    }
}

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

/// 播放期间设备发送的中断信号
enum PlaybackInterrupt {
    /// 连接关闭
    Closed,
    /// 收到 `Abort` 指令
    Abort,
    /// 收到 `Listen::Start` 指令（中断播放并开始新一轮录音）
    ListenStart,
}

/// 等待 60ms 帧间隔，同时监听设备中断信号
///
/// 返回 `Some` 表示收到中断信号（此时本函数未改动会话状态，状态迁移
/// 统一由 [`handle_playback_interrupt`] 完成）；返回 `None` 表示正常帧间隔
/// 结束，继续播放。
async fn wait_frame_interrupt(socket: &mut WebSocket) -> Option<PlaybackInterrupt> {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(60)) => {
            None // 正常等待，继续播放
        }
        msg = socket.recv() => match msg {
            Some(Ok(Message::Text(text))) => {
                if let Ok(cmd) = serde_json::from_str::<ClientMessage>(&text) {
                    match cmd {
                        ClientMessage::Abort => Some(PlaybackInterrupt::Abort),
                        ClientMessage::Listen { state: ListenState::Start, .. } => {
                            Some(PlaybackInterrupt::ListenStart)
                        }
                        // 其他消息（如 Listen::Stop/Detect）忽略，继续等待下一帧间隔
                        _ => None,
                    }
                } else {
                    None
                }
            }
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                tracing::debug!("Connection closed during playback");
                Some(PlaybackInterrupt::Closed)
            }
            _ => None,
        },
    }
}

/// 发送时钟追赶修正：时钟落后实时超过一帧时长（60ms）时，快进到当前时刻。
///
/// 断档（TTS 合成暂停 / LLM 思考）期间没有帧可发，`next_send_at` 停留在
/// 最后一次发送的时刻而逐渐落后于实时。若不清零，恢复后的积压帧会在
/// `wait == 0` 分支被瞬间连发（突发快发），冲击设备 jitter buffer 使其
/// 快速排空积压音频——听感"多个字挤压在一起快速念完"。快进到当前时刻后，
/// 恢复的帧从下一帧起按正常 60ms 节奏发送。
///
/// 落后不足一帧（60ms）属于正常调度抖动，不清零（保持原有立即发送 + 累加行为）。
fn snap_send_clock(next_send_at: &mut Instant, now: Instant) {
    let one_frame = Duration::from_millis(60);
    if *next_send_at + one_frame < now {
        *next_send_at = now;
    }
}

/// 等待至「发送时钟」的下一帧时刻（距上次发送 60ms），期间监听中断
///
/// 与 [`wait_frame_interrupt`] 的区别：后者每次调用都重新起一个 60ms 计时，
/// socket 有消息返回时会提前结束、压缩帧间隔导致突发快发；本函数以
/// `next_send_at` 这个绝对时刻对齐，socket 有消息时忽略并继续等待，
/// 保证帧发送间隔稳定在 60ms，避免设备 jitter buffer 因快慢不均而丢帧。
///
/// 断档恢复后由 [`snap_send_clock`] 修正时钟，避免积压帧被瞬间连发。
///
/// 返回 `Some` 表示收到中断信号；`None` 表示已到发送时刻，可发送下一帧。
async fn wait_until_send_slot(
    socket: &mut WebSocket,
    next_send_at: &mut Instant,
) -> Option<PlaybackInterrupt> {
    loop {
        let now = Instant::now();
        snap_send_clock(next_send_at, now);
        let wait = next_send_at.saturating_duration_since(now);
        if wait.is_zero() {
            return None;
        }
        let sleep = tokio::time::sleep(wait);
        tokio::pin!(sleep);
        tokio::select! {
            _ = &mut sleep => return None,
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(cmd) = serde_json::from_str::<ClientMessage>(&text) {
                        match cmd {
                            ClientMessage::Abort => return Some(PlaybackInterrupt::Abort),
                            ClientMessage::Listen { state: ListenState::Start, .. } => {
                                return Some(PlaybackInterrupt::ListenStart);
                            }
                            // 其他消息忽略，继续等待至发送时刻
                            _ => continue,
                        }
                    } else {
                        continue;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    tracing::debug!("Connection closed during playback");
                    return Some(PlaybackInterrupt::Closed);
                }
                _ => continue,
            },
        }
    }
}

/// 帧到达间隔诊断阈值（ms）：超过即视为一次合成断档（固件 underrun 会跳帧）
const FRAME_GAP_WARN_MS: u128 = 300;

/// 诊断辅助：记录相邻 Audio 帧到达 frame_rx 的时间间隔。
///
/// 间隔超过 [`FRAME_GAP_WARN_MS`]（300ms）即打 WARN 日志——说明该窗口内 TTS
/// 合成断供，固件播放缓冲会 underrun（表现为"半个字"或"几个字压缩在一起"）。
/// 真机跑一轮后据日志判断断档频率与大小，再针对性优化合成节奏。
fn record_frame_arrival_gap(
    session: &Session,
    last_frame_arrival: &mut Option<Instant>,
    gap_warn_count: &mut usize,
    max_gap_ms: &mut u128,
    phase: &str,
) {
    let now = Instant::now();
    if let Some(last) = last_frame_arrival {
        let gap_ms = now.duration_since(*last).as_millis();
        if gap_ms > *max_gap_ms {
            *max_gap_ms = gap_ms;
        }
        if gap_ms > FRAME_GAP_WARN_MS {
            *gap_warn_count += 1;
            tracing::warn!(
                device_id = %session.device_id,
                phase = %phase,
                gap_ms = gap_ms,
                warn_count = *gap_warn_count,
                "流式回放诊断: 帧到达间隔过大（合成断档）",
            );
        }
    }
    *last_frame_arrival = Some(now);
}

/// 处理播放期间的中断信号，统一完成状态迁移并停止播放
///
/// - 连接关闭 / `Abort`：会话回到 `Ready`
/// - `Listen::Start`：中断播放并进入新一轮录音（`Recording`），
///   使设备随后发送的音频帧能被正常缓冲，避免对话在中断后静默失效
async fn handle_playback_interrupt(
    socket: &mut WebSocket,
    session: &mut Session,
    interrupt: PlaybackInterrupt,
) {
    // 无论何种中断，先停止播放并告知设备
    let _ = send_json(
        socket,
        &ServerMessage::Tts {
            session_id: session.session_id.clone(),
            state: TtsState::Stop,
            text: None,
        },
    )
    .await;

    match interrupt {
        PlaybackInterrupt::ListenStart => {
            tracing::debug!(
                device_id = %session.device_id,
                "Playback interrupted by Listen::Start, entering recording",
            );
            enter_recording(session).await;
        }
        PlaybackInterrupt::Abort => {
            tracing::debug!(
                device_id = %session.device_id,
                "Playback interrupted by Abort",
            );
            session.state = SessionState::Ready;
        }
        PlaybackInterrupt::Closed => {
            session.state = SessionState::Ready;
        }
    }
}

/// 生成任务取消守卫：drop 时自动取消未完成的任务
///
/// 播放被中断（Abort / Listen::Start / 连接关闭）时，若生成任务仍在运行，
/// 需要主动取消，避免 ASR/LLM/TTS 继续白跑。生成任务正常结束后通过
/// [`disarm`](Self::disarm) 解除取消（已结束的任务 abort 也无害，但显式解除更清晰）。
struct CancelGuard(Option<tokio::task::AbortHandle>);

impl CancelGuard {
    /// 生成任务已正常完成，解除取消守卫
    fn disarm(&mut self) {
        self.0.take();
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::EchoStrategy;
    use async_trait::async_trait;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::Notify;
    use tokio_tungstenite::tungstenite::Message as TMessage;

    fn make_session() -> Session {
        Session {
            device_id: "test-device".into(),
            session_id: "test-session".into(),
            audio_params: AudioParams::default(),
            state: SessionState::Ready,
            audio_buffer: vec![AudioFrame {
                timestamp: 0,
                data: vec![0x01, 0x02],
            }],
            cumulated_timestamp: 60,
            recording_deadline: Some(Instant::now()),
            strategy: Arc::new(EchoStrategy),
        }
    }

    /// `enter_recording` 应重置录音上下文并切换到 `Recording` 状态
    #[tokio::test]
    async fn test_enter_recording_resets_and_enters_recording() {
        let mut session = make_session();

        enter_recording(&mut session).await;

        assert_eq!(session.state, SessionState::Recording);
        assert!(session.audio_buffer.is_empty(), "应清空缓冲的音频帧");
        assert_eq!(session.cumulated_timestamp, 0, "应重置累计时间戳");
        assert!(
            session.recording_deadline.is_none(),
            "应清除旧的录音截止时刻"
        );
    }

    // ─── 无语音超时端到端测试 ────────────────────────────

    /// 测试用策略：镜像真实 AsrLlmTtsStrategy 的无语音超时检测入口
    /// （on_audio_frame 计数到阈值 → 触发 no_speech_completion 的 Notify）
    struct TestNoSpeechStrategy {
        notify: Arc<Notify>,
        frame_threshold: u64,
        frame_count: Arc<AtomicU64>,
    }

    #[async_trait]
    impl ResponseStrategy for TestNoSpeechStrategy {
        fn name(&self) -> &'static str {
            "test-no-speech"
        }

        async fn generate_response(
            &self,
            _audio_buffer: Vec<AudioFrame>,
            _session_id: &str,
        ) -> Result<Vec<AudioFrame>, String> {
            Ok(Vec::new())
        }

        fn supports_streaming_asr(&self) -> bool {
            true
        }

        async fn on_audio_frame(&self, _frame: &AudioFrame) -> Result<(), String> {
            let count = self.frame_count.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= self.frame_threshold {
                self.notify.notify_one();
            }
            Ok(())
        }

        fn no_speech_completion(&self) -> Option<Arc<Notify>> {
            Some(self.notify.clone())
        }

        async fn goodbye_frames(&self, _session_id: &str) -> Option<Vec<AudioFrame>> {
            Some(vec![AudioFrame {
                timestamp: 0,
                data: vec![0xAB, 0xCD],
            }])
        }
    }

    /// 端到端：设备推流静音帧达到阈值 → 服务端播「拜拜」→ 主动发 Close 帧
    #[tokio::test]
    async fn test_no_speech_end_to_end_plays_goodbye_and_closes() {
        use crate::add_routes;

        let strategy = Arc::new(TestNoSpeechStrategy {
            notify: Arc::new(Notify::new()),
            frame_threshold: 3,
            frame_count: Arc::new(AtomicU64::new(0)),
        });
        let app = add_routes(axum::Router::new(), strategy);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定测试端口失败");
        let addr = listener.local_addr().expect("获取测试端口失败");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("测试服务器启动失败");
        });

        let url = format!("ws://{}/xiaozhi/ws", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("连接测试服务器失败");

        // HELLO 握手
        let hello = r#"{"type":"hello","version":3,"transport":"websocket","audio_params":{"format":"opus","sample_rate":16000,"channels":1,"frame_duration":60},"features":{}}"#;
        ws.send(TMessage::Text(hello.into()))
            .await
            .expect("发送 HELLO 失败");
        let resp = ws.next().await.expect("读取 HELLO 响应失败").unwrap();
        assert!(matches!(resp, TMessage::Text(_)), "应收到 HELLO 响应");

        // 开始录音
        let listen = r#"{"type":"listen","state":"start","mode":"auto"}"#;
        ws.send(TMessage::Text(listen.into()))
            .await
            .expect("发送 listen start 失败");

        // 推 3 帧静音（Protocol2），触发无语音超时
        for i in 0..3u32 {
            let frame = crate::protocol::encode_protocol2(&[0u8; 4], i * 60);
            ws.send(TMessage::Binary(frame.into()))
                .await
                .expect("发送音频帧失败");
        }

        // 断言收到的消息序列：TTS::Start → 拜拜帧 → TTS::Stop → Close
        let mut saw_tts_start = false;
        let mut saw_binary = false;
        let mut saw_tts_stop = false;
        let mut saw_close = false;
        for _ in 0..10 {
            let msg = ws.next().await.expect("连接提前关闭").expect("读消息失败");
            match msg {
                TMessage::Text(t) => {
                    let s = t.to_string();
                    if s.contains("\"type\":\"tts\"") && s.contains("\"state\":\"start\"") {
                        saw_tts_start = true;
                    }
                    if s.contains("\"type\":\"tts\"") && s.contains("\"state\":\"stop\"") {
                        saw_tts_stop = true;
                    }
                }
                TMessage::Binary(_) => saw_binary = true,
                TMessage::Close(_) => {
                    saw_close = true;
                    break;
                }
                _ => {}
            }
            if saw_close {
                break;
            }
        }

        assert!(saw_tts_start, "应收到 TTS::Start（开始播告别）");
        assert!(saw_binary, "应收到拜拜音频帧");
        assert!(saw_tts_stop, "应收到 TTS::Stop（播报结束）");
        assert!(saw_close, "服务端应主动发送 Close 帧结束对话");
    }

    // ─── 断档恢复发送节奏 ────────────────────────────────

    /// 测试用策略：先发预缓冲帧，暂停一段时长（模拟 LLM 思考断档），再发后续帧。
    /// 用于验证断档恢复后帧不被突发连发（应保持 ~60ms 发送节奏）。
    struct GapResumeStrategy {
        pre_frames: usize,
        gap: Duration,
        post_frames: usize,
    }

    #[async_trait]
    impl ResponseStrategy for GapResumeStrategy {
        fn name(&self) -> &'static str {
            "test-gap-resume"
        }

        async fn generate_response(
            &self,
            _audio_buffer: Vec<AudioFrame>,
            _session_id: &str,
        ) -> Result<Vec<AudioFrame>, String> {
            Ok(Vec::new())
        }

        fn supports_streaming_playback(&self) -> bool {
            true
        }

        async fn generate_response_stream(
            &self,
            _audio_buffer: Vec<AudioFrame>,
            _session_id: &str,
            frame_tx: tokio::sync::mpsc::Sender<PlaybackEvent>,
        ) -> Result<(), String> {
            for i in 0..self.pre_frames {
                let frame = AudioFrame {
                    timestamp: (i as u32) * 60,
                    data: vec![0x01, 0x02],
                };
                if frame_tx.send(PlaybackEvent::Audio(frame)).await.is_err() {
                    return Err("回放管道已关闭".into());
                }
            }
            tokio::time::sleep(self.gap).await;
            for i in 0..self.post_frames {
                let frame = AudioFrame {
                    timestamp: ((self.pre_frames + i) as u32) * 60,
                    data: vec![0x03, 0x04],
                };
                if frame_tx.send(PlaybackEvent::Audio(frame)).await.is_err() {
                    return Err("回放管道已关闭".into());
                }
            }
            Ok(())
        }
    }

    /// 端到端：断档（帧到达暂停）后恢复的帧应保持 ~60ms 发送节奏，
    /// 而不是被瞬间连发——突发快发会冲击设备 jitter buffer，听感"多个字挤在一起"。
    #[tokio::test]
    async fn test_post_gap_frames_sent_at_steady_cadence() {
        use crate::add_routes;

        let strategy = Arc::new(GapResumeStrategy {
            pre_frames: 10,
            gap: Duration::from_millis(1500),
            post_frames: 10,
        });
        let app = add_routes(axum::Router::new(), strategy);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定测试端口失败");
        let addr = listener.local_addr().expect("获取测试端口失败");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("测试服务器启动失败");
        });

        let url = format!("ws://{}/xiaozhi/ws", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("连接测试服务器失败");

        // HELLO 握手
        let hello = r#"{"type":"hello","version":3,"transport":"websocket","audio_params":{"format":"opus","sample_rate":16000,"channels":1,"frame_duration":60},"features":{}}"#;
        ws.send(TMessage::Text(hello.into()))
            .await
            .expect("发送 HELLO 失败");
        let resp = ws.next().await.expect("读取 HELLO 响应失败").unwrap();
        assert!(matches!(resp, TMessage::Text(_)), "应收到 HELLO 响应");

        // 开始录音并停止，触发策略回放
        let listen = r#"{"type":"listen","state":"start","mode":"auto"}"#;
        ws.send(TMessage::Text(listen.into()))
            .await
            .expect("发送 listen start 失败");
        let stop = r#"{"type":"listen","state":"stop","mode":"auto"}"#;
        ws.send(TMessage::Text(stop.into()))
            .await
            .expect("发送 listen stop 失败");

        // 读取消息：Tts::Start → 10 预缓冲帧 → [断档] → 10 恢复帧 → Tts::Stop
        let mut frame_times: Vec<(usize, Instant)> = Vec::new();
        let mut saw_start = false;
        let mut saw_stop = false;
        for _ in 0..100 {
            let msg = match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(m)) => m.expect("读消息失败"),
                _ => break,
            };
            match msg {
                TMessage::Text(t) => {
                    let s = t.to_string();
                    if s.contains("\"type\":\"tts\"") && s.contains("\"state\":\"start\"") {
                        saw_start = true;
                    }
                    if s.contains("\"type\":\"tts\"") && s.contains("\"state\":\"stop\"") {
                        saw_stop = true;
                    }
                }
                TMessage::Binary(_) => {
                    frame_times.push((frame_times.len(), Instant::now()));
                }
                _ => {}
            }
            if saw_stop {
                break;
            }
        }

        assert!(saw_start, "应收到 TTS::Start");
        assert!(saw_stop, "应收到 TTS::Stop");
        assert_eq!(frame_times.len(), 20, "应收到 10 预缓冲帧 + 10 断档恢复帧");

        // 恢复帧 = 索引 10..20。它们应保持 ~60ms 发送节奏，
        // 而非断档恢复后瞬间连发。9 个 60ms 间隔 ≈ 540ms；
        // 断言 >= 300ms 以容忍 CI 抖动，同时远大于突发连发（毫秒级）。
        let first_resume = frame_times[10].1;
        let last_resume = frame_times[19].1;
        let resume_span = last_resume.duration_since(first_resume);

        assert!(
            resume_span >= Duration::from_millis(300),
            "断档恢复后的帧被突发连发（耗时仅 {:?}），会冲击设备 jitter buffer 导致语速压缩",
            resume_span,
        );
    }

    /// `snap_send_clock`：时钟落后超过一帧时长才快进，正常抖动（< 一帧）不动作
    #[test]
    fn test_snap_send_clock() {
        // 时钟在未来：不应快进
        let now = Instant::now();
        let future = now + Duration::from_millis(50);
        let mut ts = future;
        snap_send_clock(&mut ts, now);
        assert_eq!(ts, future, "时钟在未来不应被快进");

        // 落后不足一帧（<60ms）：正常调度抖动，不清零
        let now = Instant::now();
        let slightly_behind = now - Duration::from_millis(50);
        let mut ts = slightly_behind;
        snap_send_clock(&mut ts, now);
        assert_eq!(ts, slightly_behind, "落后不足一帧时不应快进");

        // 落后超过一帧（断档场景）：快进到当前时刻，避免积压帧突发连发
        let now = Instant::now();
        let far_behind = now - Duration::from_millis(2000);
        let mut ts = far_behind;
        snap_send_clock(&mut ts, now);
        assert_eq!(ts, now, "落后超过一帧时应快进到当前时刻");
    }
}
