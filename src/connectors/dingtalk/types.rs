/// 钉钉消息类型
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum DingTalkMsgType {
    Text,
    RichText,
    Picture,
    Audio,
    File,
    Video,
    Unknown,
}

impl DingTalkMsgType {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "text" => Self::Text,
            "richText" => Self::RichText,
            "picture" => Self::Picture,
            "audio" => Self::Audio,
            "file" => Self::File,
            "video" => Self::Video,
            _ => Self::Unknown,
        }
    }
}

/// 会话类型
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationType {
    Group,
    Single,
}

impl ConversationType {
    pub fn prefix(&self) -> &str {
        match self {
            Self::Group => "g",
            Self::Single => "d",
        }
    }
}

/// 会话键构建器
///
/// 格式:
///   群聊共享: dingtalk:g:{openConversationId}
///   群聊隔离: dingtalk:g:{openConversationId}:{senderId}
///   单聊:     dingtalk:d:{senderId}
pub fn build_session_key(
    conversation_id: &str,
    conversation_type: &str,
    sender_id: &str,
    share_session: bool,
) -> String {
    let conv_type = match conversation_type {
        "group" => ConversationType::Group,
        _ => ConversationType::Single,
    };

    let prefix = format!("dingtalk:{}:{}", conv_type.prefix(), conversation_id);

    if share_session && conv_type == ConversationType::Group {
        prefix
    } else {
        format!("{}:{}", prefix, sender_id)
    }
}

/// 从 session key 解析目标标识（用于 send）
///
/// 格式:
///   群聊共享:  dingtalk:g:{openConversationId}
///   群聊隔离:  dingtalk:g:{openConversationId}:{senderId}
///   单聊:      dingtalk:d:{conversationId}:{senderId}
///
/// 返回: (conv_type, target_id)
///   conv_type = "g" -> target_id 是 openConversationId（群聊 groupMessages/send API）
///   conv_type = "d" -> target_id 是 senderId（单聊 oToMessages/batchSend API 需要 userIds）
pub fn parse_target_from_session_key(session_key: &str) -> (&str, &str) {
    let parts: Vec<&str> = session_key.splitn(4, ':').collect();
    let conv_type = parts.get(1).copied().unwrap_or("d");
    match conv_type {
        "g" => {
            // dingtalk:g:{openConversationId}[:{senderId}]
            let conversation_id = parts.get(2).copied().unwrap_or("");
            (conv_type, conversation_id)
        }
        _ => {
            // dingtalk:d:{conversationId}:{senderId}
            // oToMessages/batchSend 需要 userIds，所以取 senderId
            let sender_id = parts.get(3).copied().unwrap_or("");
            (conv_type, sender_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msg_type_all_variants() {
        assert_eq!(DingTalkMsgType::from_str("text"), DingTalkMsgType::Text);
        assert_eq!(
            DingTalkMsgType::from_str("richText"),
            DingTalkMsgType::RichText
        );
        assert_eq!(
            DingTalkMsgType::from_str("picture"),
            DingTalkMsgType::Picture
        );
        assert_eq!(DingTalkMsgType::from_str("audio"), DingTalkMsgType::Audio);
        assert_eq!(DingTalkMsgType::from_str("file"), DingTalkMsgType::File);
        assert_eq!(DingTalkMsgType::from_str("video"), DingTalkMsgType::Video);
    }

    #[test]
    fn test_msg_type_unknown() {
        assert_eq!(
            DingTalkMsgType::from_str("unknown_type"),
            DingTalkMsgType::Unknown
        );
        assert_eq!(DingTalkMsgType::from_str(""), DingTalkMsgType::Unknown);
        assert_eq!(DingTalkMsgType::from_str("  "), DingTalkMsgType::Unknown);
    }

    #[test]
    fn test_build_session_key_group_shared() {
        assert_eq!(
            build_session_key("cid123", "group", "uid456", true),
            "dingtalk:g:cid123"
        );
    }

    #[test]
    fn test_build_session_key_group_isolated() {
        assert_eq!(
            build_session_key("cid123", "group", "uid456", false),
            "dingtalk:g:cid123:uid456"
        );
    }

    #[test]
    fn test_build_session_key_single() {
        // 单聊: dingtalk:d:{conversationId}:{senderId}
        assert_eq!(
            build_session_key("cid789", "p2p", "uid111", false),
            "dingtalk:d:cid789:uid111"
        );
    }

    #[test]
    fn test_parse_target_group() {
        let (t, id) = parse_target_from_session_key("dingtalk:g:cid123");
        assert_eq!(t, "g");
        assert_eq!(id, "cid123");
    }

    #[test]
    fn test_parse_target_group_isolated() {
        let (t, id) = parse_target_from_session_key("dingtalk:g:cid123:uid456");
        assert_eq!(t, "g");
        assert_eq!(id, "cid123");
    }

    #[test]
    fn test_parse_target_single() {
        // 单聊: dingtalk:d:{conversationId}:{senderId} → target = senderId (parts[3])
        let (t, id) = parse_target_from_session_key("dingtalk:d:cid789:uid111");
        assert_eq!(t, "d");
        assert_eq!(id, "uid111");
    }

    #[test]
    fn test_parse_target_malformed_empty() {
        let (t, id) = parse_target_from_session_key("");
        assert_eq!(t, "d");
        assert_eq!(id, "");
    }

    #[test]
    fn test_parse_target_malformed_wrong_prefix() {
        let (t, id) = parse_target_from_session_key("dingtalk:x:cid:uid");
        assert_eq!(t, "x");
        assert_eq!(id, "uid");
    }

    #[test]
    fn test_build_session_key_empty_sender() {
        assert_eq!(
            build_session_key("cid123", "group", "", true),
            "dingtalk:g:cid123"
        );
    }

    #[test]
    fn test_parse_target_malformed_too_short() {
        let (t, id) = parse_target_from_session_key("dingtalk");
        assert_eq!(t, "d");
        assert_eq!(id, "");
    }
}
