# OpenClaw 集成可行性分析

日期: 2026-08-07
状态: 已实现（OpenClaw Agent 后端已落地，见 §5 实现记录）

---

## 1. 现状分析

### 1.1 项目当前状态

haimen 是一个 **AI 网关基建 CLI**：通过 Connector 架构接入多种消息渠道（飞书/Lark、GitHub、Telegram 等），通过 Agent 抽象接入多种 AI 后端。目前已落地的 Agent 后端：

| Agent | 调用方式 | 流式 | 事件轨迹 |
|-------|---------|------|---------|
| Claude Code | `claude --print --output-format stream-json --verbose --include-partial-messages` | ✅ 逐事件 JSONL | ✅ thinking / tool_use / tool_result |
| Codex | `codex exec --json --sandbox <mode>` | ✅ JSONL | ⚠️ 暂留空 |
| MCP | haimen-core 内嵌客户端 | — | — |

用户诉求：**参考 claude-code / codex 的 Agent 集成模式，继续实现 OpenClaw 的集成**。

### 1.2 一个需要正视的历史判断

`docs/plans/design-2026-06-19-haimen-personal-gateway.md` 中，OpenClaw 曾被明确列为**竞争对手**：

> | OpenClaw | 自包含 AI Agent 运行时 | 太重，自己做推理，不是"路由"思维 |
>
> **haimen 的差异化定位：不做 Agent，做 Agent 的网关。**

也就是说，haimen 的立身之本正是"薄网关"，而 OpenClaw 是"自包含全栈运行时"。当前诉求相当于把当年的"竞品"吸纳为自身的一个 Agent 后端——**这是一个定位层面的转向**，需要先想清楚收益，再谈技术。

---

## 2. 当前架构分析

### 2.1 Agent 抽象（`haimen-core/provider.rs`）

```
AgentProvider (trait)
 ├─ name() -> &str
 ├─ process(message, session_id, work_dir) -> (AgentOutput, new_session_id)   // 批处理
 ├─ process_stream(...) -> (TextStream, new_session_id, AgentEventStream)      // 流式，驱动 TTS
 └─ check_available() -> Result<(), String>

AgentOutput = { text, events: Vec<AgentLogEvent> }
AgentLogEvent = Thinking | ToolUse | ToolResult
```

### 2.2 子进程桥接模式（claude-code / codex 已验证的范式）

```
haimen (Rust)
   │  启动子进程（build_command 兼容 Windows npm shim）
   │  current_dir = work_dir
   ▼
外部 CLI 子进程
   │  stdout 输出 JSONL 事件流
   ▼
后台读流任务（tokio::spawn）
   ├─ 解析 JSONL → 提取 text_delta → mpsc 文本通道（供 TTS 消费）
   ├─ 提取 thinking/tool_use/tool_result → mpsc 事件通道
   └─ 提取 session_id（claude 的 session_id / codex 的 thread_id）→ oneshot
```

### 2.3 Agent 注册与配置

- `AgentRegistry::register(id, display_name, factory)` 集中注册，新增 Agent 只需在 `builtin()` 加一行
- 配置：`active_provider` + `[gateway.providers.<name>]` 键值对
- 消费方：`chat_loop`（批处理）、`xiaozhi`（流式 + TTS 并行）、Web UI（provider 列表 + Agent 调用日志）

**结论：Agent 抽象边界清晰，新增一个后端是低摩擦的横向扩展，模式已被 claude-code/codex 两次验证。**

---

## 3. 全面分析

### 3.1 OpenClaw 是什么

- **定位**：个人 AI 助手 / 全栈 Agent 运行时（Node.js 22+，MIT），"Any OS. Any Platform"
- **本质**：它自己就是一个完整网关——Gateway（本地控制面）+ 30+ 消息渠道（含飞书）+ session 持久化 + 13000+ 社区 skills + cron/heartbeat（主动行为）+ 14 家 TTS + 多模型路由与 API key failover
- **与 haimen 的异同**：

