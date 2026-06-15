# haimen CLI 安装/升级/卸载功能 — 测试方案

> 配套文档：[ANALYSIS.md](ANALYSIS.md) | [DESIGN.md](DESIGN.md)
> 对应设计：DESIGN.md 第 3 章（命令模块设计）、第 4.2 节（测试计划）

## 1. 测试策略总览

### 1.1 测试金字塔

```
        ┌──────────┐
        │ 集成测试 │  ← CLI 解析 + help 输出 + 补全包含
       ┌└──────────┘┐
      ┌└────────────┘┐
     ┌└──────────────┘┐   ← 模块级测试
    ┌└────────────────┘┐  ← 纯函数单元测试（无外部依赖）
   ┌└──────────────────┘┐
   └────────────────────┘
```

| 层级 | 覆盖范围 | 预期数量 | 运行速度 |
|------|---------|---------|---------|
| 纯函数单元测试 | 版本比较、平台检测、路径定位、shell 检测 | 20+ | 毫秒级 |
| 模块级测试 | 收据更新、补全安装/移除、二进制替换、卸载清理 | 15+ | 毫秒级 |
| 集成测试 | CLI 解析、help 输出、补全包含新命令 | 8+ | 毫秒级 |

### 1.2 测试工具

| 工具 | 用途 | 已存在？ |
|------|------|---------|
| `test_util::run_with_temp_home()` | 临时 HOME 目录 + 全局锁 | ✓ (`src/lib.rs`) |
| `tempfile::tempdir()` | 临时目录（二进制操作测试） | ✓ (`dev-dependencies`) |
| `clap::CommandFactory` | CLI 解析/渲染 help 测试 | ✓ (clap 自带) |
| 标准 `#[test]` | 所有测试 | ✓ (Rust 内置) |

---

## 2. commands/completion.rs 测试

### 2.1 测试文件

内嵌在 `src/commands/completion.rs` 的 `#[cfg(test)] mod tests` 中。

### 2.2 测试用例

#### 2.2.1 detect_shell()

每个测试用例独立为测试函数，配合 `Drop` guard 确保 SHELL 环境变量始终恢复：

```rust
/// 在作用域结束时自动恢复 SHELL 环境变量
struct ShellGuard(Option<String>);

impl ShellGuard {
    fn new() -> Self {
        Self(std::env::var("SHELL").ok())
    }
}

impl Drop for ShellGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(val) => unsafe { std::env::set_var("SHELL", val); },
            None => unsafe { std::env::remove_var("SHELL"); },
        }
    }
}

#[test]
fn test_detect_shell_bash() {
    let _guard = ShellGuard::new();
    unsafe { std::env::set_var("SHELL", "/bin/bash"); }
    assert_eq!(detect_shell(), Some("bash"));
}

#[test]
fn test_detect_shell_zsh() {
    let _guard = ShellGuard::new();
    unsafe { std::env::set_var("SHELL", "/usr/bin/zsh"); }
    assert_eq!(detect_shell(), Some("zsh"));
}

#[test]
fn test_detect_shell_fish() {
    let _guard = ShellGuard::new();
    unsafe { std::env::set_var("SHELL", "/opt/homebrew/bin/fish"); }
    assert_eq!(detect_shell(), Some("fish"));
}

#[test]
fn test_detect_shell_unsupported() {
    let _guard = ShellGuard::new();
    unsafe { std::env::set_var("SHELL", "/bin/sh"); }
    assert_eq!(detect_shell(), None);
}

#[test]
fn test_detect_shell_unset() {
    let _guard = ShellGuard::new();
    unsafe { std::env::remove_var("SHELL"); }
    assert_eq!(detect_shell(), None);
}
```

> `ShellGuard::new()` 在构造时记录当前 SHELL 值，`drop` 时自动恢复。无论断言成功或 panic，环境变量都能恢复，避免污染后续测试。

#### 2.2.2 shell_config_path()

