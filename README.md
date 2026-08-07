# 海门 - Haimen

<p align="center">
  <img src="docs/public/logo.svg" alt="haimen logo" width="300" />
</p>

<p align="center">
  <a href="https://github.com/shenjingnan/haimen/actions/workflows/ci.yml"><img src="https://github.com/shenjingnan/haimen/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://codecov.io/gh/shenjingnan/haimen"><img src="https://img.shields.io/codecov/c/github/shenjingnan/haimen" alt="Codecov"></a>
  <a href="https://crates.io/crates/haimen"><img src="https://img.shields.io/crates/v/haimen.svg?color=brightgreen" alt="crates.io"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-brightgreen.svg" alt="License: MIT"></a>
</p>

**haimen** 是一个 AI 网关基建 CLI 工具，支持多种消息渠道（飞书/Lark、钉钉、GitHub Webhook）和多种 AI 后端（Claude Code、MCP 等）。

## 安装

### 方式一：一键安装脚本（推荐）

从 GitHub Release 下载并运行 cargo-dist 安装器，脚本会自动检测平台、下载对应二进制归档、验证完整性，并配置 PATH。

**macOS / Linux**

```bash
curl -fsSL https://github.com/shenjingnan/haimen/releases/latest/download/haimen-installer.sh | sh
```

**Windows (PowerShell)**

在 PowerShell 中（推荐）：

```powershell
irm https://github.com/shenjingnan/haimen/releases/latest/download/haimen-installer.ps1 | iex
```

在 cmd.exe 中：

```powershell
powershell -c "irm https://github.com/shenjingnan/haimen/releases/latest/download/haimen-installer.ps1 | iex"
```

