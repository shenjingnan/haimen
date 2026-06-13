---
name: check
description: 运行完整的代码质量检查（格式化、Lint、类型检查、测试）。当用户输入 /check、/lint、/test、/typecheck 或要求代码检查、测试、类型检查、格式化时使用
---

# Check Command

运行完整的代码质量检查，包括格式化检查、Lint、测试和编译检查。

## 上下文获取

以下命令将在技能加载时自动执行，结果将注入到上下文中供分析：

- 全部检查结果: !`bash .agents/skills/check/scripts/run-checks.sh`

## 你的任务

根据上方注入的检查结果，按以下步骤处理：

### 1. 分析结果

逐项检查每个命令的输出，判断哪些检查未通过。

### 2. 修复问题

按以下优先级依次处理：

1. **格式化问题** — 运行 `cargo fmt` 自动修复
2. **Lint 问题** — 分析 `cargo clippy` 输出并修复代码中的问题
3. **测试失败** — 分析 `cargo test` 输出并修复失败的测试
4. **编译错误** — 分析 `cargo build` 输出并修复编译错误

### 3. 最终验证

修复完成后，重新运行完整检查确认全部通过：

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test -- --test-threads=1 && cargo build
```

## 配置参考

| 工具          | 配置方式       | 用途             |
| ------------- | -------------- | ---------------- |
| Cargo fmt     | `rustfmt.toml`  | 代码格式化       |
| Clippy        | `clippy.toml` / `Cargo.toml` | Lint 检查        |
| Cargo test    | `Cargo.toml`   | 测试运行         |
| Cargo check   | `Cargo.toml`   | 编译检查         |

## 代码风格

- 缩进: 2 空格
- 行宽: 100 字符
- 由 `cargo fmt` 强制执行

## 测试规范

- 测试文件: `src/*.rs` 中的 `#[cfg(test)] mod tests`
- 框架: Rust 内置 `#[test]` + `assert_eq!` / `assert!`
- 运行: `cargo test`（单线程: `cargo test -- --test-threads=1`）
