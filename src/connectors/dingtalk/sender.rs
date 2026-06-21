use reqwest::Client;

use super::token::TokenManager;

/// 钉钉消息发送器
///
/// 与 TokenManager 共享同一个 reqwest::Client，复用连接池。
pub(crate) struct DingTalkSender {
    client: Client,
    token_manager: TokenManager,
    robot_code: String,
}

impl DingTalkSender {
    pub fn new(
        client: Client,
        client_id: String,
        client_secret: String,
        robot_code: String,
    ) -> Self {
        let token_manager = TokenManager::new(client_id, client_secret, client.clone());
        Self {
            client,
            token_manager,
            robot_code,
        }
    }

    /// 发送消息到钉钉
    ///
    /// target: 群聊=openConversationId, 单聊=senderId
    pub async fn send(&self, target: &str, is_group: bool, message: &str) -> Result<(), String> {
        let token = self.token_manager.get_token().await?;

        if is_group {
            self.send_group_message(&token, target, message).await
        } else {
            self.send_single_message(&token, target, message).await
        }
    }

    async fn send_group_message(
        &self,
        token: &str,
        group_id: &str,
        message: &str,
    ) -> Result<(), String> {
        let msg_param = serde_json::json!({
            "title": "haimen AI 回复",
            "text": message,
        });
        let body = serde_json::json!({
            "robotCode": self.robot_code,
            "openConversationId": group_id,
            "msgKey": "sampleMarkdown",
            "msgParam": msg_param.to_string(),
        });

        let resp = self
            .client
            .post("https://api.dingtalk.com/v1.0/robot/groupMessages/send")
            .header("x-acs-dingtalk-access-token", token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("发送群消息请求失败: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("发送群消息失败 ({}): {}", status, text));
        }

        Ok(())
    }

    async fn send_single_message(
        &self,
        token: &str,
        user_id: &str,
        message: &str,
    ) -> Result<(), String> {
        let msg_param = serde_json::json!({
            "title": "haimen AI 回复",
            "text": message,
        });
        let body = serde_json::json!({
            "robotCode": self.robot_code,
            "userIds": [user_id],
            "msgKey": "sampleMarkdown",
            "msgParam": msg_param.to_string(),
        });

        let resp = self
            .client
            .post("https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend")
            .header("x-acs-dingtalk-access-token", token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("发送单聊消息请求失败: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("发送单聊消息失败 ({}): {}", status, text));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sender_construct_with_client() {
        let client = Client::new();
        let sender = DingTalkSender::new(
            client,
            "client_id".into(),
            "client_secret".into(),
            "".into(),
        );
        // robot_code 存储原始值，为空时由 DingTalkConfig::effective_robot_code() 处理
        assert_eq!(sender.robot_code, "");
    }
}
