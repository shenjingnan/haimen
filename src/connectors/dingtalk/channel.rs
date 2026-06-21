use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::Stream;
use reqwest::Client;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing;

use crate::gateway::channel::MessageChannel;
use crate::gateway::model::Message;

use super::config::DingTalkConfig;
use super::handler::try_parse_message;
use super::sender::DingTalkSender;
use super::types::parse_target_from_session_key;

/// 钉钉消息通道
pub struct DingTalkChannel {
    config: DingTalkConfig,
    client: Client,
    sender: OnceLock<DingTalkSender>,
    cancel: CancellationToken,
}

impl DingTalkChannel {
    pub fn new(config: DingTalkConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest Client 创建失败");
        Self {
            config,
            client,
            sender: OnceLock::new(),
            cancel: CancellationToken::new(),
        }
    }

    fn get_or_init_sender(&self) -> &DingTalkSender {
        self.sender.get_or_init(|| {
            let resolved = self.config.resolve_env_refs().unwrap_or_else(|e| {
                panic!("钉钉配置解析失败: {e}。请检查 settings.toml 中的 [dingtalk] 配置或设置环境变量。")
            });
            DingTalkSender::new(
                self.client.clone(),
                resolved.client_id,
                resolved.client_secret,
                self.config.effective_robot_code().to_string(),
            )
        })
    }
}

impl Drop for DingTalkChannel {
    fn drop(&mut self) {
        self.cancel.cancel();
        tracing::debug!("DingTalkChannel 已关闭，已发送取消信号");
    }
}

/// 桥接 dingtalk-stream 的 CallbackHandler 到 haimen 的 Message 流
struct DingTalkBridge {
    tx: mpsc::Sender<Message>,
    allow_from: String,
    share_session: bool,
    seen: std::sync::Mutex<std::collections::HashSet<String>>,
}

#[async_trait]
impl dingtalk_stream::CallbackHandler for DingTalkBridge {
    async fn process(&self, callback_message: &dingtalk_stream::MessageBody) -> (u16, String) {
        // MessageBody.data 是 chatbot 消息的 JSON 字符串
        let root: serde_json::Value = match serde_json::from_str(&callback_message.data) {
            Ok(v) => v,
            Err(_) => return (200, "OK".to_string()),
        };

        // 提取 msg_id 做去重
        let msg_id = root
            .get("msgId")
            .or_else(|| root.get("messageId"))
            .and_then(|v| v.as_str())
            .map(String::from);

        if let Some(ref id) = msg_id {
            let mut seen = self.seen.lock().unwrap();
            if seen.contains(id) {
                return (200, "OK".to_string());
            }
            seen.insert(id.clone());
            if seen.len() > 4096 {
                seen.clear();
            }
        }

        // chatbot 消息数据在 callback_message.data 中（已经是 JSON 字符串）
        // try_parse_message 支持查找 /msgId 等扁平路径
        if let Some(msg) =
            try_parse_message(&callback_message.data, &self.allow_from, self.share_session)
        {
            if let Err(e) = self.tx.send(msg).await {
                tracing::error!(error = %e, "消息队列发送失败");
            }
        }

        (200, "OK".to_string())
    }
}

#[async_trait]
impl MessageChannel for DingTalkChannel {
    fn name(&self) -> &str {
        "dingtalk"
    }

    async fn listen(&self) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String> {
        self.config.validate().map_err(|errors| errors.join("; "))?;
        let resolved = self
            .config
            .resolve_env_refs()
            .map_err(|e| format!("配置解析失败: {}", e))?;

        let (tx, rx) = mpsc::channel::<Message>(256);

        let client_id = resolved.client_id.clone();
        let client_secret = resolved.client_secret.clone();
        let allow_from = resolved.allow_from.clone();
        let share_session = self.config.share_session_in_channel;
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            let credential = dingtalk_stream::Credential::new(client_id, client_secret);

            let bridge = DingTalkBridge {
                tx: tx.clone(),
                allow_from: allow_from.clone(),
                share_session,
                seen: std::sync::Mutex::new(std::collections::HashSet::new()),
            };

            let mut client = dingtalk_stream::DingTalkStreamClient::builder(credential)
                .register_callback_handler(dingtalk_stream::ChatbotMessage::TOPIC, bridge)
                .build();

            tracing::info!("钉钉 Stream 客户端已启动");

            // start() 内部有重连循环，用 select! 支持取消
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("钉钉 Stream 客户端收到取消信号");
                }
                result = client.start() => {
                    match result {
                        Ok(()) => tracing::info!("钉钉 Stream 客户端正常退出"),
                        Err(e) => tracing::error!(error = %e, "钉钉 Stream 客户端异常退出"),
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn send(&self, conversation_id: &str, message: &str) -> Result<(), String> {
        let sender = self.get_or_init_sender();
        let (conv_type, target) = parse_target_from_session_key(conversation_id);
        let is_group = conv_type == "g";

        tracing::info!(
            target = %target,
            is_group = is_group,
            message_len = message.len(),
            "发送钉钉消息"
        );

        sender.send(target, is_group, message).await
    }

    async fn health_check(&self) -> Result<(), String> {
        self.config.validate().map_err(|errors| errors.join("; "))?;

        tracing::info!("钉钉通道健康检查通过");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let channel = DingTalkChannel::new(DingTalkConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            ..Default::default()
        });
        assert_eq!(channel.name(), "dingtalk");
    }

    #[tokio::test]
    async fn test_health_check_ok() {
        let channel = DingTalkChannel::new(DingTalkConfig {
            client_id: "valid_id".into(),
            client_secret: "valid_secret".into(),
            ..Default::default()
        });
        assert!(channel.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_health_check_missing_client_id() {
        let channel = DingTalkChannel::new(DingTalkConfig::default());
        let err = channel.health_check().await.unwrap_err();
        assert!(err.contains("client_id"));
    }

    #[test]
    fn test_drop_triggers_cancel() {
        let channel = DingTalkChannel::new(DingTalkConfig::default());
        let cancel = channel.cancel.clone();
        assert!(!cancel.is_cancelled());
        drop(channel);
        assert!(cancel.is_cancelled());
    }
}
