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

从 GitHub Release 下载并运行 cargo-dist 安装器：

```bash
# macOS / Linux
curl -fsSL https://github.com/shenjingnan/haimen/releases/latest/download/installer.sh | sh

# Windows PowerShell
$r = Invoke-WebRequest https://github.com/shenjingnan/haimen/releases/latest/download/installer.ps1; iex $r.Content
```

### 方式二：cargo install

```bash
cargo install haimen
```

### 方式三：手动下载

从 [GitHub Releases](https://github.com/shenjingnan/haimen/releases) 下载对应平台的压缩包，解压后放入 `PATH`。

| 平台 | 架构 | 文件名 |
|------|------|--------|
| macOS | Intel | `haimen-x86_64-apple-darwin.tar.xz` |
| macOS | Apple Silicon | `haimen-aarch64-apple-darwin.tar.xz` |
| Linux | x86_64 | `haimen-x86_64-unknown-linux-gnu.tar.xz` |
| Linux | ARM64 | `haimen-aarch64-unknown-linux-gnu.tar.xz` |

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

| 名称 | 类型 | 说明 |
|------|------|------|
| Claude Code | AgentProvider | 通过 Claude CLI 交互 |
| Codex | AgentProvider | OpenAI Codex 集成 |

### 消息渠道

| 名称 | 类型 | 连接方式 |
|------|------|----------|
| 飞书 / Lark | MessageChannel | lark-cli 子进程桥接 |
| 钉钉 | MessageChannel | 直连 Web API |
| GitHub | WebhookHandler | Webhook + @mention 触发 |
| 小智 AI 硬件 | WebSocket | 音频流协议直连 |

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
    --no-browser      不自动打开浏览器
  agent               AI Agent 调试
    run               单轮对话
      --provider      Agent 提供者（claude-code / codex）
      --prompt        对话提示词
    chat              交互式对话
      --provider      Agent 提供者（claude-code / codex）
  serve               启动 HTTP 服务器
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

[web]
host = "0.0.0.0"
port = 9527

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

[gateway.providers.codex]
# CLI 工具无需额外凭证

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
app_key = "${env.DOUBAO_APP_KEY}"
access_key = "${env.DOUBAO_ACCESS_KEY}"

[asr.providers.qwen]
api_key = "${env.QWEN_API_KEY}"

# TTS 配置（小智硬件，支持多服务商）
[tts]
active_provider = "doubao"

[tts.providers.doubao]
app_key = "${env.DOUBAO_APP_KEY}"
access_token = "${env.DOUBAO_ACCESS_TOKEN}"

[tts.providers.openai]
api_key = "${env.OPENAI_API_KEY}"
voice = "alloy"
model = "tts-1"

[tts.providers.edge]
voice = "zh-CN-XiaoxiaoNeural"
```

## 许可

[MIT](LICENSE)
