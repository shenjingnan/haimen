//! Shell 补全安装和移除工具函数。
//!
//! 供 upgrade 和 uninstall 命令使用。

use std::path::{Path, PathBuf};

/// 检测当前 shell 类型（从 `$SHELL` 环境变量）
pub(crate) fn detect_shell() -> Option<&'static str> {
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    match name {
        "bash" => Some("bash"),
        "zsh" => Some("zsh"),
        "fish" => Some("fish"),
        _ => None,
    }
}

/// 获取 shell 配置文件路径
///
/// - bash: `~/.bashrc`（优先）或 `~/.bash_profile`
/// - zsh: `~/.zshrc`
/// - fish: `~/.config/fish/config.fish`
pub(crate) fn shell_config_path(shell: &str, home: &Path) -> PathBuf {
    match shell {
        "bash" => {
            let bashrc = home.join(".bashrc");
            let bash_profile = home.join(".bash_profile");
            if bashrc.exists() {
                bashrc
            } else {
                bash_profile
            }
        }
        "zsh" => home.join(".zshrc"),
        "fish" => home.join(".config/fish/config.fish"),
        _ => panic!("不支持的 shell: {}", shell),
    }
}

/// 获取 shell 对应的补全 eval 行
pub(crate) fn completion_line(shell: &str) -> &'static str {
    match shell {
        "bash" => "eval \"$(haimen completion bash)\"",
        "zsh" => "eval \"$(haimen completion zsh)\"",
        "fish" => "haimen completion fish | source",
        _ => panic!("不支持的 shell: {}", shell),
    }
}

/// 安装 shell 补全（可测试的内部实现）
pub(crate) fn setup_shell_completion_inner(
    shell: Option<&str>,
    home: &Path,
) -> Result<String, String> {
    let shell = shell.ok_or_else(|| {
        "未能检测到当前 Shell（$SHELL 未设置）\n\
         请手动配置自动补全：运行 `haimen completion --help` 查看帮助。"
            .to_string()
    })?;

    let config_path = shell_config_path(shell, home);
    let line = completion_line(shell);

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("读取 {:?} 失败: {}", config_path, e))?;
        if content.contains(line) {
            return Ok(format!("Shell 自动补全已配置（{:?}）", config_path));
        }
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录 {:?} 失败: {}", parent, e))?;
    }

    let content = if config_path.exists() {
        let mut content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("读取 {:?} 失败: {}", config_path, e))?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(line);
        content.push('\n');
        content
    } else {
        format!("{}\n", line)
    };

    std::fs::write(&config_path, content)
        .map_err(|e| format!("写入 {:?} 失败: {}", config_path, e))?;

    let source_hint = match shell {
        "fish" => "请重启终端以生效。",
        _ => "请运行 `source` 命令或重启终端以生效。",
    };

    Ok(format!(
        "Shell 自动补全已启用（{:?}）。\n{}",
        config_path, source_hint,
    ))
}

/// 安装 shell 补全：从环境读取 shell/home，调用 `setup_shell_completion_inner`
pub(crate) fn setup_shell_completion() -> Result<String, String> {
    let home = crate::config::settings::get_home_dir();
    setup_shell_completion_inner(detect_shell(), &home)
}

