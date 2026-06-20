use serde::{Deserialize, Serialize};

/// GitHub Webhook 配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitHubConfig {
    /// Webhook 密钥（用于验证 HMAC-SHA256 签名）
    pub webhook_secret: String,
    /// GitHub Personal Access Token（用于通过 API 回复评论）
    pub token: String,
}
