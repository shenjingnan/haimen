use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::Stream;
use futures_util::StreamExt;

use haimen_core::Message;
use haimen_core::MessageChannel;

use crate::bridge::DwsBridge;
use crate::types::DingTalkEvent;

/// 配置：钉钉通道
#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    pub dws_path: String,
    pub share_session_in_channel: bool,
}

impl Default for DingTalkConfig {
    fn default() -> Self {
        Self {
            dws_path: "dws".to_string(),
            share_session_in_channel: false,
        }
    }
}

/// 钉钉消息通道
pub struct DingTalkChannel {
    bridge: DwsBridge,
    config: DingTalkConfig,
}

impl DingTalkChannel {
    pub fn new(config: DingTalkConfig) -> Self {
        let bridge = DwsBridge::new(&config.dws_path);
        Self { bridge, config }
    }

    pub fn new_with_path(dws_path: impl Into<String>) -> Self {
        Self {
            bridge: DwsBridge::new(dws_path),
            config: DingTalkConfig::default(),
        }
    }

    fn build_session_key(
        conversation_id: &str,
        conversation_type: &str,
        sender_id: &str,
        share_session: bool,
    ) -> String {
        let conv_type = match conversation_type {
            "group" => "g",
            _ => "d",
        };
        let prefix = format!("dingtalk:{}:{}", conv_type, conversation_id);
        if share_session && conv_type == "g" {
            prefix
        } else {
            format!("{}:{}", prefix, sender_id)
        }
    }

    fn parse_target_from_session_key(session_key: &str) -> (&str, &str) {
        let parts: Vec<&str> = session_key.splitn(4, ':').collect();
        let conv_type = parts.get(1).copied().unwrap_or("d");
        match conv_type {
            "g" => {
                let conversation_id = parts.get(2).copied().unwrap_or("");
                (conv_type, conversation_id)
            }
            _ => {
                let sender_id = parts.get(3).copied().unwrap_or("");
                (conv_type, sender_id)
            }
        }
    }

    fn is_old_message(create_time_millis: i64) -> bool {
        let msg_time =
            chrono::DateTime::from_timestamp_millis(create_time_millis).unwrap_or_default();
        let now = Utc::now();
        (now - msg_time).num_minutes() > 5
    }
}

#[async_trait]
impl MessageChannel for DingTalkChannel {
    fn name(&self) -> &str {
        "dingtalk"
    }

