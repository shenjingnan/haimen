use std::time::{Duration, Instant};

use reqwest::Client;
use tokio::sync::Mutex;

/// OAuth2 Access Token 管理器
///
/// 负责获取钉钉 API 的 access_token，缓存并在过期前 5 分钟刷新。
pub(crate) struct TokenManager {
    client_id: String,
    client_secret: String,
    cache: Mutex<Option<TokenCache>>,
    http_client: Client,
}

struct TokenCache {
    token: String,
    expires_at: Instant,
}

impl TokenCache {
    fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at - Duration::from_secs(300)
    }
}

impl TokenManager {
    pub fn new(client_id: String, client_secret: String, http_client: Client) -> Self {
        Self {
            client_id,
            client_secret,
            cache: Mutex::new(None),
            http_client,
        }
    }

    /// 获取有效的 access_token
    ///
    /// 使用 Mutex 保证临界区内完成"检查+请求"，避免并发请求风暴。
    pub async fn get_token(&self) -> Result<String, String> {
        let mut cache = self.cache.lock().await;

        if let Some(ref cached) = *cache {
            if cached.is_valid() {
                return Ok(cached.token.clone());
            }
        }

        let token = self.request_token().await?;

        let expires_at = Instant::now() + Duration::from_secs(7200);
        *cache = Some(TokenCache {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    /// 强制刷新 token（在收到 401 响应时调用）
    #[allow(dead_code)]
    pub async fn force_refresh(&self) -> Result<String, String> {
        *self.cache.lock().await = None;
        self.get_token().await
    }

    async fn request_token(&self) -> Result<String, String> {
        let body = serde_json::json!({
            "appKey": self.client_id,
            "appSecret": self.client_secret,
        });

        let resp = self
            .http_client
            .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("获取 access_token 失败: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("获取 access_token 错误 ({}): {}", status, text));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("解析 access_token 响应失败: {}", e))?;

        body["accessToken"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                let msg = body["message"].as_str().unwrap_or("未知错误");
                format!("获取 access_token 失败: {}", msg)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_cache_valid() {
        let cache = TokenCache {
            token: "token".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        assert!(cache.is_valid());
    }

    #[test]
    fn test_token_cache_expired() {
        let cache = TokenCache {
            token: "token".into(),
            expires_at: Instant::now() - Duration::from_secs(3600),
        };
        assert!(!cache.is_valid());
    }

    #[test]
    fn test_token_cache_about_to_expire() {
        let cache = TokenCache {
            token: "token".into(),
            expires_at: Instant::now() + Duration::from_secs(240),
        };
        assert!(!cache.is_valid());
    }

    #[test]
    fn test_token_cache_exactly_at_boundary() {
        let cache = TokenCache {
            token: "token".into(),
            expires_at: Instant::now() + Duration::from_secs(300),
        };
        assert!(!cache.is_valid());
    }
}