| 维度 | haimen | OpenClaw |
|------|--------|----------|
| 定位 | 薄网关，路由思维 | 自包含运行时 |
| 消息渠道 | 飞书 / GitHub / Telegram… | 30+ 渠道 |
| 技能 | 依赖底层 Agent（claude/codex）自带 | 自带 13000+ skills |
| 主动行为 | 请求-响应模型 | heartbeat / cron 主动触发 |
| 运行时 | Rust 单二进制 | Node.js + 常驻 Gateway |

### 3.2 三种可选的集成层次

```
L1  Agent 后端（用户当前意向）        L2  能力复用                      L3  对等网关互联
haimen ──openclaw agent -m──► OpenClaw   haimen ──MCP/工具──► OpenClaw   haimen ──channel──► OpenClaw
（参考 claude/codex，子进程桥接）         （复用 skills/多模型路由）         （互为对方的渠道/Agent）
```

- **L1**：把 OpenClaw 当"又一个 agent 后端"，通过子进程调用其 CLI。
- **L2**：不把 OpenClaw 当 agent 后端，而是通过 haimen 的 MCP/工具通道按需调用 OpenClaw 的 skills / cron / 渠道能力。
- **L3**：让 haimen 作为 channel 接入 OpenClaw（或反向），网关互联。

用户诉求基于 claude-code/codex 的既有模式，指向 **L1**。下文以 L1 为主展开，并在 3.5 对照 L2/L3 的价值。

### 3.3 L1 技术可行性对比

| 维度 | claude-code | codex | openclaw |
|------|------------|-------|----------|
| 非交互调用 | `claude --print` | `codex exec` | `openclaw agent -m "<msg>" --json` |
| 流式输出 | 逐事件 JSONL | JSONL | 底层 RPC 有流式事件（assistant/tool/lifecycle），**CLI 层是否逐事件到 stdout 需实测** |
| session | `--resume <session_id>` | `resume <thread_id>` | `--session-key` / `--to` / `--session-id` |
| 事件轨迹 | thinking/tool_use/tool_result | 暂留空 | 底层有事件流，**CLI 层结构化输出需实测** |
| 守护进程依赖 | 无 | 无 | **默认走 Gateway RPC（需 `openclaw gateway run` 常驻）**；`--local` 可绕过但事件/投递能力受限 |
| 运行时 | Node（npm） | Node（npm） | Node 22+（npm） |

**可复用的部分（约 80% 工作量已在 claude/codex 中沉淀）**：
- 子进程启动：`haimen_core::process::build_command`（含 Windows npm shim）
- 三通道读流框架：session_id（oneshot）+ 文本（mpsc）+ 事件（mpsc）
- `AgentProvider` 实现 + `AgentRegistry` 注册 + `[gateway.providers.openclaw]` 配置
- 单元测试范式（JSONL 解析函数的纯函数测试）

**新增工作**：
- `src/agents/openclaw/{mod.rs, agent.rs}`，约 300~400 行
- `registry.rs` 注册一行
- `check_available`（检测 Node 与 openclaw CLI）

**3 个关键不确定点（PoC 实测结论，2026-08-07，openclaw 2026.7.1-2）**：
1. **流式**：CLI 层无逐事件流式。`openclaw agent --json` 为**结尾一次性输出**（源码 `writeRuntimeJson(buildGatewayJsonResponse(response))`）；`--local` embedded 内部虽有 `text_delta`/ACP 事件但仅走内部 runtime，不暴露 stdout。→ haimen 回落批处理，TTS 降级为"等全部说完再合成"。
2. **Gateway**：本机已常驻（LaunchAgent port 18789），`openclaw agent` 默认走 Gateway RPC；Gateway 缺失时**自动降级 embedded**。→ haimen 不管理 gateway 生命周期。
3. **JSON schema（实测）**：文本在 `result.payloads[].text`（多段 join）；顶层**无 `sessionKey`**（在 `result.meta.systemPromptReport.sessionKey`，显式传 key 时原样返回）；顶层 `runId`/`status:"ok"`。**关键发现**：不传 `--session-key` 时所有新会话共享默认 `agent:main:main` 会话 → haimen 必须**自持唯一 session key**（`agent:<id>:haimen:<unique>`）显式传入，resume 原样传回（实测同 key 同 sessionId、上下文延续）。无 thinking/tool 事件。

