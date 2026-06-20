use std::pin::Pin;

use async_trait::async_trait;
use futures_util::Stream;

use crate::gateway::model::Message;

/// IM 消息通道抽象
///
/// 所有即时通讯通道（飞书、Telegram、Discord 等）实现此 trait。
/// - listen() 返回消息流
/// - send() 发送回复
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
