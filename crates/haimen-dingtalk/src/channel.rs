use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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
    /// dws 可执行文件路径（默认 "dws"）
    pub dws_path: String,
    /// 群聊中是否共享 Agent 会话
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
///
/// 通过 dws CLI 桥接实现 MessageChannel trait。
/// 接收消息支持同时监听群消息（`user_im_message_receive_group`）和
/// @我消息（`user_im_message_receive_at`），自动合并流。
/// 发送消息使用 `dws im message send-by-bot`（一次性命令）。
pub struct DingTalkChannel {
    bridge: DwsBridge,
    config: DingTalkConfig,
    /// 消息 ID 去重缓存
    seen_msgs: Arc<Mutex<HashSet<String>>>,
}

impl DingTalkChannel {
    pub fn new(config: DingTalkConfig) -> Self {
        let bridge = DwsBridge::new(&config.dws_path);
        Self {
            bridge,
            config,
            seen_msgs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn new_with_path(dws_path: impl Into<String>) -> Self {
        Self {
            bridge: DwsBridge::new(dws_path),
            config: DingTalkConfig::default(),
            seen_msgs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 构建 session key
    ///
    /// 格式:
    ///   群聊共享: dingtalk:g:{openConversationId}
    ///   群聊隔离: dingtalk:g:{openConversationId}:{senderId}
    ///   单聊:     dingtalk:d:{conversationId}:{senderId}
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

    /// 从 session key 解析目标标识（用于 send）
    ///
    /// 返回: (conv_type, target_id)
    ///   conv_type = "g" → target_id 是 openConversationId（群聊）
    ///   conv_type = "d" → target_id 是 senderId（单聊）
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

    /// 检查消息是否过期（超过 5 分钟视为过期）
    fn is_old_message(create_time_millis: i64) -> bool {
        let msg_time =
            chrono::DateTime::from_timestamp_millis(create_time_millis).unwrap_or_default();
        let now = Utc::now();
        (now - msg_time).num_minutes() > 5
    }

    /// 将一行 NDJSON 字符串解析为 Message（返回 None 表示跳过该行）
    fn parse_line_to_message(line: &str, share_session: bool) -> Option<Message> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let event: DingTalkEvent = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => {
                // 非事件行（心跳、连接状态等），静默跳过
                return None;
            }
        };

        // 丢弃过期消息
        if Self::is_old_message(event.data.create_at) {
            return None;
        }

        let session_key = Self::build_session_key(
            &event.data.conversation_id,
            &event.data.conversation_type,
            &event.data.sender_id,
            share_session,
        );

        Some(Message {
            id: event.data.msg_id,
            conversation_id: session_key,
            sender_id: event.data.sender_id,
            content: event.data.text.content,
            timestamp: Utc::now(),
            channel: "dingtalk".to_string(),
        })
    }

    /// 构建单个事件流：从原始 NDJSON 流转换为 Message 流
    async fn build_event_stream(
        &self,
        event_key: String,
        share_session: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String> {
        let raw_stream = self
            .bridge
            .stream(&["event", "consume", &event_key, "-f", "ndjson"])
            .await?;

        let message_stream = raw_stream.filter_map(move |line_result| {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(event_key = %event_key, error = %e, "dws 事件流读取错误");
                    return futures_util::future::ready(None);
                }
            };

            futures_util::future::ready(Self::parse_line_to_message(&line, share_session))
        });

        Ok(Box::pin(message_stream))
    }

    /// 构建事件流，失败时记录警告并返回 None
    async fn try_build_event_stream(
        &self,
        event_key: &str,
        share_session: bool,
    ) -> Option<Pin<Box<dyn Stream<Item = Message> + Send>>> {
        match self
            .build_event_stream(event_key.to_string(), share_session)
            .await
        {
            Ok(stream) => {
                tracing::info!(event_key = %event_key, "钉钉事件流已启动");
                Some(stream)
            }
            Err(e) => {
                tracing::warn!(event_key = %event_key, error = %e, "启动钉钉事件流失败，跳过");
                None
            }
        }
    }
}

#[async_trait]
impl MessageChannel for DingTalkChannel {
    fn name(&self) -> &str {
        "dingtalk"
    }

