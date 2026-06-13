# CLAUDE.md - ai-rust-starter

本文档为 Claude Code 提供项目上下文和开发规范。

## 项目概述

**ai-rust-starter** 是一个 Rust 项目快速启动模板，提供开箱即用的工程化配置和通用工具模块。

## 技术栈

| 技术           | 版本  | 用途                         |
| -------------- | ----- | ---------------------------- |
| Rust           | 1.85+ | 编程语言 / 编译 / 测试 / Lint / Format |
| clap           | 4.x   | CLI 参数解析                 |
| tokio          | 1.x   | 异步运行时                   |
| serde          | 1.x   | JSON/TOML 序列化/反序列化    |
| tracing        | 0.1   | 日志和诊断                   |

## 快速命令参考

```bash
# 开发
cargo run                           # 直接运行（无参进入帮助）
cargo run -- config                 # 显示配置
cargo run -- greet --name World     # 向用户问好
cargo run -- completion bash        # 生成 shell 补全

# 测试
cargo test                          # 运行测试
cargo test -- --test-threads=1      # 单线程测试（避免 env 竞争）

# 代码质量
cargo fmt                           # 格式化代码
cargo fmt --check                   # 格式检查
cargo clippy                        # Lint 检查
cargo clippy -- -D warnings         # 严格 Lint 检查
cargo test                          # 测试
cargo fmt --check && cargo clippy -- -D warnings && cargo test   # 完整检查

# 构建
cargo build                         # 调试构建
cargo build --release               # 发布构建

# 文档
cargo doc --open                    # 生成并打开 API 文档

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
│   ├── config/
│   │   ├── mod.rs       # 配置模块入口
│   │   └── settings.rs  # TOML 配置管理
│   ├── logging.rs       # tracing 双层日志
│   └── datetime.rs      # 日期时间工具
├── tests/               # 集成测试
├── .github/             # CI/CD 配置
└── .githooks/           # Git hooks
```

## 自定义指南

1. 修改 `Cargo.toml` 中的 `name`、`version`、`description`
2. 更新 `src/cli.rs` 中的命令名称和子命令
3. 在 `src/config/settings.rs` 中修改 `PROJECT_DIR` 常量（`.{{project_name}}`）
4. 在 `src/logging.rs` 中修改日志路径
5. 更新 `AGENTS.md` 中的项目名称和描述

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

## 模板使用

### 开始新项目

1. 克隆此仓库或 fork
2. 全局搜索替换 `ai-rust-starter` 为你的项目名
3. 搜索替换 `.ai-rust-starter` 为你的配置目录名
4. 修改 `Cargo.toml` 中的项目元信息
5. 开始编写你的业务代码
