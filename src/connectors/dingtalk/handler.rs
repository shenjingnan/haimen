use std::collections::HashSet;

use chrono::Utc;
use tokio::sync::mpsc;
use tracing;

use crate::gateway::model::Message;

use super::types::build_session_key;

/// dingtalk-stream 的消息回调处理器
#[allow(dead_code)]
pub(crate) struct DingTalkHandler {
    /// 发送消息到 listen() 的 Stream
    pub tx: mpsc::Sender<Message>,
    /// 允许的用户白名单
    pub allow_from: String,
    /// 群聊共享会话
    pub share_session: bool,
    /// 消息去重缓存
    pub seen: std::sync::Mutex<HashSet<String>>,
}

#[allow(dead_code)]
impl DingTalkHandler {
    /// 检查用户是否被授权
    pub fn is_authorized(&self, user_id: &str) -> bool {
        if self.allow_from == "*" {
            return true;
        }
        self.allow_from.split(',').any(|id| id.trim() == user_id)
    }

    /// 检查消息是否过期（超过 5 分钟视为过期）
    pub fn is_old_message(create_time_millis: i64) -> bool {
        let msg_time =
            chrono::DateTime::from_timestamp_millis(create_time_millis).unwrap_or_default();
        let now = Utc::now();
        (now - msg_time).num_minutes() > 5
    }

    /// 检查消息是否重复（已处理过）
    pub fn is_duplicate(&self, msg_id: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.contains(msg_id) {
            return true;
        }
        seen.insert(msg_id.to_string());
        if seen.len() > 4096 {
            seen.clear();
        }
        false
    }
}