    async fn listen(&self) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String> {
        let share_session = self.config.share_session_in_channel;

        // 启动群消息监听（核心事件，失败则整体错误）
        let group_stream = self
            .build_event_stream("user_im_message_receive_group".to_string(), share_session)
            .await?;

        // 尝试启动 @我消息监听（可选事件，失败只警告）
        let at_stream = self
            .try_build_event_stream("user_im_message_receive_at", share_session)
            .await;

        // 尝试启动单聊消息监听（可选事件，失败只警告）
        let o2o_stream = self
            .try_build_event_stream("user_im_message_receive_o2o", share_session)
            .await;

        // 构建事件源描述（先判断，后消费）
        let mut sources = vec!["group"];
        let mut streams: Vec<Pin<Box<dyn Stream<Item = Message> + Send>>> = vec![group_stream];
        if at_stream.is_some() {
            sources.push("at");
        }
        if o2o_stream.is_some() {
            sources.push("o2o");
        }

        if let Some(s) = at_stream {
            streams.push(s);
        }
        if let Some(s) = o2o_stream {
            streams.push(s);
        }

        let merged = if streams.len() == 1 {
            streams.into_iter().next().unwrap()
        } else {
            Box::pin(futures_util::stream::select_all(streams))
        };

        tracing::info!(sources = %sources.join(", "), "钉钉监听已启动");

        // 添加跨流去重：同一 msg_id 只处理一次
        let seen = Arc::clone(&self.seen_msgs);
        Ok(Box::pin(merged.filter_map(move |msg| {
            let mut cache = seen.lock().unwrap();
            if cache.contains(&msg.id) {
                futures_util::future::ready(None)
            } else {
                cache.insert(msg.id.clone());
                if cache.len() > 4096 {
                    cache.clear();
                }
                futures_util::future::ready(Some(msg))
            }
        })))
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
    use chrono::Duration;

    // ── Helper: 构建测试用的事件 JSON ──────────────────────────

    fn make_event_json(
        event_type: &str,
        conversation_id: &str,
        conversation_type: &str,
        sender_id: &str,
        msg_id: &str,
        content: &str,
        create_at: i64,
    ) -> String {
        serde_json::json!({
            "event_id": format!("evt_{}", msg_id),
            "event_type": event_type,
            "data": {
                "conversation_id": conversation_id,
                "conversation_type": conversation_type,
                "msg_id": msg_id,
                "sender_id": sender_id,
                "text": { "content": content },
                "create_at": create_at
            }
        })
        .to_string()
    }

    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    // ── parse_line_to_message ──────────────────────────────────

    #[test]
    fn test_parse_line_to_message_group() {
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid123",
            "group",
            "uid456",
            "msg001",
            "你好",
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, false).unwrap();
        assert_eq!(msg.channel, "dingtalk");
        assert_eq!(msg.content, "你好");
        assert_eq!(msg.conversation_id, "dingtalk:g:cid123:uid456");
    }

    #[test]
    fn test_parse_line_to_message_at_event() {
        let json = make_event_json(
            "user_im_message_receive_at",
            "cid_at_01",
            "group",
            "uid789",
            "msg_at_001",
            "@我 帮查天气",
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, true).unwrap();
        assert_eq!(msg.content, "@我 帮查天气");
        // share_session=true, group → 共享 session key（无 sender_id 后缀）
        assert_eq!(msg.conversation_id, "dingtalk:g:cid_at_01");
    }

    #[test]
    fn test_parse_line_to_message_o2o() {
        let json = make_event_json(
            "user_im_message_receive_o2o",
            "cid_o2o_01",
            "p2p",
            "uid111",
            "msg_o2o_001",
            "单聊消息",
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, false).unwrap();
        assert_eq!(msg.content, "单聊消息");
        assert_eq!(msg.conversation_id, "dingtalk:d:cid_o2o_01:uid111");
    }

    #[test]
    fn test_parse_line_to_message_empty_line() {
        assert!(DingTalkChannel::parse_line_to_message("", false).is_none());
        assert!(DingTalkChannel::parse_line_to_message("  ", false).is_none());
    }

    #[test]
    fn test_parse_line_to_message_invalid_json() {
        assert!(DingTalkChannel::parse_line_to_message("not json", false).is_none());
    }

    #[test]
    fn test_parse_line_to_message_expired() {
        let old_ts = (Utc::now() - Duration::minutes(10)).timestamp_millis();
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid",
            "group",
            "uid",
            "msg_expired",
            "过期消息",
            old_ts,
        );
        assert!(DingTalkChannel::parse_line_to_message(&json, false).is_none());
    }