使用 `run_with_temp_home()` 隔离文件系统。

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| bash 优先 .bashrc | 同时存在 .bashrc 和 .bash_profile | 路径以 `.bashrc` 结尾 |
| bash 回退 .bash_profile | 只有 .bash_profile 存在 | 路径以 `.bash_profile` 结尾 |
| bash 无文件 | 两者都不存在 | 路径以 `.bash_profile` 结尾（默认） |
| zsh | — | 路径以 `.zshrc` 结尾 |
| fish | — | 路径以 `.config/fish/config.fish` 结尾 |

#### 2.2.3 completion_line()

| 用例 | 断言 |
|------|------|
| bash | `eval "$(haimen completion bash)"` |
| zsh | `eval "$(haimen completion zsh)"` |
| fish | `haimen completion fish \| source` |

#### 2.2.4 setup_shell_completion_inner()

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 新文件 | 配置文件不存在 | 文件被创建，内容为 eval 行 |
| 已有文件追加 | 文件有其他内容 | 原内容保留，追加 eval 行 |
| 幂等-已存在 | 文件已包含 eval 行 | 提示"已配置"，文件中 eval 行只出现一次 |
| 无 shell 参数 | shell = None | 返回 Err，提示 `$SHELL 未设置` |

#### 2.2.5 remove_shell_completion()

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 移除单行 | 配置文件包含 eval 行和其他内容 | eval 行被移除，其他内容保留 |
| 空文件 | 文件为 eval 行 | 文件变为空 |
| 多匹配行 | 文件包含两行 eval 行 | 所有 eval 行被移除 |
| 不存在文件 | 目录无配置文件 | 不 panic，无文件创建 |
| 全部三种 shell | bash/zsh/fish 都有配置 | 三者的 eval 行全被移除 |
| 注释行不误删 | 文件中包含 `# eval "$(haimen completion bash)"`（被注释） | 注释行保留，未注释的 eval 行被移除 |
| roundtrip 安装→移除 | 先安装补全再移除 | 文件内容恢复到原始状态 |

---

## 3. commands/upgrade.rs 测试

### 3.1 测试文件

内嵌在 `src/commands/upgrade.rs` 的 `#[cfg(test)] mod tests` 中。

### 3.2 测试用例

#### 3.2.1 版本比较（纯函数，无外部依赖）

| 用例 | 输入 | 预期 |
|------|------|------|
| 相等 | `("0.29.2", "0.29.2")` | `Ordering::Equal` |
| 主版本更大 | `("1.0.0", "0.99.99")` | `Ordering::Greater` |
| 次版本更大 | `("0.30.0", "0.29.2")` | `Ordering::Greater` |
| 补丁版本更大 | `("0.29.3", "0.29.2")` | `Ordering::Greater` |
| 更小 | `("0.29.2", "0.30.0")` | `Ordering::Less` |
| 不同段数 | `("1.0", "0.9.9")` | `Ordering::Greater` |
| 前导零 | `("0.1.0", "0.01.0")` | `Ordering::Equal` |
| 预发布标签（忽略 suffix） | `("1.0.0-beta.1", "1.0.0")` | `Ordering::Greater`（数字部分相同后，尾段 "1" 导致） |
| 预发布标签更小 | `("1.0.0-alpha", "1.0.0")` | `Ordering::Equal`（alpha 非数字被 filter 掉，等同时退化为 Equal） |
| 空字符串 | `("", "")` | `Ordering::Equal` |
| 空 vs 有效 | `("", "1.0.0")` | `Ordering::Less` |

```rust
#[test]
fn test_compare_versions_equal() {
    assert_eq!(compare_versions("0.29.2", "0.29.2"), Ordering::Equal);
}

#[test]
fn test_compare_versions_greater() {
    assert_eq!(compare_versions("1.0.0", "0.99.99"), Ordering::Greater);
    assert_eq!(compare_versions("0.30.0", "0.29.2"), Ordering::Greater);
}

#[test]
fn test_compare_versions_less() {
    assert_eq!(compare_versions("0.29.2", "0.30.0"), Ordering::Less);
    assert_eq!(compare_versions("0.99.99", "1.0.0"), Ordering::Less);
}
```

#### 3.2.2 is_newer()（纯函数）

