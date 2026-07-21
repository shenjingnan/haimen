use serde::Deserialize;

/// dws event consume 输出的 NDJSON 事件行
///
/// 格式参考 dws 官方文档: `dws event schema <event_key>`
/// 当前支持的事件类型: user_im_message_receive_group, user_im_message_receive_o2o, user_im_message_receive_at
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DingTalkEvent {
    /// 事件唯一 ID（可用于去重）
    pub event_id: String,
    /// 事件类型（如 "user_im_message_receive_group"）
    pub event_type: String,
    /// 事件数据
    pub data: EventData,
}

/// 钉钉消息事件数据体
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EventData {
    /// 会话 ID（openConversationId）
    pub conversation_id: String,
    /// 会话类型: "group" / "p2p"
    pub conversation_type: String,
    /// 消息 ID
    pub msg_id: String,
    /// 发送者 ID（staffId）
    pub sender_id: String,
    /// 消息文本内容
    pub text: EventText,
    /// 消息创建时间（毫秒时间戳）
    pub create_at: i64,
}

/// 消息文本内容
#[derive(Debug, Deserialize)]
pub struct EventText {
    /// 文本内容
    pub content: String,
}

/// 桥接健康状态
#[derive(Debug)]
pub struct BridgeHealth {
    /// dws CLI 是否存在
    pub dws_found: bool,
    /// 是否已认证
    pub authenticated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_group_event() {
        let json = r#"{
            "event_id": "evt_001",
            "event_type": "user_im_message_receive_group",
            "data": {
                "conversation_id": "cid_abc123",
                "conversation_type": "group",
                "msg_id": "msg_001",
                "sender_id": "user_456",
                "text": {
                    "content": "你好，AI 助手"
                },
                "create_at": 1712345678000
            }
        }"#;
        let event: DingTalkEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_id, "evt_001");
        assert_eq!(event.event_type, "user_im_message_receive_group");
        assert_eq!(event.data.conversation_id, "cid_abc123");
        assert_eq!(event.data.conversation_type, "group");
        assert_eq!(event.data.msg_id, "msg_001");
        assert_eq!(event.data.sender_id, "user_456");
        assert_eq!(event.data.text.content, "你好，AI 助手");
        assert_eq!(event.data.create_at, 1712345678000);
    }

    #[test]
    fn test_deserialize_single_event() {
        let json = r#"{
            "event_id": "evt_002",
            "event_type": "user_im_message_receive_o2o",
            "data": {
                "conversation_id": "cid_xyz",
                "conversation_type": "p2p",
                "msg_id": "msg_002",
                "sender_id": "user_789",
                "text": {
                    "content": "在吗？"
                },
                "create_at": 1712345680000
            }
        }"#;
        let event: DingTalkEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "user_im_message_receive_o2o");
        assert_eq!(event.data.conversation_type, "p2p");
        assert_eq!(event.data.text.content, "在吗？");
    }

    #[test]
    fn test_deserialize_at_event() {
        let json = r#"{
            "event_id": "evt_003",
            "event_type": "user_im_message_receive_at",
            "data": {
                "conversation_id": "cid_group_01",
                "conversation_type": "group",
                "msg_id": "msg_003",
                "sender_id": "user_111",
                "text": {
                    "content": "@我 帮我查一下天气"
                },
                "create_at": 1712345690000
            }
        }"#;
        let event: DingTalkEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.data.text.content, "@我 帮我查一下天气");
    }

    #[test]
    fn test_deserialize_missing_fields() {
        let json = r#"{
            "event_id": "",
            "event_type": "unknown_event",
            "data": {
                "conversation_id": "",
                "conversation_type": "",
                "msg_id": "",
                "sender_id": "",
                "text": {
                    "content": ""
                },
                "create_at": 0
            }
        }"#;
        let event: DingTalkEvent = serde_json::from_str(json).unwrap();
        assert!(event.event_id.is_empty());
        assert_eq!(event.data.create_at, 0);
    }

    #[test]
    fn test_deserialize_malformed_missing_data() {
        let json = r#"{"event_id": "evt_004"}"#;
        let result: Result<DingTalkEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_empty_json() {
        let result: Result<DingTalkEvent, _> = serde_json::from_str(r#"{}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_bridge_health_all_ok() {
        let health = BridgeHealth {
            dws_found: true,
            authenticated: true,
        };
        assert!(health.dws_found);
        assert!(health.authenticated);
    }

    #[test]
    fn test_bridge_health_not_installed() {
        let health = BridgeHealth {
            dws_found: false,
            authenticated: false,
        };
        assert!(!health.dws_found);
        assert!(!health.authenticated);
    }

    #[test]
    fn test_event_data_text_content_with_special_chars() {
        let json = r#"{
            "event_id": "evt_005",
            "event_type": "user_im_message_receive_group",
            "data": {
                "conversation_id": "cid_test",
                "conversation_type": "group",
                "msg_id": "msg_005",
                "sender_id": "user_test",
                "text": {
                    "content": "Hello\nWorld\t!&"
                },
                "create_at": 1712345700000
            }
        }"#;
        let event: DingTalkEvent = serde_json::from_str(json).unwrap();
        assert!(event.data.text.content.contains("Hello"));
    }
}
