use std::pin::Pin;
use std::process::Stdio;

use futures_util::Stream;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::types::BridgeHealth;

/// lark-cli 子进程桥接器
pub struct LarkCliBridge {
    lark_cli_path: String,
}

impl LarkCliBridge {
    pub fn new(lark_cli_path: impl Into<String>) -> Self {
        Self {
            lark_cli_path: lark_cli_path.into(),
        }
    }

    pub async fn exec(&self, args: &[&str]) -> Result<serde_json::Value, String> {
        // Windows 上 lark-cli 可能是 npm 安装的 .cmd shim，经 build_command 解析包装
        let output = Command::from(haimen_core::process::build_command(
            &self.lark_cli_path,
            args,
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("执行 lark-cli 失败: {} (请确保 lark-cli 已安装)", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(format!(
                "lark-cli 返回错误 (exit code: {}):\n  stderr: {}\n  stdout: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
                stdout.trim(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let value: serde_json::Value = serde_json::from_str(stdout.trim())
            .map_err(|e| format!("解析 lark-cli 输出失败: {}", e))?;

        if let Some(obj) = value.as_object() {
            if let Some(false) = obj.get("ok").and_then(|v| v.as_bool()) {
                let code = obj
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(|c| c.as_i64())
                    .unwrap_or(-1);
                let msg = obj
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("未知错误");
                return Err(format!("lark-cli API 错误 ({}): {}", code, msg));
            }
        }

        Ok(value)
    }

    pub async fn stream(
        &self,
        args: &[&str],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>, String> {
        let mut child = Command::from(haimen_core::process::build_command(
            &self.lark_cli_path,
            args,
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 必须保持子进程 stdin 打开：lark-cli 的 `event consume` 把 stdin EOF 当作
        // "父进程要求停止"的信号，读到 EOF 会以 `context canceled` 退出，导致消息流中断。
        // 这里用管道接管 stdin，并把写端句柄留在流状态里，确保其存活（不 drop）即不 EOF。
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("启动 lark-cli 失败: {}", e))?;

        // 持有 stdin 写端，防止 drop 后管道关闭（EOF）。无需写入，仅保证打开。
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "无法获取 lark-cli stdin".to_string())?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法获取 lark-cli stdout".to_string())?;

        let reader = BufReader::new(stdout);
        let lines = reader.lines();

        let stream = futures_util::stream::unfold(
            (lines, child, stdin),
            |(mut lines, _child, _stdin)| async move {
                match lines.next_line().await {
                    Ok(Some(line)) => Some((Ok(line), (lines, _child, _stdin))),
                    Ok(None) => None,
                    Err(e) => Some((
                        Err(format!("读取 lark-cli 输出失败: {}", e)),
                        (lines, _child, _stdin),
                    )),
                }
            },
        );
        Ok(Box::pin(stream))
    }

    pub async fn health_check(&self) -> BridgeHealth {
        let lark_cli_found = Command::from(haimen_core::process::build_command(
            &self.lark_cli_path,
            &["--version"],
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

        if !lark_cli_found {
            return BridgeHealth {
                lark_cli_found: false,
                authenticated: false,
                bot_ready: false,
            };
        }

        let authenticated = Command::from(haimen_core::process::build_command(
            &self.lark_cli_path,
            &["auth", "status", "--json"],
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

        BridgeHealth {
            lark_cli_found: true,
            authenticated,
            bot_ready: authenticated,
        }
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &str {
        &self.lark_cli_path
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::time::Duration;

    /// 回归测试：`stream()` 必须保持子进程 stdin 打开。
    ///
    /// lark-cli 的 `event consume` 在 stdin 读到 EOF 时会以 `context canceled` 退出，
    /// 导致飞书消息流中断（见 bridge 实现说明）。此处用 `cat` 模拟
    /// "stdin EOF 即退出"的子进程：
    /// - 修复前：cat 继承测试进程 stdin（EOF）→ 立即退出 → 流结束
    /// - 修复后：stdin 以管道持有 → cat 阻塞等待输入 → 流保持存活
    #[tokio::test]
    async fn test_stream_keeps_stdin_open() {
        let bridge = LarkCliBridge::new("cat");
        let mut stream = bridge.stream(&[]).await.expect("启动 cat 失败");

        // 给子进程时间初始化/退出
        tokio::time::sleep(Duration::from_millis(300)).await;

        // 200ms 内流不应结束（cat 未因 stdin EOF 退出）；若流提前结束则测试失败
        let next = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
        assert!(
            next.is_err(),
            "子进程 stdin 应保持打开，流不应因 EOF 提前结束"
        );
    }
}
