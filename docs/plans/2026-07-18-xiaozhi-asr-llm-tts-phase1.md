# Phase 1: ASR → LLM → TTS 语音对话管线

## 概述

在已有的 `AsrTtsStrategy`（ASR → TTS）管线中插入 LLM 步骤，实现：
ESP32 说话 → ASR 识别 → Claude/Codex LLM 处理 → TTS 合成 → ESP32 播放

## 架构

```
┌─────────────────────────────────────────────────────────┐
│                    AsrLlmTtsStrategy                     │
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐ │
│  │Opus→PCM  │  │ Doubao   │  │ Claude   │  │ Doubao   │ │
│  │解码      │→│ ASR      │→│ Agent    │→│ TTS      │ │
│  │(16kHz)   │  │(流式)    │  │(--print) │  │(PCM 24k) │ │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘ │
│                                        ┌──────────┐     │
│                                     →  │PCM→Opus  │     │
│                                        │编码(24k) │     │
│                                        └──────────┘     │
└─────────────────────────────────────────────────────────┘
```

## 数据流

```
ESP32 Opus帧 (16kHz, 60ms)
  ↓ decode_opus_frames_to_pcm() — 复用 AsrTtsStrategy
PCM16 mono 16000Hz
  ↓ asr_listen_to_text() — 复用 AsrTtsStrategy
用户语音文本 (String)
  ↓ agent.process(text, session_id) — ClaudeAgent
LLM 响应文本 (String)
  ↓ DoubaoTts::synthesize(format="pcm")
PCM16 mono 24000Hz
  ↓ pcm_to_opus_frames() — 复用
Vec<OpusPacket>
  ↓ 封装 AudioFrame { timestamp, data }
playback_frames() → ESP32 播放
```

## 组件设计

### 1. `AsrLlmTtsStrategy` 结构体

```rust
pub struct AsrLlmTtsStrategy {
    // -- ASR/TTS 配置（同 AsrTtsStrategy） --
    app_key: String,
    access_token: String,
    voice: Option<String>,
    resource_id: Option<String>,
    cluster: Option<String>,

    // -- LLM 配置 --
    /// AI Agent（ClaudeAgent / McpAgent 等）
    agent: Arc<dyn AgentProvider>,
    /// LLM 会话 ID（多轮对话用）
    llm_session_id: Mutex<Option<String>>,
}
```

**关键设计决策：**
- `agent` 使用 `Arc<dyn AgentProvider>`，与 gateway 侧使用相同的 trait，无需额外抽象
- `llm_session_id` 用 `Mutex<Option<String>>` 保护，因为 `generate_response` 是 `&self`（不可变引用），而 session_id 需要跨调用保持
- ASR/TTS 配置直接复用 `AsrTtsStrategy` 的同名字段

### 2. `generate_response()` 实现

```
audio_buffer (Vec<AudioFrame>)
  │
  ├── 空缓冲区检查 → 返回 Ok(vec![])
  │
  ├── Step 1: Opus 解码
  │   └── decode_opus_frames_to_pcm(&audio_buffer, 16000, 60)
  │       return Err → 日志 + 返回错误
  │
  ├── Step 2: Doubao ASR
  │   ├── 构造 DoubaoAsr（Streaming 模式）
  │   ├── asr_listen_to_text() → 文本
  │   ├── 30s 超时
  │   └── 空文本检查 → 日志 + 返回空
  │
  ├── Step 3: LLM 处理
  │   ├── 读取当前 llm_session_id
  │   ├── agent.process(text, llm_session_id) → (response, new_id)
  │   ├── 60s 超时（LLM 可能较长）
  │   ├── 保存 new_id → llm_session_id
  │   └── 空响应检查 → 错误处理
  │
  ├── Step 4: Doubao TTS
  │   ├── 构造 DoubaoTts（PCM 格式）
  │   ├── synthesize(llm_response)
  │   └── 空音频检查 → 日志
  │
  ├── Step 5: PCM → Opus 编码 (24kHz, 60ms)
  │   └── pcm_to_opus_frames()
  │
  └── Step 6: 封装 AudioFrame → 返回
```

### 3. CLI 变更

在 `Commands::Serve` 中新增参数：

```rust
/// ASR-LLM-TTS 模式：语音识别 → AI 处理 → 语音合成
#[arg(long)]
xiaozhi_llm: bool,

/// LLM 提供者（仅 --xiaozhi-llm 有效）
#[arg(long, default_value = "claude-code")]
xiaozhi_llm_provider: String,
```

策略选择逻辑：

```rust
let strategy: Arc<dyn ResponseStrategy> = if xiaozhi_llm {
    let agent = create_agent(Some("claude-code"))?;
    Arc::new(AsrLlmTtsStrategy::new(
        app_key(), access_token(),
        xiaozhi_tts_voice, agent,
    ))
} else if xiaozhi_asr_tts {
    // 现有逻辑
} else if let Some(text) = xiaozhi_tts_text {
    // 现有逻辑
} else {
    Arc::new(EchoStrategy)
};
```

## 错误处理策略

| 错误类型 | 处理方式 |
|---------|---------|
| Opus 解码失败 | 返回 Err，不播放 |
| ASR 超时 (30s) | 日志警告，返回空 Vec |
| ASR 返回空文本 | 日志警告，返回空 Vec |
| LLM process 失败 | 返回 Err，设备无响应 |
| LLM 超时 (60s) | 返回 Err |
| LLM 返回空 | 返回 Err |
| TTS 合成失败 | 返回 Err |
| Opus 编码失败 | 返回 Err |
| TTS 返回空音频 | 日志警告，返回空 Vec |

所有错误均在 `strategy_playback()` 中捕获（已有机制），不会导致 WebSocket 连接断开。

## 测试计划

### 单元测试
1. **策略基本信息**：name()、hello_audio_params()
2. **Opus 编解码往返**：复用现有测试
3. **ASR 空结果处理**：模拟空文本
4. **LLM session 管理**：验证 session_id 持久化
5. **边界条件**：空缓冲区、无效参数
6. **Send + Sync**：trait 约束验证

### 集成测试（手动）
1. 启动 `haimen serve --xiaozhi-llm`
2. ESP32 说话 → 等待 LLM 响应 + TTS 回播
3. 多轮对话验证上下文连续性
4. 超时场景：录音 5 秒、LLM 60 秒
5. 中断场景：播放时打断

## 文件清单

| 操作 | 文件 | 说明 |
|------|------|------|
| 新建 | `src/xiaozhi_asr_llm_tts.rs` | 核心策略实现 |
| 修改 | `src/cli.rs` | 新增 --xiaozhi-llm 参数 |
| 修改 | `src/lib.rs` | 注册新模块 |
| 不改 | `crates/haimen-xiaozhi/src/strategy.rs` | ResponseStrategy trait 足够 |
| 不改 | `crates/haimen-xiaozhi/src/ws.rs` | 无需修改 |
| 不改 | `src/agents/claude_code/agent.rs` | ClaudeAgent 直接复用 |

## 验收标准

1. `cargo build` 编译通过
2. `cargo test -- --test-threads=1` 全部通过
3. `cargo clippy -- -D warnings` 无警告
4. `cargo fmt --check` 格式正确
5. CLI 参数 `--xiaozhi-llm` 可正确选择和实例化策略
6. 策略中 ASR → LLM → TTS 流程完整、错误处理合理