    #[test]
    fn test_parse_line_to_message_partial_data() {
        // missing conversation_type → defaults to "d" (p2p)
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid",
            "",
            "uid",
            "msg_p",
            "partial",
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, false).unwrap();
        // empty conversation_type -> "d"
        assert!(msg.conversation_id.starts_with("dingtalk:d:"));
    }

    #[test]
    fn test_parse_line_to_message_zero_create_at() {
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid",
            "group",
            "uid",
            "msg_zero",
            "zero time",
            0,
        );
        // create_at=0 is considered old (epoch is >5min ago)
        assert!(DingTalkChannel::parse_line_to_message(&json, false).is_none());
    }

    // ── 事件类型不影响解析结果 ────────────────────────────────

    #[test]
    fn test_parse_line_different_event_types_render_same_message() {
        let ts = now_ms();
        let group_msg = DingTalkChannel::parse_line_to_message(
            &make_event_json(
                "user_im_message_receive_group",
                "cid",
                "group",
                "uid",
                "m1",
                "同内容",
                ts,
            ),
            false,
        )
        .unwrap();
        let at_msg = DingTalkChannel::parse_line_to_message(
            &make_event_json(
                "user_im_message_receive_at",
                "cid",
                "group",
                "uid",
                "m2",
                "同内容",
                ts,
            ),
            false,
        )
        .unwrap();
        // 内容相同，channel 相同
        assert_eq!(group_msg.content, at_msg.content);
        assert_eq!(group_msg.channel, at_msg.channel);
    }

    // ── 通道名 ─────────────────────────────────────────────────

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

    // ── session_key 构建 ───────────────────────────────────────

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
    fn test_build_session_key_group_shared_different_senders() {
        let key1 = DingTalkChannel::build_session_key("cid123", "group", "uid456", true);
        let key2 = DingTalkChannel::build_session_key("cid123", "group", "uid789", true);
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_build_session_key_unknown_type_defaults_to_single() {
        let key = DingTalkChannel::build_session_key("cid", "unknown", "uid", false);
        assert_eq!(key, "dingtalk:d:cid:uid");
    }

    // ── parse_target_from_session_key ──────────────────────────

    #[test]
    fn test_parse_target_group() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("dingtalk:g:cid123");
        assert_eq!(t, "g");
        assert_eq!(id, "cid123");
    }

    #[test]
    fn test_parse_target_group_isolated() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("dingtalk:g:cid123:uid456");
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
    fn test_parse_target_malformed_wrong_prefix() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("dingtalk:x:cid:uid");
        assert_eq!(t, "x");
        assert_eq!(id, "uid");
    }

    #[test]
    fn test_parse_target_malformed_too_short() {
        let (t, id) = DingTalkChannel::parse_target_from_session_key("dingtalk");
        assert_eq!(t, "d");
        assert_eq!(id, "");
    }

    // ── is_old_message ─────────────────────────────────────────

    #[test]
    fn test_is_old_message_fresh() {
        assert!(!DingTalkChannel::is_old_message(now_ms()));
    }

    #[test]
    fn test_is_old_message_expired() {
        let old = (Utc::now() - Duration::minutes(10)).timestamp_millis();
        assert!(DingTalkChannel::is_old_message(old));
    }

    #[test]
    fn test_is_old_message_just_under_limit() {
        let recent = (Utc::now() - Duration::minutes(4)).timestamp_millis();
        assert!(!DingTalkChannel::is_old_message(recent));
    }

    #[test]
    fn test_is_old_message_future() {
        let future = (Utc::now() + Duration::hours(1)).timestamp_millis();
        assert!(!DingTalkChannel::is_old_message(future));
    }

    #[test]
    fn test_is_old_message_zero() {
        assert!(DingTalkChannel::is_old_message(0));
    }

    // ── Config ─────────────────────────────────────────────────

    #[test]
    fn test_config_default() {
        let config = DingTalkConfig::default();
        assert_eq!(config.dws_path, "dws");
        assert!(!config.share_session_in_channel);
    }

    #[test]
    fn test_config_custom() {
        let config = DingTalkConfig {
            dws_path: "/opt/bin/dws".into(),
            share_session_in_channel: true,
        };
        assert_eq!(config.dws_path, "/opt/bin/dws");
        assert!(config.share_session_in_channel);
    }

    // ── 集成行为 ───────────────────────────────────────────────

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

    #[test]
    fn test_build_and_parse_roundtrip_group_shared() {
        let key = DingTalkChannel::build_session_key("cid123", "group", "uid456", true);
        let (t, id) = DingTalkChannel::parse_target_from_session_key(&key);
        assert_eq!(t, "g");
        assert_eq!(id, "cid123");
    }

    // ── parse_line_to_message 边界 ─────────────────────────────

    #[test]
    fn test_parse_line_to_message_special_chars() {
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid_spec",
            "group",
            "uid_spec",
            "msg_spec",
            "Hello\nWorld\t!@#$%",
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, false).unwrap();
        assert_eq!(msg.content, "Hello\nWorld\t!@#$%");
    }

    #[test]
    fn test_parse_line_to_message_unicode() {
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid_uni",
            "group",
            "uid_uni",
            "msg_uni",
            "你好世界🌍🔥",
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, false).unwrap();
        assert_eq!(msg.content, "你好世界🌍🔥");
    }

    #[test]
    fn test_parse_line_to_message_long_content() {
        let long_text = "A".repeat(10000);
        let json = make_event_json(
            "user_im_message_receive_group",
            "cid_long",
            "group",
            "uid_long",
            "msg_long",
            &long_text,
            now_ms(),
        );
        let msg = DingTalkChannel::parse_line_to_message(&json, false).unwrap();
        assert_eq!(msg.content.len(), 10000);
    }
}