### 3.4 会话语义差异

- haimen 的 session_id 语义 = "继续同一个 agent 会话"（claude session_id / codex thread_id）。
- OpenClaw 的 session 有自己的 **compaction / 记忆管理**，`session-key` 格式为 `<agentId>:<channel>:<scope>:<identifier>`。
- 映射方案：用 haimen 的 session_id 直接作为 OpenClaw 的 `--session-key`（或映射到 `--to`）。**haimen 无法完全掌控 OpenClaw 内部的上下文裁剪策略**，多轮上下文可能被 OpenClaw 自行压缩。

### 3.5 战略价值评估（L1 与 L2/L3 对照）

OpenClaw 作为 agent 后端（L1）相对 claude-code/codex 的**差异化价值**：
- ✅ **多模型路由 + API key failover**：claude/codex 绑定单一厂商，OpenClaw 可在多模型间路由/轮换，降低单点失效与额度压力。
- ✅ **13000+ skills**：claude-code 的 skills 生态远不及 OpenClaw 社区规模，集成后 haimen 用户可触达更广的技能库。
- ⚠️ 这些价值其实**不必通过 L1 获得**——skills / 模型路由通过 **L2（MCP/工具调用）** 即可按需复用，且不引入双网关常驻的开销。

**重叠与张力**：
- OpenClaw 本身就是个完整网关（含飞书渠道），haimen 复用其 agent 后端，等于**一个网关驱动另一个网关**：Node 进程 + Gateway 常驻 + 自身 memory 持久化，资源双重。
- OpenClaw 的 **proactive / cron / heartbeat** 走事件驱动模型，与 haimen 的请求-响应 `chat_loop` 模型不兼容，L1 无法复用这部分能力。
- 与 design-2026-06-19 的定位判断直接冲突——若坚持 L1，需明确**当初"太重"的结论为何现在失效**（例：haimen 已验证的多后端路由需要"多模型 failover"这一能力）。

### 3.6 风险清单

| 风险 | 等级 | 说明与缓解 |
|------|------|-----------|
| 流式输出不确定 | 高 | CLI 层可能无逐事件流，TTS 体验降级；PoC 先行验证 |
| Gateway 常驻依赖 | 高 | 生命周期管理、失败恢复、资源占用；优先评估 `--local` 是否够用 |
| 版本漂移 | 中 | OpenClaw 用 `vYYYY.M.D` 高频发布，CLI/协议可能不稳定，需持续适配 + 锁定版本 |
| 会话语义差异 | 中 | 上下文压缩不受 haimen 控制；文档化预期 |
| 安全面扩大 | 中 | OpenClaw 工具具备全权限（shell/浏览器/钥匙串），作为子进程桥接时等于把这些权限引入 haimen 管道；参考 codex 沙箱经验（`--sandbox` 放开后的钥匙串问题） |
| 定位冲突 | 中 | 与"薄网关"立身之本相悖，需产品决策 |

---

## 4. 分析结论

### 4.1 技术可行性：**高**

L1 集成完全复用 claude-code/codex 已验证的"子进程桥接 + JSONL 解析 + 三通道"范式，`AgentProvider` 抽象与注册表让横向扩展成本很低。预计工作量 **1~2 人日**（不含 PoC 与联调）。

### 4.2 体验上限取决于 3 个未知数（须先 PoC）

1. CLI 是否流式输出（决定 TTS 体验）；
2. 是否必须 Gateway 常驻（决定运维模型）；
3. `--json` schema 是否稳定可解析（决定事件轨迹质量）。

### 4.3 战略可行性：**需权衡**

