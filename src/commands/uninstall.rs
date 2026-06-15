//! haimen 自卸载模块
//!
//! 通过交互式确认或静默模式从系统移除 haimen。

use std::io::IsTerminal;
use std::path::Path;

use crate::commands::completion;

/// 卸载 haimen（交互式 + 清理）
pub fn cmd_uninstall() -> Result<(), String> {
    let home = crate::config::settings::get_home_dir();
    let haimen_dir = crate::config::settings::get_settings_dir();
    let exe_path = std::env::current_exe().ok();
    let receipt_dir = home.join(".config/haimen");
    let has_receipt = receipt_dir.exists();
    let has_haimen_dir = haimen_dir.exists();

    // 非交互式模式（管道、CI）：直接静默卸载
    if !std::io::stdin().is_terminal() {
        return execute_uninstall(
            &receipt_dir,
            &haimen_dir,
            has_receipt,
            true,
            exe_path.as_deref(),
            &home,
        );
    }

    // 交互式确认
    let want_keep_haimen = if has_haimen_dir {
        match ask_yes_no("是否保留 ~/.haimen/ 目录（配置和数据）？") {
            Some(val) => val,
            None => {
                println!("取消卸载。");
                return Ok(());
            }
        }
    } else {
        true
    };

    match ask_yes_no("是否确认卸载 haimen？") {
        Some(true) => {}
        _ => {
            println!("取消卸载。");
            return Ok(());
        }
    }

    execute_uninstall(
        &receipt_dir,
        &haimen_dir,
        has_receipt,
        want_keep_haimen,
        exe_path.as_deref(),
        &home,
    )
}

/// 执行卸载清理（不含用户交互，可测试）
pub fn execute_uninstall(
    receipt_dir: &Path,
    haimen_dir: &Path,
    has_receipt: bool,
    keep_haimen_dir: bool,
    exe_path: Option<&Path>,
    home: &Path,
) -> Result<(), String> {
    // 清理 shell 补全
    completion::remove_shell_completion(home);

    // 删除安装收据
    if has_receipt {
        if let Err(e) = std::fs::remove_dir_all(receipt_dir) {
            eprintln!("  警告: 删除安装收据失败: {}", e);
        }
    }

    // 删除配置目录
    if !keep_haimen_dir && haimen_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(haimen_dir) {
            eprintln!("  警告: 删除 {:?} 失败: {}", haimen_dir, e);
        }
    }

    // 删除当前二进制文件
    #[cfg(not(windows))]
    if let Some(path) = exe_path {
        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("  警告: 删除二进制文件失败: {}", e);
        }
    }

    #[cfg(windows)]
    if let Some(path) = exe_path {
        println!("请手动删除二进制文件: {:?}", path);
    }

    println!("haimen 已卸载。有缘再见~");
    Ok(())
}

