# haimen CLI 安装/升级/卸载功能分析报告

## 1. 背景与目标

haimen 是一个 AI 网关基建 CLI 工具，集成飞书消息处理。当前项目缺少以下能力：

- **安装方式**：用户无法通过标准方式安装使用（缺少 `cargo install` / 一键脚本等支持）
- **自升级**：无法通过 CLI 命令检查并升级到最新版本
- **自卸载**：无法通过 CLI 命令从系统中移除自身
- **README 文档**：缺少安装说明和完整的 CLI 命令文档

本分析针对如何参考成熟项目 [zapmyco](https://github.com/shenjingnan/zapmyco) 的实现，为 haimen 增加以上能力。

---

## 2. 参考项目分析: zapmyco

### 2.1 项目概况

zapmyco 是一个成熟的 Rust CLI 项目（v0.40.0），具有完整的 CLI 生命周期管理。

#### CLI 命令结构

```
src/commands/
├── mod.rs          ✓ 模块声明
├── completion.rs   ✓ Shell 补全生成 + 安装/移除
├── upgrade.rs      ✓ 自升级 (GitHub Releases → 下载 → 替换)
└── uninstall.rs    ✓ 自卸载 (删除配置/补全/二进制)
```

#### 发布流水线

| 阶段 | 触发 | 操作 |
|------|------|------|
| release-plz (publish.yml) | push to main | 创建 git tag → 发布到 crates.io |
| cargo-dist (release.yml) | tag push | 构建 5 平台二进制 → GitHub Release |

### 2.2 upgrade 命令实现分析

**位置**: `src/commands/upgrade.rs` (~250 行)

**执行流程**:
```
1. get_latest_version()
   → GET https://api.github.com/repos/shenjingnan/zapmyco/releases/latest
   → 解析 tag_name (去掉 v 前缀)

2. is_newer(latest, current)
   → 自制 semver 比较器 (按 . 分割后逐段比较)

3. detect_target_triple()
   → 根据 ARCH + OS 返回 6 种组合之一
   → aarch64/x86_64 × macos/linux/windows

4. 下载归档
   → haimen-{triple}.tar.xz (Unix) / .zip (Windows)
   → 流式写入临时文件 (reqwest + futures-util StreamExt)

5. extract_archive()
   → Unix: tar -xJf
   → Windows: PowerShell Expand-Archive

6. locate_binary()
   → 预期路径: dir/haimen-{triple}/haimen
   → 回退: dir/haimen

7. replace_binary()
   → Unix: copy → rename 原子替换 (0o755 权限)
   → Windows: rename old → copy new

8. update_receipt()
   → 更新 ~/.config/haimen/haimen-receipt.json

9. upgrade_completion()
   → 重新配置 shell 补全 (失败不阻塞)
```

**关键依赖**: `reqwest` (HTTP), `serde_json` (API 解析), `futures-util` (流式下载)

### 2.3 uninstall 命令实现分析

**位置**: `src/commands/uninstall.rs` (~120 行)

**执行流程**:
```
1. 检测是否为 TTY
   → 非交互式: 直接执行 (静默卸载)
   → 交互式: 询问用户确认

2. 交互式确认 (非 TTY 跳过)
   → 询问是否保留 ~/.zapmyco/ 目录 (配置+数据)
   → 询问是否确认卸载

3. remove_shell_completion()
   → 从 bash/zsh/fish 配置文件中移除补全 eval 行

4. 删除 ~/.config/zapmyco/ (安装收据目录)

5. 根据用户选择删除 ~/.zapmyco/ (配置目录)

6. 删除当前二进制文件
   → Unix: 直接删除 (运行中可删除)
   → Windows: 提示用户手动删除

7. 打印完成信息
```

### 2.4 completion 工具函数分析

**位置**: `src/commands/completion.rs` 中的工具函数

| 函数 | 说明 |
|------|------|
| `detect_shell()` | 从 `$SHELL` 环境变量解析 shell 类型 |
| `shell_config_path(shell, home)` | 返回 shell 配置文件路径 (bash: .bashrc or .bash_profile, zsh: .zshrc, fish: .config/fish/config.fish) |
| `completion_line(shell)` | 返回补全 eval 命令字符串 |
| `setup_shell_completion()` | 向配置文件中追加补全行 |
| `remove_shell_completion()` | 从配置文件中移除补全行 |

### 2.5 安装方式

| 方式 | 命令 | 适用场景 |
|------|------|----------|
| 一键脚本 | `curl -fsSL https://zapmyco.com/install.sh \| sh` | 推荐 (自动检测平台) |
| cargo install | `cargo install zapmyco` | Rust 开发者 |
| 手动下载 | GitHub Releases 页面下载压缩包 | 离线环境 |

安装脚本 (`install.sh` / `install.ps1`) 是轻量跳转脚本，从 GitHub Releases 下载并执行 cargo-dist 生成的真正安装器。

---

## 3. Haimen 现状分析

### 3.1 当前 CLI 结构

```
src/
├── main.rs       # 入口
├── lib.rs        # 库入口 (导出 cli, config, feishu, gateway, logging, web, datetime)
├── cli.rs        # CLI 命令定义 (Config, Feishu, Gateway, Serve, Completion)
├── config/       # 配置管理
├── feishu/       # 飞书集成
├── gateway/      # AI 网关
├── web/          # HTTP 服务
├── logging.rs
└── datetime.rs
```

当前命令: `config`, `feishu` (auth/chat/listen), `gateway` (status/listen), `serve`, `completion`

### 3.2 发现问题

| 问题 | 严重性 | 说明 |
|------|--------|------|
| 无 `upgrade` / `uninstall` | 高 | 用户无法管理 CLI 生命周期 |
| `reqwest` 被注释 | 中 | upgrade 需要 HTTP 客户端 |
| `release-plz.toml` 包名错误 | 中 | 引用 `ai-rust-starter` 而非 `haimen` |
| cargo-dist release.yml 不存在 | 中 | 虽然有 dist-workspace.toml 但未生成 workflow |
| 无 src/commands/ 目录 | 低 | 所有实现在 cli.rs 中可维护性差 |
| README 缺少安装说明 | 中 | 用户不知道如何安装 |

---

## 4. 实施方案

### 4.1 目录结构变更

```
src/
  commands/
    mod.rs          # 模块声明 (pub mod)
    completion.rs   # Shell 补全安装/移除工具
    upgrade.rs      # 自升级实现
    uninstall.rs    # 自卸载实现
  cli.rs            # 新增 Upgrade/Uninstall 命令
  lib.rs            # 新增 pub mod commands
```

### 4.2 依赖变更

```toml
# 取消注释 (已有但被注释)
reqwest = { version = "0.12", features = ["json", "stream"] }

# 新增 (用于流式下载的 StreamExt)
tokio-stream = "0.1"
```

注意: `futures-util` 已在依赖中，reqwest 的 `stream` feature 提供 `bytes_stream()`

### 4.3 模块设计

#### commands/completion.rs
- `setup_shell_completion()` → 检测 shell → 写 eval 行到配置文件
- `remove_shell_completion()` → 从所有 shell 配置移除 eval 行
- 工具函数: `detect_shell()`, `shell_config_path()`, `completion_line()`

#### commands/upgrade.rs
- `cmd_upgrade()` → async 主函数
- 获取最新版本 → 比较 → 下载 → 解压 → 替换 → 收据更新 → 补全重配

#### commands/uninstall.rs
- `cmd_uninstall()` → 交互/静默卸载
- 移除补全 → 删除收据 → 删除配置 → 删除二进制

### 4.4 发布流程修复

- `release-plz.toml`: 修正包名为 `haimen`
- `release.yml`: 运行 `cargo dist init` 生成 cargo-dist GitHub Actions workflow

### 4.5 安装脚本

- `docs/public/install.sh`: 跳转至 cargo-dist 生成的安装器
- `docs/public/install.ps1`: PowerShell 版本

### 4.6 README 更新

- 新增安装章节 (3 种方式)
- 新增 `upgrade` / `uninstall` 命令文档

---

## 5. 分析结论

1. **可行性**: haimen 项目结构与 zapmyco 高度相似（都是 Rust CLI、clap 框架、同一发布者），参考 zapmyco 的实现模式完全可行
2. **工作量**: 新增约 4 个源文件 (~500 行 Rust 代码) + 2 个脚本 + README 更新
3. **风险**: 无高风险——所有新功能都是独立模块，不涉及现有业务逻辑变更
4. **建议实施顺序**: 依赖配置 → completion 工具 → upgrade → uninstall → CLI 集成 → 发布流水线修复 → README 更新 → 安装脚本

---

## 6. 关键文件清单

| 文件路径 | 操作 | 说明 |
|----------|------|------|
| `src/commands/mod.rs` | 新建 | 模块声明 |
| `src/commands/completion.rs` | 新建 | Shell 补全工具函数 |
| `src/commands/upgrade.rs` | 新建 | 自升级实现 |
| `src/commands/uninstall.rs` | 新建 | 自卸载实现 |
| `Cargo.toml` | 修改 | 取消注释 reqwest + 添加 tokio-stream |
| `release-plz.toml` | 修改 | 修复包名 |
| `src/cli.rs` | 修改 | 新增 Upgrade/Uninstall 命令 |
| `src/lib.rs` | 修改 | 新增 `pub mod commands` |
| `docs/public/install.sh` | 新建 | 一键安装脚本 |
| `docs/public/install.ps1` | 新建 | PowerShell 安装脚本 |
| `README.md` | 修改 | 新增安装说明 + 命令文档 |