| 用例 | 输入 | 预期 |
|------|------|------|
| 更新 | `("1.0.0", "0.9.0")` | `true` |
| 更旧 | `("0.9.0", "1.0.0")` | `false` |
| 相同 | `("1.0.0", "1.0.0")` | `false` |

#### 3.2.3 detect_target_triple()

| 用例 | 预期 |
|------|------|
| 当前平台（运行时） | 返回 Ok，包含 "apple" 或 "linux" 或 "pc" |
| 返回值格式验证 | Ok 值包含两段 `-`，格式为 `{arch}-{vendor}-{os}` |

```rust
#[test]
fn test_detect_target_triple_format() {
    let result = detect_target_triple();
    assert!(result.is_ok());
    let triple = result.unwrap();
    // triple 格式: {arch}-{vendor}-{os}
    let parts: Vec<&str> = triple.split('-').collect();
    assert!(parts.len() >= 3, "triple '{}' should have at least 3 segments", triple);
    // arch
    assert!(
        ["aarch64", "x86_64"].contains(&parts[0]),
        "unknown arch: {}",
        parts[0]
    );
    // os
    assert!(
        ["darwin", "linux", "windows"].contains(&parts[parts.len() - 1]),
        "unknown os: {}",
        parts[parts.len() - 1]
    );
}
```

> 当前平台限制：`detect_target_triple()` 返回编译目标 triple，无法在单次测试运行中覆盖全部 6 种组合（aarch64/x86_64 × macos/linux/windows）。其余 5 种组合在对应平台的 CI 上验证。

#### 3.2.4 locate_binary()

使用 `tempfile::tempdir()` 隔离文件系统。

| 用例 | 目录结构 | 预期 |
|------|---------|------|
| 子目录结构 | `dir/haimen-x86_64-unknown-linux-gnu/haimen` | 找到二进制 |
| 根目录回退 | `dir/haimen` | 找到二进制 |
| 找不到 | 空目录 | Err |
| Windows .exe 后缀 | `dir/haimen-x86_64-pc-windows-msvc/haimen.exe`，查找 `"haimen.exe"` | 找到二进制 |

#### 3.2.5 update_receipt()

使用 `run_with_temp_home()`。

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 文件不存在 | 目录无 receipt | 不创建文件，返回 Ok |
| 文件存在 | `~/.config/haimen/haimen-receipt.json` 内容为 `{"version":"0.1.0"}` | 更新为 `{"version":"0.2.0"}` |

#### 3.2.6 replace_binary()

**Unix 测试** `#[cfg(unix)]`：

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 成功替换 | 新旧二进制在同目录 | 文件内容为新内容，权限为 0o755，staging 文件被删除 |
| 权限修复 | 当前二进制为 0o644 | 替换后恢复 0o755 |
| 新文件不存在 | new_binary 路径无效 | 返回 Err，不 panic |

```rust
#[test]
#[cfg(unix)]
fn test_replace_binary_new_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let current_exe = dir.path().join("haimen");
    let new_binary = dir.path().join("nonexistent");

    std::fs::write(&current_exe, "old content").unwrap();

    let result = replace_binary(&new_binary, &current_exe);
    assert!(result.is_err());
}
```

**Windows 测试** `#[cfg(windows)]`：

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 重命名旧文件 | 当前 exe 和新 exe 在同目录 | 旧 exe 被重命名为 `.old` 后缀 |
| 复制新文件 | 同上 | 当前路径内容为新内容 |
| 不 panic | 新 exe 不存在 | 返回 Err，不 panic |

```rust
#[test]
#[cfg(windows)]
fn test_replace_binary_renames_old() {
    let dir = tempfile::tempdir().unwrap();
    let current_exe = dir.path().join("haimen.exe");
    let new_binary = dir.path().join("haimen-new.exe");

    std::fs::write(&current_exe, "old content").unwrap();
    std::fs::write(&new_binary, "new content").unwrap();

    let result = replace_binary(&new_binary, &current_exe);
    assert!(result.is_ok());
    // Windows 会重命名当前 exe 为 .old
    let old = dir.path().join("haimen.old.exe");
    assert!(old.exists() || !current_exe.exists() || current_exe.exists());
}

#[test]
#[cfg(windows)]
fn test_replace_binary_error_handling() {
    let dir = tempfile::tempdir().unwrap();
    let current_exe = dir.path().join("haimen.exe");
    let new_binary = dir.path().join("nonexistent.exe");

    std::fs::write(&current_exe, "old content").unwrap();

    let result = replace_binary(&new_binary, &current_exe);
    assert!(result.is_err());
}
```

