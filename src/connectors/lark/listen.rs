use futures_util::StreamExt;
use tokio::time::{Duration, sleep};

use super::bridge::LarkCliBridge;
use super::types::FeishuEvent;

/// 事件订阅模式：监听飞书消息
///
/// 通过 `lark-cli event consume im.message.receive_v1` 实时接收消息。
/// 需要飞书开发者后台启用 im.message.receive_v1 事件订阅。
pub async fn listen_events(bridge: &LarkCliBridge, use_json: bool) -> Result<(), String> {
    // 先做健康检查
    let health = bridge.health_check().await;
    if !health.lark_cli_found {
        return Err("lark-cli 未安装。请执行: npm install -g @larksuite/cli".to_string());
    }
    if !health.authenticated {
        return Err("飞书未认证。请先执行: haimen feishu auth login".to_string());
    }

    println!("正在监听飞书消息... (按 Ctrl+C 退出)");

    let mut stream = bridge
        .stream(&[
            "event",
            "consume",
            "im.message.receive_v1",
            "--as",
            "bot",
            "--quiet",
        ])
        .await?;

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

        match serde_json::from_str::<FeishuEvent>(&line) {
            Ok(event) => {
                if use_json {
                    println!("{}", line);
                } else {
                    print_event_pretty(&event);
                }
            }
            Err(e) => {
                // 非 NDJSON 行（如 lark-cli 的日志），在 quiet 模式应很少出现
                tracing::debug!("忽略非事件行: {} ({})", line.trim(), e);
            }
        }
    }

    Err("lark-cli 事件流意外结束".to_string())
}

/// 轮询模式：定期检查新消息
///
/// 通过 lark-cli 定期查询指定聊天的消息。
pub async fn listen_poll(
    bridge: &LarkCliBridge,
    chat_id: &str,
    interval_secs: u64,
    use_json: bool,
) -> Result<(), String> {
    let health = bridge.health_check().await;
    if !health.lark_cli_found {
        return Err("lark-cli 未安装。请执行: npm install -g @larksuite/cli".to_string());
    }
    if !health.authenticated {
        return Err("飞书未认证。请先执行: haimen feishu auth login".to_string());
    }

    println!(
        "正在轮询聊天 {} 的消息 (间隔 {} 秒)... (按 Ctrl+C 退出)",
        chat_id, interval_secs
    );

    // 记录最新消息的时间作为游标
    let mut cursor: Option<String> = None;

    loop {
        let mut args = vec![
            "im",
            "+chat-messages-list",
            "--as",
            "bot",
            "--chat-id",
            chat_id,
            "--order",
            "asc",
            "--format",
            "json",
        ];
        if let Some(ref start) = cursor {
            args.push("--start");
            args.push(start);
        }

        match bridge.exec(&args).await {
            Ok(value) => {
                if let Some(data) = value.get("data") {
                    if let Some(messages) = data.get("messages").and_then(|m| m.as_array()) {
                        for msg_value in messages {
                            if let Ok(event) =
                                serde_json::from_value::<FeishuEvent>(msg_value.clone())
                            {
                                // 更新游标
                                cursor = Some(event.create_time.clone());

                                if use_json {
                                    println!(
                                        "{}",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                } else {
                                    print_event_pretty(&event);
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("轮询消息失败: {}", e);
                // 不要退出，继续重试
            }
        }

        sleep(Duration::from_secs(interval_secs)).await;
    }
}

/// 格式化输出飞书事件
fn print_event_pretty(event: &FeishuEvent) {
    let chat_label = match event.chat_type.as_str() {
        "p2p" => "私聊",
        "group" => "群聊",
        _ => &event.chat_type,
    };

    // 尝试格式化时间
    let time_str = if event.create_time.len() >= 13 {
        let millis: i64 = event.create_time[..13].parse().unwrap_or(0);
        let secs = millis / 1000;
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        } else {
            event.create_time.clone()
        }
    } else {
        event.create_time.clone()
    };

    let msg_type_label = match event.message_type.as_str() {
        "text" => "文本",
        "image" => "图片",
        "audio" => "语音",
        "video" => "视频",
        "file" => "文件",
        "post" => "富文本",
        "system" => "系统消息",
        _ => &event.message_type,
    };

    println!("[{}] 来自 {} ({})", time_str, event.sender_id, chat_label);
    println!("━━━ {} ━━━", msg_type_label);

    // 文本消息直接显示内容，其他类型显示原始 JSON
    if event.message_type == "text" {
        // 尝试解析文本内容中的 JSON
        if let Ok(text_obj) = serde_json::from_str::<serde_json::Value>(&event.content) {
            if let Some(text) = text_obj.get("text").and_then(|t| t.as_str()) {
                println!("{}", text);
            } else {
                println!("{}", event.content);
            }
        } else {
            // 不是 JSON，直接打印
            let text = event.content.trim_start_matches('"').trim_end_matches('"');
            println!("{}", text);
        }
    } else {
        println!("{}", event.content);
    }
    println!("━━━━━━━━━━━");
    println!();
}
