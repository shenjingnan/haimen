# haimen

**haimen** 是一个 AI 网关基建 CLI 工具，集成飞书（Feishu/Lark），支持在终端接收和处理飞书消息。

## 特性

- **飞书消息监听** — 实时接收飞书消息并在终端展示（事件订阅模式 / 轮询模式）
- **lark-cli 桥接** — 利用飞书官方 CLI 处理认证、事件订阅等复杂逻辑
- **AI 网关预留** — 模块化架构，为未来 AI 处理管线预留扩展点
- **TOML 配置管理** — 支持环境变量引用 `${env.VAR}`
- **双层日志** — 基于 tracing 的日志系统，同时输出到文件和 stderr
- **Shell 补全** — 支持 bash / zsh / fish / powershell 自动补全

## 前置依赖

安装 [lark-cli](https://github.com/larksuite/cli)（飞书官方 CLI）：

```bash
npm install -g @larksuite/cli
lark-cli auth login    # 设备码授权登录
```

## 快速开始

```bash
# 显示帮助
cargo run

# 查看飞书认证状态
cargo run -- feishu auth status

# 登录飞书
cargo run -- feishu auth login

# 列出可访问的群聊
cargo run -- feishu chat list

# 监听飞书消息（事件订阅模式）
cargo run -- feishu listen

# 监听飞书消息（轮询模式）
cargo run -- feishu listen --mode poll --chat-id oc_xxxxx

# 显示配置
cargo run -- config

# 显示网关状态
cargo run -- gateway status

# 运行测试
cargo test
```

## CLI 命令

```
haimen — AI 网关基建 CLI

COMMANDS:
  config              显示配置信息
  feishu              飞书集成
    auth status       查看飞书认证状态
    auth login        登录飞书（设备码授权）
    chat list         列出可访问的群聊
    listen [OPTIONS]  监听飞书消息
  gateway
    status            显示网关状态
  completion <SHELL>  生成 Shell 补全脚本
```

### listen 命令

| 参数 | 说明 | 默认值 |
|------|------|--------|
| `--mode` | 监听模式: `event` 或 `poll` | `event` |
| `--chat-id` | 聊天 ID（poll 模式必填） | — |
| `--interval` | 轮询间隔（秒） | 30 |
| `--format` | 输出格式: `pretty` 或 `json` | `pretty` |

## 项目结构

```
├── Cargo.toml           # 项目配置和依赖
├── rust-toolchain.toml  # Rust 工具链版本（1.85）
├── src/
│   ├── main.rs          # 入口文件
│   ├── lib.rs           # 库入口 + 测试工具
│   ├── cli.rs           # CLI 命令定义
│   ├── config/
│   │   ├── mod.rs       # 配置模块入口
│   │   └── settings.rs  # TOML 配置管理
│   ├── feishu/
│   │   ├── mod.rs       # 飞书模块入口
│   │   ├── types.rs     # 飞书数据模型
│   │   ├── bridge.rs    # lark-cli 子进程桥接
│   │   ├── auth.rs      # 飞书认证
│   │   ├── chat.rs      # 群聊管理
│   │   └── listen.rs    # 消息监听
│   ├── gateway/
│   │   └── mod.rs       # AI 网关占位模块
│   ├── logging.rs       # tracing 双层日志
│   └── datetime.rs      # 日期时间工具
├── tests/               # 集成测试
├── .github/workflows/   # CI/CD
└── .githooks/           # Git hooks
```

## 架构

```
┌──────────────┐    subprocess     ┌──────────────┐    WebSocket     ┌─────────┐
│   haimen CLI │ ────────────────> │   lark-cli   │ ──────────────> │  飞书   │
│  (Rust)      │ <──────────────── │              │ <────────────── │  Open   │
│              │   stdout JSON     │              │    Event Bus    │  API    │
└──────────────┘                   └──────────────┘                 └─────────┘
```

所有飞书 API 通信通过 `lark-cli` 子进程桥接，haimen 不直接管理飞书凭证。

## 依赖说明

| 分类 | Crate | 用途 |
|------|-------|------|
| 核心 | clap | CLI 参数解析 |
| 核心 | tokio | 异步运行时 |
| 核心 | serde / serde_json / toml | 序列化 |
| 核心 | chrono | 日期时间处理 |
| 核心 | tracing / tracing-subscriber | 日志 |
| 核心 | thiserror / anyhow | 错误处理 |
| 核心 | futures-util | 异步流处理 |
| 外部 | lark-cli | 飞书 API 桥接（需预先安装） |

## 配置

配置文件位于 `~/.haimen/settings.toml`：

```toml
debug = false
log_level = "info"

[feishu]
lark_cli_path = "lark-cli"

[feishu.listen]
mode = "event"
interval_secs = 30

[gateway]
# enabled = false
# provider = "openai"
# model = "gpt-4"
```

## 许可

[MIT](LICENSE)