/// 从钉钉消息报文 JSON 中提取纯文本内容
pub fn extract_text_content(payload_json: &str) -> String {
    let val: serde_json::Value = match serde_json::from_str(payload_json) {
        Ok(v) => v,
        Err(_) => return payload_json.to_string(),
    };

    // text: {"content": "..."}
    if let Some(text) = val.get("content").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return text.to_string();
        }
    }

    // richText: {"richText": {"blocks": [{"text": {"text": "..."}}, ...]}}
    if let Some(blocks) = val.pointer("/richText/blocks").and_then(|v| v.as_array()) {
        let parts: Vec<String> = blocks
            .iter()
            .filter_map(|block| {
                block
                    .pointer("/text/text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        if !parts.is_empty() {
            return parts.join("");
        }
    }

    payload_json.to_string()
}

/// 通用辅助：从 JSON 中尝试多条路径提取字符串值
pub fn json_str<'a>(root: &'a serde_json::Value, paths: &[&str]) -> Option<&'a str> {
    for path in paths {
        if let Some(v) = root.pointer(path).or_else(|| root.get(path)) {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

/// 从消息 JSON 中提取关键字段并构建 haimen Message
pub fn try_parse_message(
    data_json: &str,
    allow_from: &str,
    share_session: bool,
) -> Option<Message> {
    let root: serde_json::Value = serde_json::from_str(data_json).ok()?;

    let msg_id = json_str(
        &root,
        &["/data/msg_id", "/data/messageId", "/msgId", "/messageId"],
    )?
    .to_string();

    // senderStaffId 优先（钉钉 API 需要真实的 staffId）
    let sender_id = json_str(&root, &["/senderStaffId", "/data/senderStaffId"])
        .or_else(|| {
            json_str(
                &root,
                &[
                    "/data/sender/sender_id",
                    "/data/senderId",
                    "/senderId",
                    "/sender/sender_id",
                ],
            )
        })
        .unwrap_or("unknown")
        .to_string();

    let conversation_id = json_str(
        &root,
        &[
            "/data/conversation_id",
            "/data/conversationId",
            "/conversationId",
        ],
    )
    .unwrap_or("unknown")
    .to_string();

    let conversation_type = json_str(
        &root,
        &[
            "/data/conversation_type",
            "/data/conversationType",
            "/conversationType",
        ],
    )
    .unwrap_or("p2p")
    .to_string();

    // 钉钉 Stream 协议: conversationType = "1"(单聊) / "2"(群聊)
    // 统一转为 haimen 内部格式: "p2p" / "group"
    let conversation_type = match conversation_type.as_str() {
        "2" | "group" => "group".to_string(),
        _ => "p2p".to_string(),
    };

    let create_time: i64 = root
        .pointer("/data/create_at")
        .or_else(|| root.pointer("/data/createAt"))
        .or_else(|| root.get("createAt"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if DingTalkHandler::is_old_message(create_time) {
        tracing::debug!(msg_id, create_time, "丢弃过期消息");
        return None;
    }

    if allow_from != "*" {
        let authorized = allow_from.split(',').any(|id| id.trim() == sender_id);
        if !authorized {
            tracing::warn!(sender_id, "未授权的用户");
            return None;
        }
    }

    let content = root
        .pointer("/data/payload")
        .or_else(|| root.get("payload"))
        .or_else(|| root.pointer("/text/content"))
        .or_else(|| root.pointer("/text/text"))
        .map(|p| extract_text_content(&p.to_string()))
        .unwrap_or_default();

    let session_key = build_session_key(
        &conversation_id,
        &conversation_type,
        &sender_id,
        share_session,
    );

    Some(Message {
        id: msg_id,
        conversation_id: session_key,
        sender_id,
        content,
        timestamp: Utc::now(),
        channel: "dingtalk".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_text_type() {
        let result = extract_text_content(r#"{"content":"你好世界"}"#);
        assert_eq!(result, "你好世界");
    }

    #[test]
    fn test_extract_text_rich_text() {
        let json = r#"{
            "richText": {
                "blocks": [
                    {"text": {"text": "Hello "}},
                    {"text": {"text": "World"}}
                ]
            }
        }"#;
        assert_eq!(extract_text_content(json), "Hello World");
    }

    #[test]
    fn test_extract_text_plain_string() {
        assert_eq!(extract_text_content("hello world"), "hello world");
    }

    #[test]
    fn test_extract_text_empty() {
        assert_eq!(extract_text_content(""), "");
    }

    #[test]
    fn test_is_old_message_fresh() {
        let now = Utc::now().timestamp_millis();
        assert!(!DingTalkHandler::is_old_message(now));
    }

    #[test]
    fn test_is_old_message_expired() {
        let old = (Utc::now() - chrono::Duration::minutes(10)).timestamp_millis();
        assert!(DingTalkHandler::is_old_message(old));
    }

    fn make_handler(allow_from: &str) -> DingTalkHandler {
        let (tx, _rx) = mpsc::channel(16);
        DingTalkHandler {
            tx,
            allow_from: allow_from.to_string(),
            share_session: false,
            seen: std::sync::Mutex::new(HashSet::new()),
        }
    }

    #[test]
    fn test_is_authorized_wildcard() {
        let h = make_handler("*");
        assert!(h.is_authorized("any_user"));
    }

    #[test]
    fn test_is_authorized_single_user() {
        let h = make_handler("user123");
        assert!(h.is_authorized("user123"));
        assert!(!h.is_authorized("other_user"));
    }

    #[test]
    fn test_is_authorized_multiple_users() {
        let h = make_handler("user1,user2,user3");
        assert!(h.is_authorized("user1"));
        assert!(h.is_authorized("user3"));
        assert!(!h.is_authorized("user4"));
    }

    #[test]
    fn test_is_duplicate_first_call() {
        let h = make_handler("*");
        assert!(!h.is_duplicate("msg_001"));
    }

    #[test]
    fn test_is_duplicate_second_call() {
        let h = make_handler("*");
        h.is_duplicate("msg_001");
        assert!(h.is_duplicate("msg_001"));
    }

    #[test]
    fn test_json_str_found() {
        let val = serde_json::json!({"data": {"msg_id": "abc123"}});
        assert_eq!(json_str(&val, &["/data/msg_id"]), Some("abc123"));
    }

    #[test]
    fn test_json_str_fallback_path() {
        let val = serde_json::json!({"msgId": "abc123"});
        assert_eq!(json_str(&val, &["/data/msg_id", "/msgId"]), Some("abc123"));
    }

    #[test]
    fn test_json_str_not_found() {
        let val = serde_json::json!({"other": "value"});
        assert_eq!(json_str(&val, &["/data/msg_id", "/msgId"]), None);
    }

    #[test]
    fn test_try_parse_message_success() {
        let json = serde_json::json!({
            "data": {
                "msg_id": "msg_001",
                "sender": {"sender_id": "user123"},
                "conversation_id": "cid_abc",
                "conversation_type": "group",
                "create_at": Utc::now().timestamp_millis(),
                "payload": {"content": "你好"}
            }
        })
        .to_string();
        let msg = try_parse_message(&json, "*", false).unwrap();
        assert_eq!(msg.id, "msg_001");
        assert_eq!(msg.sender_id, "user123");
        assert_eq!(msg.content, "你好");
        assert_eq!(msg.channel, "dingtalk");
        assert!(msg.conversation_id.starts_with("dingtalk:g:cid_abc"));
    }

    #[test]
    fn test_try_parse_message_expired() {
        let data: serde_json::Value = serde_json::json!({
            "data": {
                "msg_id": "msg_002",
                "sender": {"sender_id": "user123"},
                "conversation_id": "cid_abc",
                "conversation_type": "group",
                "create_at": (Utc::now() - chrono::Duration::minutes(10)).timestamp_millis(),
                "payload": {"content": "过期消息"}
            }
        });
        let result = try_parse_message(&data.to_string(), "*", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_parse_message_unauthorized() {
        let json = serde_json::json!({
            "data": {
                "msg_id": "msg_003",
                "sender": {"sender_id": "unknown"},
                "conversation_id": "cid_abc",
                "conversation_type": "group",
                "create_at": Utc::now().timestamp_millis(),
                "payload": {"content": "hello"}
            }
        })
        .to_string();
        let result = try_parse_message(&json, "specific_user", false);
        assert!(result.is_none());
    }
}
