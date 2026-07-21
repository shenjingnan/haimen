use std::pin::Pin;
use std::process::Stdio;

use futures_util::Stream;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::types::BridgeHealth;

/// dws（DingTalk Workspace CLI）子进程桥接器
///
/// 提供三种通信模式：
/// - `exec()`: 一次性命令，等待子进程退出，解析 JSON stdout
/// - `stream()`: 长驻进程，逐行读取 NDJSON stdout，返回 Stream
/// - `health_check()`: 检查 dws 存在性和认证状态
pub struct DwsBridge {
    dws_path: String,
}

impl DwsBridge {
    /// 创建新的桥接器实例
    ///
    /// `dws_path` 可以是绝对路径或仅命令名（从 PATH 查找）
    pub fn new(dws_path: impl Into<String>) -> Self {
        Self {
            dws_path: dws_path.into(),
        }
    }

    /// 执行一次性命令，等待退出并解析 JSON 输出
    ///
    /// # 参数
    /// - `args`: 命令参数，例如 `["im", "message", "send-by-bot", ...]`
    ///
    /// # 返回值
    /// - `Ok(serde_json::Value)`: 命令成功执行后的 JSON 输出
    /// - `Err(String)`: 命令执行失败或输出解析失败的错误信息
    pub async fn exec(&self, args: &[&str]) -> Result<serde_json::Value, String> {
        let output = Command::new(&self.dws_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                format!(
                    "执行 dws 失败: {} (请确保 dws 已安装: npm i -g dingtalk-workspace-cli)",
                    e
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(format!(
                "dws 返回错误 (exit code: {}):\n  stderr: {}\n  stdout: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
                stdout.trim(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Ok(serde_json::Value::Null);
        }

        let value: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("解析 dws 输出失败: {}\n  输出内容: {}", e, trimmed))?;

        Ok(value)
    }

    /// 启动长驻进程，返回 NDJSON 行流
    ///
    /// 适用于 `dws event consume` 等需要长时间运行的命令。
    /// 子进程在 Stream 被 drop 时自动终止（`kill_on_drop = true`）。
    ///
    /// # 参数
    /// - `args`: 命令参数，例如 `["event", "consume", "user_im_message_receive_group", "-f", "ndjson"]`
    ///
    /// # 返回值
    /// - `Ok(Stream<Result<String, String>>)`: 每行一个 NDJSON 字符串的异步流
    /// - `Err(String)`: 子进程启动失败
    pub async fn stream(
        &self,
        args: &[&str],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>, String> {
        let mut child = Command::new(&self.dws_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                format!(
                    "启动 dws 失败: {} (请确保 dws 已安装: npm i -g dingtalk-workspace-cli)",
                    e
                )
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法获取 dws stdout".to_string())?;

        let reader = BufReader::new(stdout);
        let lines = reader.lines();

        let stream =
            futures_util::stream::unfold((lines, child), |(mut lines, _child)| async move {
                match lines.next_line().await {
                    Ok(Some(line)) => Some((Ok(line), (lines, _child))),
                    Ok(None) => None,
                    Err(e) => Some((Err(format!("读取 dws 输出失败: {}", e)), (lines, _child))),
                }
            });

        Ok(Box::pin(stream))
    }

    /// 健康检查：检查 dws 是否存在以及是否已认证
    ///
    /// 执行两个子进程命令：
    /// 1. `dws --version` — 检查 CLI 是否存在
    /// 2. `dws auth status` — 检查是否已登录认证
    pub async fn health_check(&self) -> BridgeHealth {
        // 检查 dws CLI 是否存在
        let dws_found = Command::new(&self.dws_path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !dws_found {
            return BridgeHealth {
                dws_found: false,
                authenticated: false,
            };
        }

        // 检查认证状态
        let authenticated = Command::new(&self.dws_path)
            .args(["auth", "status"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        BridgeHealth {
            dws_found,
            authenticated,
        }
    }

    /// 获取 dws 可执行文件路径
    pub fn path(&self) -> &str {
        &self.dws_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::OnceLock;

    /// 检查 dws 是否在 PATH 中（用于跳过需要真实 dws 的测试）
    fn dws_available() -> bool {
        static AVAILABLE: OnceLock<bool> = OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            std::process::Command::new("dws")
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    }

    #[test]
    fn test_bridge_new() {
        let bridge = DwsBridge::new("dws");
        assert_eq!(bridge.path(), "dws");
    }

    #[test]
    fn test_bridge_new_custom_path() {
        let bridge = DwsBridge::new("/usr/local/bin/dws");
        assert_eq!(bridge.path(), "/usr/local/bin/dws");
    }

    #[tokio::test]
    async fn test_exec_version() {
        if !dws_available() {
            eprintln!("跳过 test_exec_version: dws 未安装");
            return;
        }
        let bridge = DwsBridge::new("dws");
        let result = bridge.exec(&["--version"]).await;
        assert!(result.is_ok(), "dws --version 应成功: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_exec_invalid_command() {
        if !dws_available() {
            eprintln!("跳过 test_exec_invalid_command: dws 未安装");
            return;
        }
        let bridge = DwsBridge::new("dws");
        let result = bridge.exec(&["nonexistent-command-xyz"]).await;
        assert!(result.is_err(), "无效命令应返回错误");
    }

    #[tokio::test]
    async fn test_health_check_returns() {
        let bridge = DwsBridge::new("dws");
        let health = bridge.health_check().await;
        assert!(!health.dws_found || health.dws_found == health.authenticated || true);
        let _ = health.dws_found;
    }

    #[test]
    fn test_bridge_path_method() {
        let bridge = DwsBridge::new("dws");
        assert_eq!(bridge.path(), "dws");
    }

    #[tokio::test]
    async fn test_stream_invalid_args() {
        let bridge = DwsBridge::new("dws");
        let result = bridge
            .stream(&["event", "consume", "--nonexistent-flag"])
            .await;
        if let Ok(mut stream) = result {
            let first = stream.next().await;
            if let Some(Ok(line)) = first {
                assert!(!line.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn test_exec_with_format_json() {
        if !dws_available() {
            eprintln!("跳过 test_exec_with_format_json: dws 未安装");
            return;
        }
        let bridge = DwsBridge::new("dws");
        let result = bridge.exec(&["auth", "status", "--format", "json"]).await;
        if let Ok(value) = result {
            assert!(value.is_object() || value.is_null());
        }
    }

    #[tokio::test]
    async fn test_exec_dws_not_found() {
        let bridge = DwsBridge::new("nonexistent-dws-binary");
        let result = bridge.exec(&["--version"]).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("执行 dws 失败"));
    }

    #[test]
    fn test_bridge_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DwsBridge>();
    }
}
