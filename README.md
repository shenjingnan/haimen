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

**haimen** 是一个 AI 网关基建 CLI 工具，通过 Connector 架构集成多种 IM 消息渠道（飞书/Lark、钉钉），
通过 Agent 抽象支持多种 AI 后端（Claude Code、MCP 协议等）。

## 特性

- **飞书/Lark 消息监听** — 通过 lark-cli 桥接实时接收飞书消息
- **钉钉消息监听** — 通过 dws（DingTalk CLI）桥接实时接收钉钉消息
- **统一消息模型** — 不同 IM 平台统一为 MessageChannel 抽象，网关层无感对接
- **AI 网关编排** — 消息 → Agent 处理 → 回复的完整闭环
- **会话管理** — 支持按会话隔离、超时切换、最大轮次控制
- **MCP 协议支持** — 支持 MCP 服务器作为 Agent 后端
- **TOML 配置管理** — 支持环境变量引用 ${env.VAR}
- **Web 控制台** — HTTP 服务器提供实时状态查看

## 安装

### 方式一：一键安装脚本（推荐）

```bash
# macOS / Linux
curl -fsSL https://haimen.dev/install.sh | sh

# Windows PowerShell
irm https://haimen.dev/install.ps1 | iex
```

### 方式二：cargo install

```bash
cargo install haimen
```

### 方式三：手动下载

从 [GitHub Releases](https://github.com/shenjingnan/haimen/releases) 下载对应平台的压缩包，解压后放入 PATH。

### 升级

```bash
haimen upgrade
```

### 卸载

```bash
haimen uninstall
```

## 前置依赖

haimen 通过外部 CLI 桥接 IM 平台，需要预先安装和认证：

### 飞书/Lark

```bash
npm install -g @larksuite/cli
lark-cli auth login
```

### 钉钉

```bash
npm install -g dingtalk-workspace-cli
dws auth login
```

## 快速开始

### 启动 AI 网关

```bash
haimen start
```

### 配置文件

配置文件位于 ~/.haimen/settings.toml：

```toml
debug = false
log_level = "info"

[http]
enabled = true
host = "0.0.0.0"
port = 9527

[connectors.lark]
enabled = true
lark_cli_path = "lark-cli"

[connectors.dingtalk]
enabled = false
dws_path = "dws"

[gateway]
agent = "claude-code"
session_idle_timeout_mins = 30
session_max_turns = 20
agent_timeout_secs = 300
```

## 项目结构

```
├── Cargo.toml               # 项目配置和依赖
├── src/
│   ├── main.rs              # 入口文件
│   ├── lib.rs               # 库入口
│   ├── cli.rs               # CLI 命令定义
│   ├── config/              # TOML 配置管理
│   ├── connectors/
│   │   ├── dingtalk/        # 钉钉连接器适配层
│   │   └── github/          # GitHub Webhook
│   ├── gateway/             # 网关编排核心
│   │   ├── mod.rs           # 构建 + 启动
│   │   ├── chat_loop.rs     # 泛型编排循环
│   │   ├── session.rs       # 会话管理
│   │   └── ...
│   ├── agents/              # AI Agent 实现
│   ├── web/                 # HTTP 服务器
│   ├── logging.rs
│   └── datetime.rs
├── crates/
│   ├── haimen-core/         # 核心抽象
│   ├── haimen-dingtalk/     # 钉钉 CLI 桥接
│   ├── haimen-lark/         # 飞书 CLI 桥接
│   └── haimen-xiaozhi/      # 小智语音通道
├── web-ui/                  # 前端控制台
├── tests/                   # 集成测试
└── docs/                    # 技术文档
```

## 架构

所有 IM API 通信通过外部 CLI 子进程桥接，haimen 不直接管理 IM 平台凭证。
认证和 Token 生命周期由 CLI 自身管理（lark-cli / dws）。

```
┌─────────────────────────────────────────────────────────────┐
│                    haimen CLI (Rust)                        │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ haimen-lark  │  │haimen-dingtk │  │ haimen-xiaozhi   │  │
│  │ lark-cli桥接 │  │ dws CLI桥接  │  │ WebSocket 语音   │  │
│  └──────┬───────┘  └──────┬───────┘  └──────────────────┘  │
│         │                 │                                  │
│         ▼                 ▼                                  │
│  ┌──────────────┐  ┌──────────────┐                         │
│  │   lark-cli   │  │     dws      │                         │
│  │  (子进程)    │  │  (子进程)    │                         │
│  └──────┬───────┘  └──────┬───────┘                         │
│         │                 │                                  │
│         ▼                 ▼                                  │
│  ┌──────────────┐  ┌──────────────┐                         │
│  │  飞书 OpenAPI│  │ 钉钉 OpenAPI │                         │
│  └──────────────┘  └──────────────┘                         │
└─────────────────────────────────────────────────────────────┘
```

## CLI 命令

```
haimen — AI 网关基建 CLI

COMMANDS:
  config              显示配置信息
  start [OPTIONS]     启动 AI 网关（所有启用的连接器）
    --no-browser      不自动打开浏览器
  listen              单连接器模式
  listen-echo         Echo 模式（直接回显）
  serve [OPTIONS]     仅启动 Web 控制台
  upgrade             升级
  uninstall           卸载
  completion <SHELL>  生成补全脚本
```

## 开发

```bash
cargo run -- start         # 运行
cargo test --all           # 测试
cargo fmt --check          # 格式检查
cargo clippy -- -D warnings # Lint 检查
```

## 许可

[MIT](LICENSE)
