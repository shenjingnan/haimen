use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tracing;

type HmacSha256 = Hmac<Sha256>;

/// 验证 GitHub Webhook HMAC-SHA256 签名
///
/// GitHub 使用 HMAC-SHA256 对 payload 签名，签名放在 `x-hub-signature-256` header 中。
/// 格式: `sha256=<hex_digest>`
pub fn verify_signature(body: &[u8], signature_header: &str, secret: &str) -> Result<(), String> {
    let expected_prefix = "sha256=";
    let signature_hex = signature_header
        .strip_prefix(expected_prefix)
        .ok_or_else(|| format!("签名格式错误: 缺少 {} 前缀", expected_prefix))?;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("HMAC 初始化失败: {}", e))?;

    mac.update(body);

    let expected = mac.finalize().into_bytes();

    // 使用常量时间比较防止时序攻击
    let expected_hex = hex::encode(expected);

    // 常量时间比较
    if signature_hex.len() != expected_hex.len() {
        return Err("签名长度不匹配".to_string());
    }

    // 使用字节级常量时间比较
    let sig_bytes = signature_hex.as_bytes();
    let exp_bytes = expected_hex.as_bytes();
    let mut result = 0u8;
    for i in 0..sig_bytes.len() {
        result |= sig_bytes[i] ^ exp_bytes[i];
    }

    if result != 0 {
        return Err("签名验证失败".to_string());
    }

    Ok(())
}

/// 从 GitHub 评论中提取 @claude 之后的 prompt
///
/// 支持格式:
/// - `@claude fix this bug` → `Some("fix this bug")`
/// - `Hello @claude 分析一下这个 issue` → `Some("分析一下这个 issue")`
/// - 无 @claude → `None`
pub fn extract_mention(body: &str, mention: &str) -> Option<String> {
    let trimmed = body.trim();
    // 查找 mention 位置
    let mention_lower = mention.to_lowercase();
    let body_lower = trimmed.to_lowercase();

    let pos = body_lower.find(&mention_lower)?;

    // 提取 mention 之后的部分
    let after_mention = &trimmed[pos + mention.len()..];

    let prompt = after_mention.trim();

    if prompt.is_empty() {
        // @claude 后面没有内容,返回 None
        return None;
    }

    Some(prompt.to_string())
}

/// 通过 GitHub API 回复评论
pub async fn post_comment(token: &str, comments_url: &str, body: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("haimen-gateway")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let payload = serde_json::json!({ "body": body });

    let resp = client
        .post(comments_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github.v3+json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("发送评论失败: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub API 返回错误 ({}): {}", status, text));
    }

    tracing::info!(comments_url = %comments_url, "评论已发布");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_mention_simple() {
        let result = extract_mention("@claude fix this bug", "@claude");
        assert_eq!(result, Some("fix this bug".to_string()));
    }

    #[test]
    fn test_extract_mention_with_prefix_text() {
        let result = extract_mention("Hello @claude 分析一下这个 issue", "@claude");
        assert_eq!(result, Some("分析一下这个 issue".to_string()));
    }

    #[test]
    fn test_extract_mention_no_mention() {
        let result = extract_mention("帮我改代码", "@claude");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_mention_empty_after() {
        let result = extract_mention("@claude ", "@claude");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_mention_only_mention() {
        let result = extract_mention("@claude", "@claude");
        assert_eq!(result, None);
    }

    #[test]
    fn test_verify_signature_valid() {
        let secret = "my_secret";
        let body = b"hello world";
        // 手动构造 HMAC
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let digest = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={}", digest);

        assert!(verify_signature(body, &header, secret).is_ok());
    }

    #[test]
    fn test_verify_signature_invalid() {
        let secret = "my_secret";
        let body = b"hello world";
        let header = "sha256=0000000000000000000000000000000000000000000000000000000000000000";

        assert!(verify_signature(body, header, secret).is_err());
    }

    #[test]
    fn test_verify_signature_wrong_prefix() {
        let body = b"hello";
        let header = "md5=xxx";
        assert!(verify_signature(body, header, "secret").is_err());
    }
}
