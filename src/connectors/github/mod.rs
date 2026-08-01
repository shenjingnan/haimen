pub mod config;
pub mod handler;
pub mod types;

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;
use tracing;

use crate::gateway::provider::AgentProvider;
use crate::gateway::webhook::{WebhookHandler, WebhookResult};

pub use config::GitHubConfig;

/// GitHub Webhook 连接器
pub struct GitHubConnector {
    config: GitHubConfig,
    /// 使用 Arc 而非 Box：handle() 中需要 clone 到 tokio::spawn 的异步闭包
    agent: Arc<dyn AgentProvider>,
    /// Agent 子进程工作目录
    work_dir: String,
    /// Webhook 幂等性去重缓存（最多保留 1000 条 delivery_id）
    dedup: DedupCache,
}

/// 简单的 LRU 去重缓存
struct DedupCache {
    seen: std::sync::Mutex<Vec<String>>,
    max_size: usize,
}

impl DedupCache {
    fn new(max_size: usize) -> Self {
        Self {
            seen: std::sync::Mutex::new(Vec::with_capacity(max_size)),
            max_size,
        }
    }

    /// 返回 true 表示未处理过（通过检查），false 表示已处理过（跳过）
    fn try_check(&self, id: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.contains(&id.to_string()) {
            return false;
        }
        seen.push(id.to_string());
        if seen.len() > self.max_size {
            seen.remove(0);
        }
        true
    }
}

impl GitHubConnector {
    pub fn new(config: GitHubConfig, agent: Arc<dyn AgentProvider>, work_dir: String) -> Self {
        Self {
            config,
            agent,
            work_dir,
            dedup: DedupCache::new(1000),
        }
    }

    /// 验证 GitHub HMAC-SHA256 签名
    fn verify_signature(&self, body: &[u8], signature_header: &str) -> Result<(), String> {
        handler::verify_signature(body, signature_header, &self.config.webhook_secret)
    }
}

#[async_trait]
impl WebhookHandler for GitHubConnector {
    fn name(&self) -> &str {
        "github"
    }

    async fn handle(&self, body: &[u8], headers: &HeaderMap) -> Result<WebhookResult, String> {
        // === 同步阶段：验证 + 前置检查（必须在 10 秒内返回 200）===

        // 1. 验证签名
        let sig = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "缺少 x-hub-signature-256 header".to_string())?;
        self.verify_signature(body, sig)?;

        // 2. 幂等性检查：跳过已处理的 delivery
        let delivery_id = headers
            .get("x-github-delivery")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        if !self.dedup.try_check(delivery_id) {
            tracing::debug!(delivery_id = %delivery_id, "跳过已处理的 webhook delivery");
            return Ok(WebhookResult { triggered: false });
        }

        // 3. 解析事件类型，只处理 IssueCommentEvent
        let event_type = headers
            .get("x-github-event")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if event_type != "issue_comment" {
            return Ok(WebhookResult { triggered: false });
        }

        // 4. 解析 JSON → 检测 @claude → 提取 prompt
        let parsed: types::IssueCommentEvent = match serde_json::from_slice(body) {
            Ok(e) => e,
            Err(_) => return Ok(WebhookResult { triggered: false }),
        };

        let comment_body = match &parsed.comment.body {
            Some(b) => b.as_str(),
            None => return Ok(WebhookResult { triggered: false }),
        };

        let prompt = match handler::extract_mention(comment_body, "@claude") {
            Some(p) => p,
            None => return Ok(WebhookResult { triggered: false }),
        };

        tracing::info!(
            delivery_id = %delivery_id,
            issue = %parsed.issue.number,
            prompt_len = prompt.len(),
            "检测到 @claude 提及，开始异步处理"
        );

        // === 异步阶段：后台处理，不阻塞 HTTP 响应 ===
        let agent = self.agent.clone();
        let token = self.config.token.clone();
        let issue = parsed.issue.clone();
        let work_dir = self.work_dir.clone();
        tokio::spawn(async move {
            // 5. 获取 Issue 上下文
            let context = format!(
                "Issue #{}\nTitle: {}\nBody: {}",
                issue.number,
                issue.title,
                issue.body.as_deref().unwrap_or("(无描述)"),
            );

            // 6. 调用 Agent
            match agent
                .process(
                    &format!("Context:\n{}\n\nRequest:\n{}", context, prompt),
                    None,
                    &work_dir,
                )
                .await
            {
                Ok((response, _session_id)) => {
                    // 7. 通过 GitHub API 回复评论
                    if let Err(e) =
                        handler::post_comment(&token, &issue.comments_url, &response).await
                    {
                        tracing::error!(error = %e, "GitHub 回复评论失败");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Agent 处理失败");
                    let msg = format!("❌ @claude 处理失败: {}", e);
                    let _ = handler::post_comment(&token, &issue.comments_url, &msg).await;
                }
            }
        });

        Ok(WebhookResult { triggered: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_cache_new_entry() {
        let cache = DedupCache::new(3);
        assert!(cache.try_check("id1"));
    }

    #[test]
    fn test_dedup_cache_duplicate() {
        let cache = DedupCache::new(3);
        assert!(cache.try_check("id1"));
        assert!(!cache.try_check("id1"));
    }

    #[test]
    fn test_dedup_cache_eviction() {
        let cache = DedupCache::new(3);
        assert!(cache.try_check("id1"));
        assert!(cache.try_check("id2"));
        assert!(cache.try_check("id3"));
        assert!(cache.try_check("id4")); // 驱逐 id1
        // id1 已被驱逐，应该返回 true
        assert!(cache.try_check("id1"));
    }
}