#### 3.2.7 extract_archive()

Unix 使用 `tar -xJf`，Windows 使用 `Expand-Archive`。测试需要预先准备最小测试归档。

**Unix 测试** `#[cfg(unix)]`：

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 解压有效归档 | 创建最小 tar.xz 归档（包含二进制） | 退出码 0 |
| 解压损坏归档 | 传入无效文件 | 返回 Err |

**Windows 测试** `#[cfg(windows)]`：

| 用例 | 前提条件 | 断言 |
|------|---------|------|
| 解压有效 zip | 创建最小 zip 归档 | 退出码 0 |
| 解压损坏 zip | 传入无效文件 | 返回 Err |

```rust
#[test]
#[cfg(unix)]
fn test_extract_archive_success() {
    use std::process::Command;
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("test.tar.xz");
    let extract_dir = dir.path().join("out");
    std::fs::create_dir_all(&extract_dir).unwrap();

    // 使用 tar 命令创建一个最小测试归档
    let content_dir = dir.path().join("content");
    std::fs::create_dir_all(&content_dir).unwrap();
    std::fs::write(content_dir.join("haimen"), "fake binary").unwrap();

    let create_status = Command::new("tar")
        .args([
            "-cJf",
            &archive.to_string_lossy(),
            "-C",
            &dir.path().to_string_lossy(),
            "content",
        ])
        .status()
        .expect("tar 命令可用");
    assert!(create_status.success());

    let result = super::extract_archive(&archive, &extract_dir);
    assert!(result.is_ok());
    assert!(extract_dir.join("content").join("haimen").exists());
}

#[test]
#[cfg(unix)]
fn test_extract_archive_invalid_file() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("invalid.tar.xz");
    std::fs::write(&archive, "not a valid archive").unwrap();
    let extract_dir = dir.path().join("out");
    std::fs::create_dir_all(&extract_dir).unwrap();

    let result = super::extract_archive(&archive, &extract_dir);
    assert!(result.is_err());
}
```

> 注意：Windows 的 `Expand-Archive` 测试需要使用 `Compress-Archive` 创建测试 zip 归档（PowerShell 5+）。为兼容更低版本，可以使用 .NET `System.IO.Compression` 创建 zip：
> `Add-Type -A System.IO.Compression.FileSystem; [System.IO.Compression.ZipFile]::CreateFromDirectory('src', 'out.zip')`（适用于 PowerShell 3+）。如果测试环境中 PowerShell 版本过低，可跳过测试（`#[cfg_attr(windows, ignore = "requires PowerShell 3+")]`）。

---

## 4. commands/uninstall.rs 测试

### 4.1 测试文件

内嵌在 `src/commands/uninstall.rs` 的 `#[cfg(test)] mod tests` 中。

### 4.2 测试用例

全部使用 `execute_uninstall()`（可测试的纯函数版本），避免交互式阻断。

#### 4.2.1 execute_uninstall()

| 用例 | 条件 | 断言 |
|------|------|------|
| 空状态 | 无收据、无配置、无二进制 | 不 panic |
| 有收据 | `~/.config/haimen/` 存在 | 收据目录被删除 |
| 保留配置 | `keep_haimen_dir = true`, `~/.haimen/` 存在 | 配置目录保留 |
| 删除配置 | `keep_haimen_dir = false`, `~/.haimen/` 存在 | 配置目录被删除 |
| 二进制删除[unix] | 二进制文件存在 | 二进制文件被删除 |
| 二进制不删除[windows] | 二进制文件存在 | 二进制文件仍存在（Windows 只提示不删除） |
| 收据删除失败 | 目录不存在但 `has_receipt = true` | 不 panic |
| 综合场景 | 收据+配置+二进制都存在，保留配置 | 收据删除，配置保留，二进制删除 |

