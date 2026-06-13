---
name: security-audit
description: 安全审计技能，用于检查和修复依赖安全问题
---

# 安全审计

## 上下文获取

以下命令将在技能加载时自动执行，结果将注入到上下文中供分析：

- 安全审计结果: !`bash .agents/skills/security-audit/scripts/run-security-audit.sh`

## 你的任务

根据上方注入的审计结果，按以下步骤处理：

### 1. 分析结果

逐项检查每个检查项的输出，判断是否存在安全问题：

1. **cargo audit** — 检查 Cargo.lock 中已知漏洞的依赖
2. **cargo deny check** — 检查许可协议合规性和依赖安全性

### 2. 修复漏洞

如果发现漏洞或过时依赖，对每个问题包依次处理：

1. **尝试自动修复**：
   ```bash
   cargo update
   ```
   或手动更新到最新兼容版本：
   ```bash
   cargo update -p <包名>
   ```

2. **验证**：运行 `cargo check && cargo test -- --test-threads=1`

3. **验证失败则回滚**：
   ```bash
   git checkout -- Cargo.toml Cargo.lock
   ```

### 3. 最终验证

修复完成后，确认所有检查通过：

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test -- --test-threads=1
```

## 前置条件

```bash
cargo install cargo-audit       # 漏洞扫描
cargo install cargo-deny         # 许可/依赖检查
```

## 原则

- 优先通过 `cargo update` 自动修复，无法修复时手动指定版本
- 升级后必须通过测试验证
- 如果某个漏洞无法通过升级修复，考虑在 `deny.toml` 中配置豁免（需确认风险可接受）