    async fn listen(&self) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String> {
        let share_session = self.config.share_session_in_channel;

        let raw_stream = self
            .bridge
            .stream(&[
                "event",
                "consume",
                "user_im_message_receive_group",
                "-f",
                "ndjson",
            ])
            .await?;

        let message_stream = raw_stream.filter_map(move |line_result| {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(error = %e, "读取 dws 事件流错误");
                    return futures_util::future::ready(None);
                }
            };

            let trimmed = line.trim();
            if trimmed.is_empty() {
                return futures_util::future::ready(None);
            }

            let event: DingTalkEvent = match serde_json::from_str(trimmed) {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        raw = %trimmed.chars().take(200).collect::<String>(),
                        "解析 dws 事件行失败"
                    );
                    return futures_util::future::ready(None);
                }
            };

            // 丢弃 5 分钟前的过期消息
            if Self::is_old_message(event.data.create_at) {
                tracing::debug!(msg_id = %event.data.msg_id, "丢弃过期消息");
                return futures_util::future::ready(None);
            }

            let session_key = Self::build_session_key(
                &event.data.conversation_id,
                &event.data.conversation_type,
                &event.data.sender_id,
                share_session,
            );

            let message = Message {
                id: event.data.msg_id,
                conversation_id: session_key,
                sender_id: event.data.sender_id,
                content: event.data.text.content,
                timestamp: Utc::now(),
                channel: "dingtalk".to_string(),
            };

            futures_util::future::ready(Some(message))
        });

        Ok(Box::pin(message_stream))
    }

    async fn send(&self, conversation_id: &str, message: &str) -> Result<(), String> {
        let (_conv_type, target) = Self::parse_target_from_session_key(conversation_id);

        tracing::info!(
            target = %target,
            message_len = message.len(),
            "发送钉钉消息"
        );

        self.bridge
            .exec(&[
                "im",
                "message",
                "send-by-bot",
                "--conversation-id",
                target,
                "--text",
                message,
                "--yes",
                "--format",
                "json",
            ])
            .await?;

        Ok(())
    }

    async fn health_check(&self) -> Result<(), String> {
        let health = self.bridge.health_check().await;
        if !health.dws_found {
            return Err(
                "dws (DingTalk CLI) 未安装或未在 PATH 中。请运行: npm i -g dingtalk-workspace-cli"
                    .to_string(),
            );
        }
        if !health.authenticated {
            return Err("钉钉未认证。请先运行: dws auth login".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let channel = DingTalkChannel::new(DingTalkConfig::default());
        assert_eq!(channel.name(), "dingtalk");
    }

    #[test]
    fn test_new_with_path() {
        let channel = DingTalkChannel::new_with_path("/usr/local/bin/dws");
        assert_eq!(channel.name(), "dingtalk");
    }

    #[test]
    fn test_build_session_key_group_shared() {
        assert_eq!(
            DingTalkChannel::build_session_key("cid123", "group", "uid456", true),
            "dingtalk:g:cid123"
        );
    }

    #[test]
    fn test_build_session_key_group_isolated() {
        assert_eq!(
            DingTalkChannel::build_session_key("cid123", "group", "uid456", false),
            "dingtalk:g:cid123:uid456"
        );
    }

    #[test]
    fn test_build_session_key_single() {
        assert_eq!(
            DingTalkChannel::build_session_key("cid789", "p2p", "uid111", false),
            "dingtalk:d:cid789:uid111"
        );
    }

    #[test]
    fn test_parse_target_group() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("dingtalk:g:cid123");
        assert_eq!(t, "g");
        assert_eq!(id, "cid123");
    }

    #[test]
    fn test_parse_target_single() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("dingtalk:d:cid789:uid111");
        assert_eq!(t, "d");
        assert_eq!(id, "uid111");
    }

    #[test]
    fn test_parse_target_malformed_empty() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("");
        assert_eq!(t, "d");
        assert_eq!(id, "");
    }

    #[test]
    fn test_is_old_message_fresh() {
        let now = Utc::now().timestamp_millis();
        assert!(!DingTalkChannel::is_old_message(now));
    }

    #[test]
    fn test_is_old_message_expired() {
        let old = (Utc::now() - chrono::Duration::minutes(10)).timestamp_millis();
        assert!(DingTalkChannel::is_old_message(old));
    }

    #[test]
    fn test_is_old_message_future() {
        let future = (Utc::now() + chrono::Duration::hours(1)).timestamp_millis();
        assert!(!DingTalkChannel::is_old_message(future));
    }

    #[test]
    fn test_is_old_message_zero() {
        assert!(DingTalkChannel::is_old_message(0));
    }

    #[test]
    fn test_config_default() {
        let config = DingTalkConfig::default();
        assert_eq!(config.dws_path, "dws");
        assert!(!config.share_session_in_channel);
    }

    #[tokio::test]
    async fn test_health_check_no_dws() {
        let channel = DingTalkChannel::new(DingTalkConfig {
            dws_path: "nonexistent-dws-binary".into(),
            ..Default::default()
        });
        let result = channel.health_check().await;
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("dws"));
    }

    #[tokio::test]
    async fn test_send_parses_session_key() {
        let channel = DingTalkChannel::new(DingTalkConfig::default());
        let result = channel.send("dingtalk:g:cid123:uid456", "hello").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_build_and_parse_roundtrip_group() {
        let key = DingTalkChannel::build_session_key("cid123", "group", "uid456", false);
        let (t, id) = DingTalkChannel::parse_target_from_session_key(&key);
        assert_eq!(t, "g");
        assert_eq!(id, "cid123");
    }

    #[test]
    fn test_build_and_parse_roundtrip_single() {
        let key = DingTalkChannel::build_session_key("cid789", "p2p", "uid111", false);
        let (t, id) = DingTalkChannel::parse_target_from_session_key(&key);
        assert_eq!(t, "d");
        assert_eq!(id, "uid111");
    }
}