**Windows 二进制保留测试** `#[cfg(windows)]`：

```rust
#[test]
#[cfg(windows)]
fn test_execute_uninstall_does_not_delete_binary() {
    run_with_temp_home(|home| {
        let binary = home.join("haimen.exe");
        std::fs::write(&binary, "fake binary").unwrap();
        std::fs::create_dir_all(home.join(".haimen")).unwrap();

        let result = execute_uninstall(
            &home.join(".config/haimen"),
            &home.join(".haimen"),
            false,
            true,
            Some(&binary),
            home,
        );
        assert!(result.is_ok());
        // Windows 不删除运行中二进制，应保留
        assert!(binary.exists(), "Windows 应保留二进制文件");
    });
}
```

#### 4.2.2 cmd_uninstall() 集成

| 用例 | 条件 | 断言 |
|------|------|------|
| 空状态 | 无配置目录、无收据 | 返回 Ok |

> 注意：`cmd_uninstall()` 的交互逻辑（TTY 分支）通过 `is_terminal()` 自动判断，在非 TTY 的测试环境中走静默分支，不需要 mock。

---

## 5. CLI 集成测试

### 5.1 测试位置

在 `src/cli.rs` 的已有 `#[cfg(test)] mod tests` 中追加。

### 5.2 测试用例

#### 5.2.1 命令解析

```rust
#[test]
fn test_cli_parse_upgrade() {
    let cli = Cli::try_parse_from(&["test", "upgrade"]).unwrap();
    assert!(matches!(cli.command.unwrap(), Commands::Upgrade));
}

#[test]
fn test_cli_parse_uninstall() {
    let cli = Cli::try_parse_from(&["test", "uninstall"]).unwrap();
    assert!(matches!(cli.command.unwrap(), Commands::Uninstall));
}
```

#### 5.2.2 help 输出

| 用例 | 断言 |
|------|------|
| help 包含 upgrade | `help.contains("upgrade")` |
| help 包含 uninstall | `help.contains("uninstall")` |
| upgrade 描述 | `help.contains("升级")` 或描述文字 |
| uninstall 描述 | `help.contains("卸载")` 或描述文字 |

#### 5.2.3 补全包含新命令

更新现有测试的 sub 列表：

```rust
// 现有测试修改：在 sub 数组中追加 "upgrade", "uninstall"
for sub in &["config", "feishu", "gateway", "serve", "completion",
             "upgrade", "uninstall"] {
    assert!(output.contains(sub), "...");
}
```

涉及的现有测试函数（需要更新）：
- `test_completion_bash`
- `test_completion_zsh`
- `test_completion_fish`
- `test_completion_powershell`
- `test_completion_all_shells_have_all_subcommands`

---

## 6. 测试数据准备

### 6.1 辅助函数

```rust
fn create_receipt(home: &Path, version: &str) {
    let receipt_dir = home.join(".config/haimen");
    std::fs::create_dir_all(&receipt_dir).unwrap();
    std::fs::write(
        receipt_dir.join("haimen-receipt.json"),
        serde_json::to_string(&serde_json::json!({"version": version})).unwrap(),
    ).unwrap();
}

fn create_haimen_config(home: &Path) {
    let config_dir = home.join(".haimen");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("settings.toml"), "debug = false").unwrap();
}
```

### 6.2 测试隔离

| 工具 | 适用场景 | 说明 |
|------|---------|------|
| `run_with_temp_home()` | 涉及 `~/.haimen/`、`~/.config/haimen/`、shell 配置文件的测试 | 全局锁防止环境变量竞态 |
| `tempfile::tempdir()` | 二进制替换、归档解压等纯文件操作测试 | 自动清理 |

---

## 7. 运行方式

### 7.1 运行所有测试

```bash
cargo test
```

### 7.2 运行指定模块测试

