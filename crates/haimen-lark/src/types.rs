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
    pub message_id: String,
    pub chat_id: String,
    pub chat_type: String,
    pub sender_id: String,
    pub message_type: String,
    pub content: String,
    pub create_time: String,
    pub event_id: Option<String>,
}

/// 桥接健康状态
#[derive(Debug)]
pub struct BridgeHealth {
    pub lark_cli_found: bool,
    pub authenticated: bool,
    pub bot_ready: bool,
}
