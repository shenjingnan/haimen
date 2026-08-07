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
cd web-ui && pnpm lint              # Biome lint + 自动修复
cd web-ui && pnpm check             # Biome CI 严格检查
cd web-ui && pnpm format            # Biome 格式化
cd web-ui && pnpm test              # Vitest 测试
cd web-ui && pnpm test:watch        # Vitest 监听模式
cd web-ui && pnpm storybook         # Storybook 组件开发

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
├── Cargo.toml           # 项目配置和依赖
├── rust-toolchain.toml  # Rust 工具链版本
├── src/
│   ├── main.rs          # 入口文件
│   ├── lib.rs           # 库入口 + 测试工具
│   ├── cli.rs           # CLI 命令定义
│   ├── gateway/         # 抽象层 + 编排（与具体 Connector/Agent 无关）
│   │   ├── mod.rs       # listen() 动态构造 Channel + Agent
│   │   ├── model.rs     # Message 统一消息模型
│   │   ├── channel.rs   # MessageChannel trait（IM 通道）
│   │   ├── webhook.rs   # WebhookHandler trait + WebhookState
│   │   ├── provider.rs  # AgentProvider trait
│   │   ├── chat_loop.rs # 泛型编排循环
│   │   └── session.rs   # SessionManager 会话管理
│   ├── connectors/      # 外部系统连接器
│   │   ├── lark/        # 飞书/Lark IM 通道
│   │   │   ├── bridge.rs  # lark-cli 子进程桥接
│   │   │   ├── channel.rs # LarkChannel: impl MessageChannel
│   │   │   ├── types.rs/auth.rs/chat.rs/listen.rs/send.rs
│   │   │   └── mod.rs
│   │   ├── github/      # GitHub Webhook + @mention 触发
│   │   │   ├── mod.rs     # GitHubConnector: impl WebhookHandler
│   │   │   ├── handler.rs # 签名验证 + @mention 提取
│   │   │   ├── types.rs   # GitHub 事件模型
│   │   │   └── config.rs  # GitHub 配置
│   │   └── mod.rs
│   ├── agents/          # AI Agent 实现
│   │   ├── registry.rs  # AgentRegistry 注册表（内置 Agent 分发）
│   │   ├── error.rs     # Agent 错误类型
│   │   ├── mcp_client.rs# MCP 协议客户端
│   │   ├── mcp_agent.rs # McpAgent: impl AgentProvider
│   │   └── mod.rs
│   ├── config/
│   │   ├── mod.rs       # 配置模块入口
│   │   └── settings.rs  # TOML 配置管理
│   ├── web/             # axum HTTP 服务器
│   │   ├── mod.rs       # 服务器启动 + 优雅关闭
│   │   ├── static.rs    # SPA 静态文件服务
│   │   └── api/
│   │       ├── mod.rs
│   │       ├── system.rs   # /api/v1/system/info
│   │       └── webhook.rs  # Webhook HTTP handler
│   ├── logging.rs       # tracing 双层日志
│   ├── datetime.rs      # 日期时间工具
│   └── commands/        # 工具命令（completion, upgrade, uninstall）
├── crates/              # 独立 workspace crate
│   ├── haimen-core/     # 共享抽象层（Message / MessageChannel / AgentProvider）
│   ├── haimen-lark/     # Lark/飞书消息通道连接器
│   ├── haimen-xiaozhi/  # Xiaozhi 音频/WebSocket 集成
│   ├── haimen-claude-code/ # Claude Code Agent（claude --print）
│   └── haimen-codex/    # Codex CLI Agent（codex exec --json）
├── tests/               # 集成测试
├── .agents/             # 架构方案/计划
├── .github/             # CI/CD 配置
└── .githooks/           # Git hooks
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