OpenClaw 作为 agent 后端的真实增量价值是**多模型路由/failover** 与 **13000+ skills**；但这与 haimen "薄网关" 定位存在张力，且引入双网关运行开销。若目标只是复用 skills 与模型路由，**L2（MCP/工具通道按需调用）比 L1 更轻、更契合 haimen 定位**；L1 只有在"希望 haimen 用户完整获得 OpenClaw 的单体 Agent 体验"时才有意义。

### 4.4 建议路径

```
阶段 0（决策）  明确集成目标：Agent 后端体验（L1）？还是 skills/模型路由能力（L2）？
阶段 1（PoC）   用真实 openclaw CLI 验证 3 个不确定点（流式 / Gateway 依赖 / JSON schema）
                产出：可流式？需常驻？可解析的 JSONL 样例？
阶段 2（实现）  若 PoC 通过 → agents/openclaw 模块 + 注册 + 配置 + 测试（约 300~400 行）
                若流式不可行 → 降级批处理，TTS 体验降级但功能可用
阶段 3（运维）  Gateway 生命周期管理（若走 RPC）、版本锁定、沙箱/权限策略
```

---

## 5. 实现记录（2026-08-07 落地）

采用 L1（Agent 后端）+ 批处理（CLI 无流式，已实测确认）。改动清单：

- **新增独立 crate** `crates/haimen-openclaw/`（跟随 origin/main 的 Agent 独立 crate 架构，与 `haimen-claude-code`/`haimen-codex` 一致）：`lib.rs` 导出 `OpenClawAgent`/`DEFAULT_AGENT_ID`，`agent.rs` 实现 `AgentProvider`。核心设计：
  - 子进程 `openclaw agent --json --timeout <s> --agent <id> --session-key <key> -m <msg>`
  - **haimen 自持唯一 session key**（`agent:<id>:haimen:<nanos><seq>`）显式传入——避免落到 openclaw 默认 `main` 会话导致多会话串上下文；resume 原样传回
  - 一次性 JSON 解析：文本取 `result.payloads[].text`，混入日志时 `extract_json_span` 兜底；错误文案透传
  - 13 个纯函数单测（`resolve_agent` 依赖 `GatewayConfig`，随拆分移到主 crate registry）
- **修改** `src/agents/registry.rs`（引用 `haimen_openclaw` + `resolve_openclaw_agent` + 测试断言）、`Cargo.toml`（workspace member + 依赖）、`src/config/settings.rs`（doc 注释示例）、`README.md`（配置示例）；`src/agents/mod.rs` 不再有 openclaw 模块
- **零改动**：Web API / 前端 / agent_log / chat_loop（由注册表自动驱动）

验证结果：
- `cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test --workspace` 全绿（主 crate 432 单测 + 6 集成 + haimen-openclaw 13 单测等）
- `haimen agent run --provider openclaw`：返回文本，日志记录 session key `agent:main:haimen:*`
- `haimen agent chat` 两轮对话（记住秘密数字 42 → 复述 42）：**resume 端到端生效**

已知限制（文档化）：
- 非流式：语音场景（xiaozhi）TTS 等全部回复生成完再合成；无 thinking/tool 事件轨迹
- gateway 生命周期由用户管理（缺失时 openclaw 自动降级 embedded，模型/keys 可能与常驻 Gateway 不一致）
- OpenClaw 高频发版，JSON schema 解析集中在 `agent.rs` 纯函数，便于适配

---

## 附：参考资料

- OpenClaw 仓库: https://github.com/openclaw/openclaw
- OpenClaw 文档（agents CLI）: https://docs.openclaw.ai/zh-CN/cli/agents
- OpenClaw CLI Reference (DeepWiki): https://deepwiki.com/moltbot/clawdbot/9-cli-reference
- OpenClaw Agent Commands (DeepWiki): https://deepwiki.com/openclaw/moltbot/9.4-agent-commands
- haimen Agent 抽象: `crates/haimen-core/src/provider.rs`
- haimen claude-code/codex Agent: `src/agents/claude_code/agent.rs`, `src/agents/codex/agent.rs`
- haimen 网关技术方案（定位对照）: `docs/plans/design-2026-06-19-haimen-personal-gateway.md`