/// 询问用户 yes/no，返回 `Some(bool)` 或 `None`（中断）
fn ask_yes_no(prompt: &str) -> Option<bool> {
    use std::io::Write;

    loop {
        print!("{} [Y/n] ", prompt);
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return None;
        }

        match input.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => return Some(true),
            "n" | "no" => return Some(false),
            _ => {
                println!("请输入 y 或 n。");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    // —————— execute_uninstall ——————

    #[test]
    fn test_execute_clean_state() {
        run_with_temp_home(|home| {
            let result = execute_uninstall(
                &home.join(".config/haimen"),
                &home.join(".haimen"),
                false,
                true,
                None,
                home,
            );
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_execute_receipt_only() {
        run_with_temp_home(|home| {
            let receipt_dir = home.join(".config/haimen");
            std::fs::create_dir_all(&receipt_dir).unwrap();
            std::fs::write(
                receipt_dir.join("haimen-receipt.json"),
                r#"{"version":"0.1.0"}"#,
            )
            .unwrap();

            assert!(receipt_dir.exists());
            let result =
                execute_uninstall(&receipt_dir, &home.join(".haimen"), true, true, None, home);
            assert!(result.is_ok());
            assert!(!receipt_dir.exists(), "收据目录应被删除");
        });
    }

    #[test]
    fn test_execute_keep_config() {
        run_with_temp_home(|home| {
            let haimen_dir = home.join(".haimen");
            std::fs::create_dir_all(&haimen_dir).unwrap();
            std::fs::write(haimen_dir.join("settings.toml"), "").unwrap();

            let result = execute_uninstall(
                &home.join(".config/haimen"),
                &haimen_dir,
                false,
                true, // keep config
                None,
                home,
            );
            assert!(result.is_ok());
            assert!(haimen_dir.exists(), "配置目录应保留");
        });
    }

    #[test]
    fn test_execute_remove_config() {
        run_with_temp_home(|home| {
            let haimen_dir = home.join(".haimen");
            std::fs::create_dir_all(&haimen_dir).unwrap();
            std::fs::write(haimen_dir.join("settings.toml"), "").unwrap();

            let result = execute_uninstall(
                &home.join(".config/haimen"),
                &haimen_dir,
                false,
                false, // remove config
                None,
                home,
            );
            assert!(result.is_ok());
            assert!(!haimen_dir.exists(), "配置目录应被删除");
        });
    }

    #[test]
    fn test_execute_receipt_delete_error() {
        run_with_temp_home(|home| {
            // 收据路径存在但不可读/删除 — 不 panic 即可
            let result = execute_uninstall(
                &home.join("nonexistent"),
                &home.join(".haimen"),
                true, // has_receipt = true but dir doesn't exist
                true,
                None,
                home,
            );
            assert!(result.is_ok());
        });
    }

    #[test]
    #[cfg(unix)]
    fn test_execute_binary_deleted() {
        run_with_temp_home(|home| {
            let binary = home.join("haimen");
            std::fs::write(&binary, "fake binary").unwrap();

            let result = execute_uninstall(
                &home.join(".config/haimen"),
                &home.join(".haimen"),
                false,
                true,
                Some(&binary),
                home,
            );
            assert!(result.is_ok());
            assert!(!binary.exists(), "Unix 上二进制文件应被删除");
        });
    }

    #[test]
    #[cfg(windows)]
    fn test_execute_binary_not_deleted_on_windows() {
        run_with_temp_home(|home| {
            let binary = home.join("haimen.exe");
            std::fs::write(&binary, "fake binary").unwrap();

            let result = execute_uninstall(
                &home.join(".config/haimen"),
                &home.join(".haimen"),
                false,
                true,
                Some(&binary),
                home,
            );
            assert!(result.is_ok());
            assert!(binary.exists(), "Windows 上二进制文件应保留");
        });
    }

    #[test]
    fn test_execute_complex_scenario() {
        run_with_temp_home(|home| {
            // Setup: receipt + config + binary
            let receipt_dir = home.join(".config/haimen");
            std::fs::create_dir_all(&receipt_dir).unwrap();
            std::fs::write(
                receipt_dir.join("haimen-receipt.json"),
                r#"{"version":"0.1.0"}"#,
            )
            .unwrap();

            let haimen_dir = home.join(".haimen");
            std::fs::create_dir_all(&haimen_dir).unwrap();
            std::fs::write(haimen_dir.join("settings.toml"), "").unwrap();

            let binary = home.join("haimen");
            std::fs::write(&binary, "fake binary").unwrap();

            let result = execute_uninstall(
                &receipt_dir,
                &haimen_dir,
                true,
                true, // keep config
                Some(&binary),
                home,
            );
            assert!(result.is_ok());
            assert!(!receipt_dir.exists(), "收据目录应被删除");
            assert!(haimen_dir.exists(), "配置目录应保留");
            assert!(
                !binary.exists() || cfg!(windows),
                "二进制应被删除（Windows 除外）"
            );
        });
    }

    // —————— cmd_uninstall ——————

    #[test]
    fn test_cmd_uninstall_clean_state() {
        run_with_temp_home(|_home| {
            // In non-TTY test environment, cmd_uninstall runs silent path
            let result = cmd_uninstall();
            assert!(result.is_ok());
        });
    }
}