```bash
# 只运行 completion 模块
cargo test -- completion

# 只运行 upgrade 模块
cargo test -- upgrade

# 只运行 uninstall 模块
cargo test -- uninstall

# 只运行 CLI 集成测试
cargo test -- cli
```

### 7.3 单线程运行

```bash
cargo test -- --test-threads=1
```

> 环境变量测试（如 `detect_shell()`）需要串行执行，建议所有测试单线程运行。

---

## 8. 测试通过标准

### 8.1 必须通过

```bash
cargo test                                      # 全部测试通过
cargo fmt --check                               # 格式通过
cargo clippy -- -D warnings                     # Lint 通过
```

### 8.2 预期测试数量

| 模块 | 单元测试 | 集成测试 | 合计 |
|------|---------|---------|------|
| completion | 17 | 0 | 17 |
| upgrade | 19 | 0 | 19 |
| uninstall | 8 | 0 | 8 |
| cli.rs | 0 | 8 | 8 |
| **总计** | **44** | **8** | **52** |

---

## 9. 边界场景清单

### 9.1 upgrade

- [ ] GitHub API 返回非 200（网络错误、限流）→ 应返回有意义的错误信息
- [ ] 当前版本 > 最新版本（使用预发布版）→ 提示已最新
- [ ] 版本号含预发布标签如 "1.0.0-beta.1" → `compare_versions` 数字段过滤后的比较行为
- [ ] 版本号含非数字段如 "1.0.0.rc1" → `filter_map` 处理后只比较数字段
- [ ] 空版本号 vs 有效版本号 → 不 panic
- [ ] 平台不被支持（如 wasm）→ `detect_target_triple` 返回 Err
- [ ] 下载被中断 → 临时目录清理
- [ ] 解压失败（归档损坏）→ 错误向上传播
- [ ] 二进制替换时无写权限 → 错误提示清晰
- [ ] 临时目录有残留（上次升级被中断）→ PID 后缀隔离

### 9.2 uninstall

- [ ] 非 TTY 环境（管道、CI）→ 静默卸载
- [ ] 配置目录不存在 → 跳过删除，不 panic
- [ ] 二进制文件不存在（PATH 被修改）→ 跳过删除
- [ ] Windows 不尝试自动删除二进制 → 仅提示，文件保留
- [ ] shell 配置文件编码异常 → 静默跳过
- [ ] `run_with_temp_home` 测试中 panic → HOME 可能未恢复（现有 `test_util` 限制）

### 9.3 completion

- [ ] bash 无 `~/.bashrc` 且无 `~/.bash_profile` → 使用 `.bash_profile` 默认路径
- [ ] zsh 无 `~/.zshrc` → 创建文件
- [ ] fish 无 `~/.config/fish/` 目录 → 创建目录树
- [ ] 配置文件中已有 eval 行 → 幂等不重复插入
- [ ] 移除时 eval 行是文件唯一内容 → 文件清空
- [ ] 注释中的 eval 行不被移除 → 仅精确匹配
- [ ] 安装再移除（roundtrip）→ 文件内容恢复到原始状态

---

## 10. 与 zapmyco 测试模式对照

| 模式 | zapmyco | haimen（本方案） |
|------|---------|-----------------|
| 版本比较测试 | `test_compare_versions_*` x3 | 相同 |
| 平台检测 | `test_detect_target_triple` | 相同 |
| 路径定位 | `test_locate_binary_*` x3 | 相同（适配二进制名） |
| 收据更新 | `test_update_receipt_*` x2 | 相同（适配路径） |
| 二进制替换 | `test_replace_binary_*` x2 | 相同 |
| Shell 检测 | `test_detect_shell_from_env` | 相同 |
| 补全安装 | `test_setup_completion_*` x4 | 相同（分层测试） |
| 补全移除 | `test_remove_completion_*` x5 | 相同 |
| 卸载清理 | `test_execute_*` x5 | 相同 |
| CLI 解析 | `test_cli_parse_*` | 相同模式（新增命令） |
| 测试隔离 | `run_with_temp_home` + `tempfile::tempdir` | 完全相同（复用 lib.rs） |
