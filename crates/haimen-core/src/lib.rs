use std::pin::Pin;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::Stream;

/// 统一消息模型
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

/// 消息通道抽象
#[async_trait]
pub trait MessageChannel: Send + Sync {
    /// 通道名称
    fn name(&self) -> &str;
    /// 启动监听，返回消息流
    async fn listen(&self) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String>;
    /// 发送消息到指定会话
    async fn send(&self, conversation_id: &str, message: &str) -> Result<(), String>;
    /// 健康检查
    async fn health_check(&self) -> Result<(), String>;
}
