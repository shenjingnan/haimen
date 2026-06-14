use futures_util::StreamExt;
use tokio::process::Command;
use tracing;

use crate::feishu::bridge::LarkCliBridge;
use crate::feishu::types::FeishuEvent;

/// 运行网关编排循环
///
/// 流程:
/// 1. 监听飞书消息
/// 2. 收到消息 → 回复"已收到" → 调 claude --print 处理 → 结果回飞书
pub async fn run_chat_loop(feishu_bridge: &LarkCliBridge) -> Result<(), String> {
    // 1. 检查飞书健康状态
    let health = feishu_bridge.health_check().await;
    if !health.lark_cli_found {
        return Err("lark-cli 未安装。请执行: npm install -g @larksuite/cli".to_string());
    }
    if !health.authenticated {
        return Err("飞书未认证。请先执行: haimen feishu auth login".to_string());
    }

    // 2. 检查 claude CLI 是否可用
    if !check_claude_available().await {
        return Err(
            "claude CLI 未安装。请执行: npm install -g @anthropic-ai/claude-code".to_string(),
        );
    }

    // 3. 启动飞书事件监听
    let mut stream = feishu_bridge
        .stream(&[
            "event",
            "consume",
            "im.message.receive_v1",
            "--as",
            "bot",
            "--quiet",
        ])
        .await?;

    println!("🚀 网关已启动，等待飞书消息... (按 Ctrl+C 退出)");

    // 4. 事件循环
    while let Some(line_result) = stream.next().await {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!("读取事件流错误: {}", e);
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        // 解析飞书事件
        let event: FeishuEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => {
                // 非事件行，忽略
                continue;
            }
        };

        // 只处理文本消息
        if event.message_type != "text" {
            continue;
        }

        // 提取用户消息文本
        let user_message = extract_text_content(&event.content);
        let chat_id = event.chat_id.clone();

        println!("收到消息来自 {}: {}", event.sender_id, user_message);
        tracing::info!(
            sender = %event.sender_id,
            chat_id = %event.chat_id,
            message = %user_message,
            "收到飞书消息"
        );

        // 回复"已收到"
        if let Err(e) = crate::feishu::send::send_text(
            feishu_bridge,
            &chat_id,
            "✅ 已收到你的消息，正在处理...",
        )
        .await
        {
            eprintln!("发送确认消息失败: {}", e);
            tracing::warn!(chat_id = %chat_id, error = %e, "发送确认消息失败");
            continue;
        }
        tracing::info!(chat_id = %chat_id, "已发送确认消息");

        // 调用 claude --print 处理用户消息
        println!("正在调用 Claude...");
        tracing::info!(
            chat_id = %chat_id,
            sender = %event.sender_id,
            message = %user_message,
            "开始调用 claude --print"
        );

        let start = std::time::Instant::now();
        let output = Command::new("claude")
            .args(["--print", &user_message])
            .output()
            .await
            .map_err(|e| format!("执行 claude 失败: {}", e));

        match output {
            Ok(output) => {
                let elapsed = start.elapsed();

                if output.status.success() {
                    let response = String::from_utf8_lossy(&output.stdout).to_string();
                    let response = response.trim().to_string();

                    println!(
                        "Claude 处理完成，耗时: {:?}, 长度: {} 字符",
                        elapsed,
                        response.len()
                    );
                    tracing::info!(
                        chat_id = %chat_id,
                        elapsed_ms = elapsed.as_millis() as u64,
                        response_len = response.len(),
                        "Claude 处理完成"
                    );

                    // 发送结果回飞书
                    if let Err(e) = crate::feishu::send::send_text(
                        feishu_bridge,
                        &chat_id,
                        &format!("💡 处理完成:\n\n{}", response),
                    )
                    .await
                    {
                        eprintln!("发送结果消息失败: {}", e);
                        tracing::warn!(chat_id = %chat_id, error = %e, "发送结果消息失败");
                    } else {
                        tracing::info!(chat_id = %chat_id, "结果已发送到飞书");
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    eprintln!("Claude 执行失败: {}", stderr);
                    tracing::error!(chat_id = %chat_id, stderr = %stderr, "Claude 执行失败");

                    let _ = crate::feishu::send::send_text(
                        feishu_bridge,
                        &chat_id,
                        &format!("❌ 处理失败:\n\n{}", stderr),
                    )
                    .await;
                }
            }
            Err(e) => {
                eprintln!("Claude 调用失败: {}", e);
                tracing::error!(chat_id = %chat_id, error = %e, "Claude 调用失败");

                let _ = crate::feishu::send::send_text(
                    feishu_bridge,
                    &chat_id,
                    &format!("❌ 系统错误: {}", e),
                )
                .await;
            }
        }
    }

    Err("lark-cli 事件流意外结束".to_string())
}

/// 检查 claude CLI 是否可用
async fn check_claude_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 从飞书文本消息内容中提取文本
///
/// 飞书文本消息的 content 字段是 JSON 格式: {"text": "消息内容"}
/// 也可能是纯字符串: "消息内容"
fn extract_text_content(content: &str) -> String {
    // 尝试解析 JSON
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
            return text.to_string();
        }
    }

    // 如果不是 JSON 或没有 text 字段，直接返回
    content
        .trim_start_matches('"')
        .trim_end_matches('"')
        .to_string()
}
