# CLAUDE.md - haimen

本文档为 Claude Code 提供项目上下文和开发规范。

## 项目概述

**haimen** 是一个 AI 网关基建 CLI 工具，通过 Connector 架构集成多种消息渠道（飞书/Lark、Telegram 等），通过 Agent 抽象支持多种 AI 后端（Claude Code、MCP 协议等）。

## 技术栈

| 技术           | 版本  | 用途                         |
| -------------- | ----- | ---------------------------- |
| Rust           | 1.85+ | 编程语言 / 编译 / 测试 / Lint / Format |
| clap           | 4.x   | CLI 参数解析                 |
| tokio          | 1.x   | 异步运行时                   |
| serde          | 1.x   | JSON/TOML 序列化/反序列化    |
| tracing        | 0.1   | 日志和诊断                   |
| futures-util   | 0.3   | 异步流处理                   |
| React          | 19    | 前端 UI 框架                 |
| Vite           | 8     | 前端构建工具                 |
| Tailwind CSS   | 4     | CSS 工具链                   |
| Ant Design     | 6     | UI 组件库                    |

## 快速命令参考

```bash
# 开发
cargo run                           # 直接运行（无参进入帮助）
cargo run -- config                 # 显示配置
cargo run -- feishu auth status     # 查看飞书认证状态
cargo run -- feishu chat list       # 列出飞书群聊
cargo run -- feishu listen          # 监听飞书消息
cargo run -- completion bash        # 生成 shell 补全

# 安装 pre-commit 钩子（新 clone 后必须执行一次）
npm install -g lefthook             # 安装 lefthook
lefthook install                    # 注册 pre-commit 钩子

# 测试
cargo test                          # 运行测试
cargo test -- --test-threads=1      # 单线程测试（避免 env 竞争）

# 代码质量
cargo fmt                           # 格式化代码
cargo fmt --check                   # 格式检查
cargo clippy                        # Lint 检查
cargo clippy -- -D warnings         # 严格 Lint 检查
cargo fmt --check && cargo clippy -- -D warnings && cargo test   # 完整检查

# 构建
cargo build                         # 调试构建
cargo build --release               # 发布构建

# 文档
cargo doc --open                    # 生成并打开 API 文档

# 前端开发（web-ui/）
cd web-ui && pnpm dev               # 启动 Vite 开发服务器（HMR）
cd web-ui && pnpm build             # 构建前端产物
cd web-ui && pnpm lint              # ESLint 检查

# 覆盖率
cargo tarpaulin                     # 生成覆盖率报告
```

## 代码风格规范

由 `cargo fmt` 和 `cargo clippy` 强制执行（Rust Edition 2024）：

- **缩进**: 2 空格
- **行宽**: 最大 100 字符

### 命名约定

| 类型      | 约定                 | 示例           |
| --------- | -------------------- | -------------- |
| 文件      | snake_case           | `my_module.rs` |
| 类/结构体 | PascalCase           | `MyStruct`     |
| 函数/变量 | snake_case           | `my_function`  |
| 常量      | SCREAMING_SNAKE_CASE | `MAX_COUNT`    |
| 类型/trait| PascalCase           | `UserConfig`   |
| 枚举      | PascalCase           | `ModelRole`    |

## 项目结构

```
├── Cargo.toml               # 项目配置和依赖
├── rust-toolchain.toml      # Rust 工具链版本
├── src/
│   ├── main.rs              # 入口文件
│   ├── lib.rs               # 库入口 + 测试工具
│   ├── cli.rs               # CLI 命令定义
│   ├── gateway/             # 抽象层 + 编排
│   │   ├── mod.rs           # build_connectors() + start_all()
│   │   ├── model.rs         # Message 统一消息模型
│   │   ├── channel.rs       # MessageChannel trait
│   │   ├── webhook.rs       # WebhookHandler trait
│   │   ├── provider.rs      # AgentProvider trait
│   │   ├── chat_loop.rs     # 泛型编排循环
│   │   └── session.rs       # SessionManager
│   ├── connectors/
│   │   ├── dingtalk/        # 钉钉适配层（桥接到 haimen-dingtalk）
│   │   │   └── mod.rs       # 重导出 DingTalkChannel
│   │   ├── github/          # GitHub Webhook
│   │   │   ├── mod.rs
│   │   │   ├── handler.rs
│   │   │   ├── types.rs
│   │   │   └── config.rs
│   │   └── mod.rs
│   ├── agents/
│   │   ├── claude_code/     # Claude Code Agent
│   │   │   ├── mod.rs
│   │   │   └── agent.rs
│   │   ├── codex/           # Codex 占位
│   │   ├── error.rs
│   │   ├── mcp_client.rs
│   │   ├── mcp_agent.rs
│   │   └── mod.rs
│   ├── config/
│   │   ├── mod.rs
│   │   └── settings.rs      # TOML 配置管理
│   ├── web/
│   │   ├── mod.rs
│   │   ├── static.rs
│   │   └── api/
│   │       ├── mod.rs
│   │       ├── system.rs
│   │       └── webhook.rs
│   ├── logging.rs
│   ├── datetime.rs
│   └── commands/             # completion, upgrade, uninstall
├── crates/
│   ├── haimen-core/          # 核心抽象（Message, MessageChannel）
│   ├── haimen-dingtalk/      # 钉钉 CLI (dws) 桥接
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── bridge.rs     # DwsBridge（子进程管理）
│   │       ├── channel.rs    # DingTalkChannel
│   │       └── types.rs      # DingTalkEvent NDJSON
│   ├── haimen-lark/          # 飞书 CLI (lark-cli) 桥接
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── bridge.rs     # LarkCliBridge
│   │       ├── channel.rs    # LarkChannel
│   │       └── types.rs      # FeishuEvent
│   └── haimen-xiaozhi/       # 小智 AI 语音通道
├── web-ui/                   # 前端 Web 控制台
├── tests/                    # 集成测试
├── .agents/
├── .github/
└── .githooks/
```

## Git 工作流

### 分支命名

- `feature/xxx` - 新功能
- `fix/xxx` - Bug 修复
- `docs/xxx` - 文档更新
- `refactor/xxx` - 重构

### Commit 规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]
```

**类型**:

- `feat` - 新功能
- `fix` - Bug 修复
- `docs` - 文档更新
- `style` - 代码格式
- `refactor` - 重构
- `perf` - 性能优化
- `test` - 测试相关
- `chore` - 构建/工具