/// 移除所有 shell 配置文件中的补全 eval 行
pub(crate) fn remove_shell_completion(home: &Path) {
    let shells = ["bash", "zsh", "fish"];
    for &shell in &shells {
        let config_path = shell_config_path(shell, home);
        if !config_path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&config_path) else {
            continue;
        };
        let line = completion_line(shell);
        let original_lines: Vec<&str> = content.lines().collect();
        let kept_lines: Vec<&str> = original_lines
            .iter()
            .filter(|l| l.trim() != line) // exact match only, skip commented lines
            .copied()
            .collect();

        // Only update if at least one line was removed
        if kept_lines.len() < original_lines.len() {
            let mut result = kept_lines.join("\n");
            if !result.is_empty() {
                result.push('\n');
            }
            let _ = std::fs::write(&config_path, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    // —————— ShellGuard ——————

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
                Some(val) => unsafe {
                    std::env::set_var("SHELL", val);
                },
                None => unsafe {
                    std::env::remove_var("SHELL");
                },
            }
        }
    }

    // —————— detect_shell ——————

    #[test]
    fn test_detect_shell_bash() {
        let _guard = ShellGuard::new();
        unsafe {
            std::env::set_var("SHELL", "/bin/bash");
        }
        assert_eq!(detect_shell(), Some("bash"));
    }

    #[test]
    fn test_detect_shell_zsh() {
        let _guard = ShellGuard::new();
        unsafe {
            std::env::set_var("SHELL", "/usr/bin/zsh");
        }
        assert_eq!(detect_shell(), Some("zsh"));
    }

    #[test]
    fn test_detect_shell_fish() {
        let _guard = ShellGuard::new();
        unsafe {
            std::env::set_var("SHELL", "/opt/homebrew/bin/fish");
        }
        assert_eq!(detect_shell(), Some("fish"));
    }

    #[test]
    fn test_detect_shell_unsupported() {
        let _guard = ShellGuard::new();
        unsafe {
            std::env::set_var("SHELL", "/bin/sh");
        }
        assert_eq!(detect_shell(), None);
    }

    #[test]
    fn test_detect_shell_unset() {
        let _guard = ShellGuard::new();
        unsafe {
            std::env::remove_var("SHELL");
        }
        assert_eq!(detect_shell(), None);
    }

    // —————— shell_config_path ——————

    #[test]
    fn test_shell_config_path_bash_bashrc_exists() {
        run_with_temp_home(|home| {
            std::fs::write(home.join(".bashrc"), "").unwrap();
            std::fs::write(home.join(".bash_profile"), "").unwrap();
            let path = shell_config_path("bash", home);
            assert_eq!(path.file_name().unwrap(), ".bashrc");
        });
    }

    #[test]
    fn test_shell_config_path_bash_fallback_profile() {
        run_with_temp_home(|home| {
            std::fs::write(home.join(".bash_profile"), "").unwrap();
            let path = shell_config_path("bash", home);
            assert_eq!(path.file_name().unwrap(), ".bash_profile");
        });
    }

    #[test]
    fn test_shell_config_path_bash_default() {
        run_with_temp_home(|home| {
            let path = shell_config_path("bash", home);
            assert_eq!(path.file_name().unwrap(), ".bash_profile");
        });
    }

    #[test]
    fn test_shell_config_path_zsh() {
        run_with_temp_home(|home| {
            let path = shell_config_path("zsh", home);
            assert_eq!(path.file_name().unwrap(), ".zshrc");
        });
    }

    #[test]
    fn test_shell_config_path_fish() {
        run_with_temp_home(|home| {
            let path = shell_config_path("fish", home);
            assert!(path.ends_with(".config/fish/config.fish"));
        });
    }

    // —————— completion_line ——————

    #[test]
    fn test_completion_line_bash() {
        assert_eq!(
            completion_line("bash"),
            "eval \"$(haimen completion bash)\""
        );
    }

    #[test]
    fn test_completion_line_zsh() {
        assert_eq!(completion_line("zsh"), "eval \"$(haimen completion zsh)\"");
    }

    #[test]
    fn test_completion_line_fish() {
        assert_eq!(completion_line("fish"), "haimen completion fish | source");
    }

    // —————— setup_shell_completion_inner ——————

    #[test]
    fn test_setup_completion_new_file() {
        run_with_temp_home(|home| {
            let result = setup_shell_completion_inner(Some("bash"), home);
            assert!(result.is_ok());
            let msg = result.unwrap();
            assert!(msg.contains("Shell 自动补全已启用"));

            let content = std::fs::read_to_string(home.join(".bash_profile")).unwrap();
            assert!(content.contains("haimen completion bash"));
        });
    }

    #[test]
    fn test_setup_completion_append_existing() {
        run_with_temp_home(|home| {
            std::fs::write(home.join(".zshrc"), "export FOO=bar\n").unwrap();

            let result = setup_shell_completion_inner(Some("zsh"), home);
            assert!(result.is_ok());

            let content = std::fs::read_to_string(home.join(".zshrc")).unwrap();
            assert!(content.contains("export FOO=bar"));
            assert!(content.contains("haimen completion zsh"));
        });
    }

    #[test]
    fn test_setup_completion_idempotent() {
        run_with_temp_home(|home| {
            std::fs::write(home.join(".zshrc"), "").unwrap();

            let r1 = setup_shell_completion_inner(Some("zsh"), home);
            assert!(r1.is_ok());
            assert!(r1.unwrap().contains("已启用"));

            let r2 = setup_shell_completion_inner(Some("zsh"), home);
            assert!(r2.is_ok());
            assert!(r2.unwrap().contains("已配置"));

            let content = std::fs::read_to_string(home.join(".zshrc")).unwrap();
            assert_eq!(content.matches("haimen completion zsh").count(), 1);
        });
    }

    #[test]
    fn test_setup_completion_no_shell() {
        run_with_temp_home(|home| {
            let result = setup_shell_completion_inner(None, home);
            assert!(result.is_err());
            assert!(result.err().unwrap().contains("$SHELL 未设置"));
        });
    }

    // —————— remove_shell_completion ——————

    #[test]
    fn test_remove_completion_removes_line() {
        run_with_temp_home(|home| {
            let zshrc = home.join(".zshrc");
            std::fs::write(
                &zshrc,
                "export FOO=bar\neval \"$(haimen completion zsh)\"\nexport BAR=baz\n",
            )
            .unwrap();

            remove_shell_completion(home);

            let content = std::fs::read_to_string(&zshrc).unwrap();
            assert!(!content.contains("haimen completion zsh"));
            assert!(content.contains("export FOO=bar"));
            assert!(content.contains("export BAR=baz"));
        });
    }

    #[test]
    fn test_remove_completion_only_line() {
        run_with_temp_home(|home| {
            let zshrc = home.join(".zshrc");
            std::fs::write(&zshrc, "eval \"$(haimen completion zsh)\"\n").unwrap();

            remove_shell_completion(home);

            let content = std::fs::read_to_string(&zshrc).unwrap();
            assert!(content.is_empty());
        });
    }

    #[test]
    fn test_remove_completion_multiple_matches() {
        run_with_temp_home(|home| {
            let zshrc = home.join(".zshrc");
            std::fs::write(
                &zshrc,
                "eval \"$(haimen completion zsh)\"\nexport FOO=bar\neval \"$(haimen completion zsh)\"\n",
            )
            .unwrap();

            remove_shell_completion(home);

            let content = std::fs::read_to_string(&zshrc).unwrap();
            assert_eq!(content.matches("haimen completion zsh").count(), 0);
            assert!(content.contains("export FOO=bar"));
        });
    }

    #[test]
    fn test_remove_completion_file_not_exists() {
        run_with_temp_home(|home| {
            // Should not panic
            remove_shell_completion(home);
            assert!(!home.join(".zshrc").exists());
        });
    }

    #[test]
    fn test_remove_completion_all_shells() {
        run_with_temp_home(|home| {
            std::fs::write(
                home.join(".bash_profile"),
                "eval \"$(haimen completion bash)\"\n",
            )
            .unwrap();
            std::fs::write(home.join(".zshrc"), "eval \"$(haimen completion zsh)\"\n").unwrap();
            std::fs::create_dir_all(home.join(".config/fish")).unwrap();
            std::fs::write(
                home.join(".config/fish/config.fish"),
                "haimen completion fish | source\n",
            )
            .unwrap();

            remove_shell_completion(home);

            let bash_content = std::fs::read_to_string(home.join(".bash_profile")).unwrap();
            assert!(!bash_content.contains("haimen completion bash"));

            let zsh_content = std::fs::read_to_string(home.join(".zshrc")).unwrap();
            assert!(!zsh_content.contains("haimen completion zsh"));

            let fish_content =
                std::fs::read_to_string(home.join(".config/fish/config.fish")).unwrap();
            assert!(!fish_content.contains("haimen completion fish"));
        });
    }

    #[test]
    fn test_remove_completion_keeps_commented_lines() {
        run_with_temp_home(|home| {
            let zshrc = home.join(".zshrc");
            std::fs::write(
                &zshrc,
                "# eval \"$(haimen completion zsh)\"\neval \"$(haimen completion zsh)\"\n",
            )
            .unwrap();

            remove_shell_completion(home);

            let content = std::fs::read_to_string(&zshrc).unwrap();
            // Commented line should remain
            assert!(content.contains("# eval"));
            // Uncommented eval line should be removed
            let uncommented_count = content
                .lines()
                .filter(|l| l.trim() == "eval \"$(haimen completion zsh)\"")
                .count();
            assert_eq!(uncommented_count, 0);
        });
    }

    #[test]
    fn test_remove_completion_roundtrip() {
        run_with_temp_home(|home| {
            let original = "export FOO=bar\nexport BAR=baz\n";
            let zshrc = home.join(".zshrc");
            std::fs::write(&zshrc, original).unwrap();

            // Install
            setup_shell_completion_inner(Some("zsh"), home).unwrap();
            let after_install = std::fs::read_to_string(&zshrc).unwrap();
            assert!(after_install.contains("haimen completion zsh"));
            assert!(after_install.contains("export FOO=bar"));

            // Remove
            remove_shell_completion(home);
            let after_remove = std::fs::read_to_string(&zshrc).unwrap();
            // Original content should be restored
            assert!(after_remove.contains("export FOO=bar"));
            assert!(after_remove.contains("export BAR=baz"));
            assert!(!after_remove.contains("haimen completion zsh"));
        });
    }
}
