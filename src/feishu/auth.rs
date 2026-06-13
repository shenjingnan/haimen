use tokio::process::Command;

use super::bridge::LarkCliBridge;
use super::types::AuthStatus;

/// 显示飞书认证状态
pub async fn show_auth_status(bridge: &LarkCliBridge) -> Result<AuthStatus, String> {
    let value = bridge.exec(&["auth", "status", "--json"]).await?;

    // lark-cli auth status 直接返回数据（不包装在 data 字段中）
    serde_json::from_value(value).map_err(|e| format!("解析认证状态失败: {}", e))
}

/// 飞书登录（设备码授权）
///
/// 直接转发 stdin/stdout/stderr，让用户完成设备码授权流程。
pub async fn login() -> Result<(), String> {
    let status = Command::new("lark-cli")
        .args(["auth", "login"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| format!("启动 lark-cli 登录失败: {} (请确保 lark-cli 已安装)", e))?;

    if status.success() {
        println!("飞书登录成功。");
        Ok(())
    } else {
        Err(format!(
            "飞书登录失败 (exit code: {})",
            status.code().unwrap_or(-1)
        ))
    }
}
