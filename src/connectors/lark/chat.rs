use super::bridge::LarkCliBridge;
use super::types::{ChatInfo, ChatListResponse};

/// 列出可访问的群聊
pub async fn list_chats(bridge: &LarkCliBridge) -> Result<Vec<ChatInfo>, String> {
    let value = bridge
        .exec(&["im", "+chat-list", "--as", "bot", "--format", "json"])
        .await?;

    // 从 data 字段中提取
    let data = value
        .get("data")
        .ok_or_else(|| "聊天列表响应缺少 data 字段".to_string())?;

    let response: ChatListResponse =
        serde_json::from_value(data.clone()).map_err(|e| format!("解析聊天列表失败: {}", e))?;

    Ok(response.chats.unwrap_or_default())
}
