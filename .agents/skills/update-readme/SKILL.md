---
name: update-readme
description: 根据项目当前状态更新根目录 README.md
---

# Update README Command

根据项目当前状态更新根目录 README.md。

## 执行步骤

1. 读取 `Cargo.toml` 获取项目名称、版本号、描述、依赖等信息
2. 读取 `src/` 目录下的源码，了解项目当前导出的公开 API（结构体、函数、trait 等）
3. 读取当前的 `README.md`，对比现状识别需要更新的内容
4. 更新 README.md 中的以下部分（按需）：
   - **项目描述**: 与 `Cargo.toml` 中的 description 保持一致
   - **技术栈**: Rust 版本、关键依赖（clap、tokio、serde 等）
   - **快速开始**: 使用示例
   - **项目结构**: 与实际目录结构一致
   - **可用命令**: `cargo run -- <command>` 格式
5. 运行格式检查: `cargo fmt --check`

## 更新原则

- 只更新与实际代码/配置不符的内容，不重写整个文件
- 保留用户自定义的章节和内容
- 保持文档风格与现有内容一致
- 如果用户指定了特定章节，只更新指定部分

## 注意事项

- 不要添加与项目无关的具体业务描述
- 所有命令示例使用 `cargo`
