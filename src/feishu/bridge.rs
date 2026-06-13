use std::pin::Pin;
use std::process::Stdio;

use futures_util::Stream;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::types::BridgeHealth;

/// lark-cli 子进程桥接器
pub struct LarkCliBridge {
    lark_cli_path: String,
}

impl LarkCliBridge {
    /// 创建桥接器实例
    pub fn new(lark_cli_path: impl Into<String>) -> Self {
        Self {
            lark_cli_path: lark_cli_path.into(),
        }
    }

    /// 执行一次性 lark-cli 命令并解析 JSON 输出
    pub async fn exec(&self, args: &[&str]) -> Result<serde_json::Value, String> {
        let output = Command::new(&self.lark_cli_path)
            .args(args)
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

        // 检查 lark-cli 的 ok 字段
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

    /// 启动长驻 lark-cli 进程并返回 stdout 行流
    pub async fn stream(
        &self,
        args: &[&str],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>, String> {
        let mut child = Command::new(&self.lark_cli_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("启动 lark-cli 失败: {}", e))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法获取 lark-cli stdout".to_string())?;

        let reader = BufReader::new(stdout);
        let lines = reader.lines();

        // 将 child 移入 unfold 状态中，保持进程存活
        let stream =
            futures_util::stream::unfold((lines, child), |(mut lines, _child)| async move {
                match lines.next_line().await {
                    Ok(Some(line)) => Some((Ok(line), (lines, _child))),
                    Ok(None) => None,
                    Err(e) => Some((
                        Err(format!("读取 lark-cli 输出失败: {}", e)),
                        (lines, _child),
                    )),
                }
            });
        Ok(Box::pin(stream))
    }

    /// 快速健康检查
    pub async fn health_check(&self) -> BridgeHealth {
        // 检查 lark-cli 是否存在
        let lark_cli_found = Command::new(&self.lark_cli_path)
            .arg("--version")
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

        // 检查认证状态：运行 auth status 成功即视为已认证
        let authenticated = Command::new(&self.lark_cli_path)
            .args(["auth", "status", "--json"])
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

    /// 获取 lark-cli 路径
    #[allow(dead_code)]
    pub fn path(&self) -> &str {
        &self.lark_cli_path
    }
}