> Windows 二进制依赖 **Microsoft Visual C++ Redistributable**（大多数系统已内置），若运行时报缺少 `vcruntime140.dll`，请先安装 [VC++ 运行库](https://aka.ms/vs/17/release/vc_redist.x64.exe)。

**国内用户（中国大陆）**：如果 GitHub 访问缓慢，可使用 Gitee 镜像安装

**macOS / Linux**

```bash
curl -fsSL https://gitee.com/shenjingnan/haimen/raw/main/docs/public/install-gitee.sh | sh
```

**Windows (PowerShell)**

在 PowerShell 中（推荐）：

```powershell
irm https://gitee.com/shenjingnan/haimen/raw/main/docs/public/install-gitee.ps1 | iex
```

在 cmd.exe 中：

```powershell
powershell -c "irm https://gitee.com/shenjingnan/haimen/raw/main/docs/public/install-gitee.ps1 | iex"
```

### 方式二：cargo install

```bash
cargo install haimen
```

### 方式三：手动下载

从 [GitHub Releases](https://github.com/shenjingnan/haimen/releases) 下载对应平台的压缩包，解压后放入 `PATH`。

| 平台    | 架构          | 文件名                                    |
| ------- | ------------- | ----------------------------------------- |
| macOS   | Intel         | `haimen-x86_64-apple-darwin.tar.xz`       |
| macOS   | Apple Silicon | `haimen-aarch64-apple-darwin.tar.xz`      |
| Linux   | x86_64        | `haimen-x86_64-unknown-linux-gnu.tar.xz`  |
| Linux   | ARM64         | `haimen-aarch64-unknown-linux-gnu.tar.xz` |
| Windows | x86_64        | `haimen-x86_64-pc-windows-msvc.zip`       |

> Windows on ARM（aarch64）暂不支持，`haimen upgrade` 会明确报错。

### 升级

```bash
haimen upgrade
```

### 卸载

```bash
haimen uninstall
```

## 特性

- **多消息渠道** — 集成飞书/Lark、钉钉、GitHub Webhook，统一消息模型
- **多 AI 后端** — 支持 Claude Code、MCP 协议、OpenAI-compatible API
- **小智 AI 硬件** — 原生支持 小智 AI 聊天硬件（WebSocket 音频流协议）
- **Web 管理控制台** — 内置 HTTP 服务器 + React SPA，管理配置、Agent 和语音
- **TOML 配置管理** — 支持多服务商配置和环境变量引用 `${env.VAR}`
- **双层日志** — 基于 tracing 的日志系统，同时输出到文件和 stderr
- **Shell 补全** — 支持 bash / zsh / fish / powershell / elvish 自动补全

## 支持列表

### AI Agent

| 名称        | 类型          | 说明                 |
| ----------- | ------------- | -------------------- |
| Claude Code | AgentProvider | 通过 Claude CLI 交互 |
| Codex       | AgentProvider | Codex CLI 集成        |

### 消息渠道

| 名称         | 类型           | 连接方式                |
| ------------ | -------------- | ----------------------- |
| 飞书 / Lark  | MessageChannel | lark-cli 子进程桥接     |
| 钉钉         | MessageChannel | 直连 Web API            |
| GitHub       | WebhookHandler | Webhook + @mention 触发 |
| 小智 AI 硬件 | WebSocket      | 音频流协议直连          |

## 快速开始

```bash
# 启动所有启用的连接器和 Agent
haimen start
```

## CLI 命令

```
haimen — AI 网关基建 CLI

USAGE:
  haimen [COMMAND]

COMMANDS:
  config              显示配置信息
  start               启动所有启用的连接器和 Agent
    --echo            回声模式（消息原样返回，不经过 Agent）
    --open-browser    启动成功后自动打开浏览器打开 Web 控制台
    --log-level       终端日志级别（默认关闭终端日志，仅记录到文件）
  agent               AI Agent 调试
    run               单次运行 Agent
      <PROMPT>        发送给 Agent 的消息（位置参数）
      --provider      Agent 提供者（claude-code / codex / openclaw / hermes）
    chat              交互式 Agent 会话（支持 resume）
      --provider      Agent 提供者（claude-code / codex / openclaw / hermes）
    log               查看 Agent 调用日志
      --limit         显示条数（默认 20）
      --day           只显示指定日期 (YYYY-MM-DD)
      --source        只显示指定来源（网关 / 语音 / CLI 调试）
      --chat          只显示指定会话 chat_id
      --json          以 JSON 数组输出
  serve               启动 HTTP Web 服务器（xiaozhi WebSocket + GitHub Webhook）
    --host            监听地址（默认 0.0.0.0）
    --port            监听端口（默认 9527）
    --no-browser      不自动打开浏览器
    --xiaozhi-echo    Echo 模式
    --xiaozhi-llm     ASR → AI → TTS 模式（默认）
    --xiaozhi-asr-tts ASR-TTS 回声模式
    --xiaozhi-llm-provider  LLM 提供者
    --xiaozhi-tts-text      TTS 测试文本
    --xiaozhi-tts-voice     TTS 音色
  completion <SHELL>  生成 Shell 补全脚本（bash / zsh / fish / powershell / elvish）
  upgrade             升级 haimen 到最新版本
  uninstall           卸载 haimen
```

## 配置

配置文件位于 `~/.haimen/settings.toml`：

```toml
debug = false
log_level = "info"

[http]
enabled = true
host = "0.0.0.0"
port = 9527
auto_open_browser = true

# 连接器配置
[connectors.lark]
enabled = true
lark_cli_path = "lark-cli"

[connectors.dingtalk]
enabled = true
client_id = "xxx"
client_secret = "${env.DINGTALK_CLIENT_SECRET}"

# AI 网关配置（支持多服务商）
[gateway]
active_provider = "claude-code"

[gateway.providers.claude-code]
# CLI 工具无需额外凭证
# 可选：claude CLI 可执行文件路径（留空按 PATH 查找 "claude"）
# cli_path = "/opt/claude/bin/claude"

[gateway.providers.codex]
# CLI 工具无需额外凭证
# 可选：codex CLI 可执行文件路径（留空按 PATH 查找 "codex"）
# cli_path = "/opt/codex/bin/codex"

[gateway.providers.openclaw]
# CLI 工具无需额外凭证；建议 openclaw gateway 常驻（缺失时自动降级 embedded）
# 可选：openclaw agent id（默认 "main"，OpenClaw 保留 agent）
# agent = "ops"
# 可选：openclaw CLI 可执行文件路径（留空按 PATH 查找 "openclaw"）
# cli_path = "/opt/openclaw/bin/openclaw"

[gateway.providers.hermes]
# CLI 工具无需额外凭证；Hermes Agent 经 `hermes chat -q -Q` 子进程调用
# 可选：hermes CLI 可执行文件路径（留空按 PATH 查找 "hermes"）
# cli_path = "/opt/hermes/bin/hermes"

[gateway.providers.openai]
api_key = "${env.OPENAI_API_KEY}"
model = "gpt-4o"

# MCP 服务器（haimen 作为 MCP 客户端）
[gateway.mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path"]

# ASR 配置（小智硬件，支持多服务商）
[asr]
active_provider = "doubao"

[asr.providers.doubao]
api_key = "${env.DOUBAO_API_KEY}"

[asr.providers.qwen]
api_key = "${env.QWEN_API_KEY}"

# TTS 配置（小智硬件，支持多服务商）
[tts]
active_provider = "doubao"

[tts.providers.doubao]
api_key = "${env.DOUBAO_API_KEY}"
voice = "zh_female_xiaohe_uranus_bigtts"

[tts.providers.openai]
api_key = "${env.OPENAI_API_KEY}"
voice = "alloy"
model = "tts-1"

[tts.providers.edge]
voice = "zh-CN-XiaoxiaoNeural"
```

## 许可

[MIT](LICENSE)
