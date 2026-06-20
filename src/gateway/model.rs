use chrono::{DateTime, Utc};

/// 平台无关的统一消息模型
///
/// 当前主要用于 IM 通道的文本消息。
/// 未来如果需要支持图片/文件/富媒体，可扩展 attachments 字段。
#[derive(Debug, Clone)]
pub struct Message {
    /// 平台内唯一消息 ID
    pub id: String,
    /// 会话标识（对应 chat_id / thread_id / conversation_id）
    pub conversation_id: String,
    /// 发送者 ID
    pub sender_id: String,
    /// 消息文本内容（纯文本，由各平台自行从原始格式转换）
    pub content: String,
    /// 消息时间戳
    pub timestamp: DateTime<Utc>,
    /// 来源通道名称（用于日志和调试）
    pub channel: String,
}

// 未来可扩展：
// pub struct Attachment {
//     pub mime_type: String,
//     pub url: String,
//     pub size: u64,
// }
