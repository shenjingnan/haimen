use serde::{Deserialize, Serialize};

/// lark-cli 标准 JSON 响应包装
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LarkCliResponse<T> {
    pub ok: bool,
    pub identity: String,
    pub data: Option<T>,
    pub error: Option<LarkCliError>,
}

/// lark-cli 错误信息
#[derive(Debug, Deserialize)]
pub struct LarkCliError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: i64,
    pub message: String,
    pub log_id: Option<String>,
}

/// lark-cli event consume 输出的 NDJSON 事件行
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FeishuEvent {
    /// 消息 ID
    pub message_id: String,
    /// 聊天 ID
    pub chat_id: String,
    /// 聊天类型: "p2p" | "group"
    pub chat_type: String,
    /// 发送者 ID
    pub sender_id: String,
    /// 消息类型: "text", "image", "post" 等
    pub message_type: String,
    /// 消息内容（文本或 JSON）
    pub content: String,
    /// 创建时间（毫秒时间戳）
    pub create_time: String,
    /// 事件 ID
    pub event_id: Option<String>,
}

/// 认证状态
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub app_id: String,
    pub brand: String,
    pub identity: String,
    pub identities: AuthIdentities,
}

/// 认证身份集合
#[derive(Debug, Deserialize)]
pub struct AuthIdentities {
    pub bot: IdentityInfo,
    pub user: IdentityInfo,
}

/// 身份信息
#[derive(Debug, Deserialize)]
pub struct IdentityInfo {
    pub status: String,
    pub available: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
}

/// 群聊信息
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInfo {
    pub chat_id: String,
    pub name: Option<String>,
    pub chat_type: Option<String>,
    pub member_count: Option<i64>,
}

/// 聊天列表响应
#[derive(Debug, Deserialize)]
pub struct ChatListResponse {
    #[serde(default)]
    pub chats: Option<Vec<ChatInfo>>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub page_token: Option<String>,
}

/// 桥接健康状态
#[derive(Debug)]
pub struct BridgeHealth {
    pub lark_cli_found: bool,
    pub authenticated: bool,
    pub bot_ready: bool,
}
