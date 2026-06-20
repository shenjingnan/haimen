use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::Stream;
use futures_util::StreamExt;

use crate::gateway::channel::MessageChannel;
use crate::gateway::model::Message;

use super::bridge::LarkCliBridge;
use super::types::FeishuEvent;

/// 飞书/Lark 消息通道，实现 MessageChannel trait
pub struct LarkChannel {
    bridge: LarkCliBridge,
}

impl LarkChannel {
    pub fn new(lark_cli_path: impl Into<String>) -> Self {
        Self {
            bridge: LarkCliBridge::new(lark_cli_path),
        }
    }
}

#[async_trait]
impl MessageChannel for LarkChannel {
    fn name(&self) -> &str {
        "lark"
    }

    async fn listen(&self) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String> {
        let raw_stream = self
            .bridge
            .stream(&[
                "event",
                "consume",
                "im.message.receive_v1",
                "--as",
                "bot",
                "--quiet",
            ])
            .await?;

        let message_stream = raw_stream.filter_map(move |line_result| {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(error = %e, "读取 lark-cli 事件流错误");
                    return futures_util::future::ready(None);
                }
            };

            if line.trim().is_empty() {
                return futures_util::future::ready(None);
            }

            let event: FeishuEvent = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => return futures_util::future::ready(None),
            };

            if event.message_type != "text" {
                return futures_util::future::ready(None);
            }

            let content = extract_text_content(&event.content);
            let message = Message {
                id: event.message_id,
                conversation_id: event.chat_id,
                sender_id: event.sender_id,
                content,
                timestamp: Utc::now(),
                channel: "lark".to_string(),
            };

            futures_util::future::ready(Some(message))
        });

        Ok(Box::pin(message_stream))
    }

    async fn send(&self, conversation_id: &str, message: &str) -> Result<(), String> {
        self.bridge
            .exec(&[
                "im",
                "+messages-send",
                "--as",
                "bot",
                "--chat-id",
                conversation_id,
                "--text",
                message,
            ])
            .await?;
        Ok(())
    }

    async fn health_check(&self) -> Result<(), String> {
        let health = self.bridge.health_check().await;
        if !health.lark_cli_found {
            return Err("lark-cli 未安装".to_string());
        }
        if !health.authenticated {
            return Err("飞书未认证".to_string());
        }
        Ok(())
    }
}

/// 从飞书文本消息内容中提取文本
pub fn extract_text_content(content: &str) -> String {
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
            return text.to_string();
        }
    }
    content
        .trim_start_matches('"')
        .trim_end_matches('"')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_content_json() {
        let content = r#"{"text":"你好世界"}"#;
        assert_eq!(extract_text_content(content), "你好世界");
    }

    #[test]
    fn test_extract_text_content_plain() {
        assert_eq!(extract_text_content("\"hello\""), "hello");
        assert_eq!(extract_text_content("plain text"), "plain text");
    }

    #[test]
    fn test_extract_text_content_empty() {
        assert_eq!(extract_text_content(""), "");
    }
}
