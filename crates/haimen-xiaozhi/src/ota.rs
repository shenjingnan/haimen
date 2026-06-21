use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::Json,
    http::{HeaderMap, StatusCode},
};
use chrono::Local;

use crate::types::*;

/// 处理 POST /xiaozhi/ota/ 请求
///
/// 从请求头中提取 Device-Id，构建包含 WebSocket URL、服务器时间、
/// 固件信息和音频参数的 OTA 响应。
pub async fn handle_ota(
    headers: HeaderMap,
    Json(_body): Json<OtaRequest>,
) -> Result<Json<OtaResponse>, (StatusCode, Json<serde_json::Value>)> {
    // 验证 Device-Id
    let _device_id = headers
        .get("device-id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            let err = serde_json::json!({"error": "缺少 Device-Id 请求头"});
            (StatusCode::BAD_REQUEST, Json(err))
        })?;

    let ws_url = build_ws_url(&headers);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let timezone_offset = Local::now().offset().local_minus_utc() / 60;

    let response = OtaResponse {
        websocket: WebsocketInfo {
            url: ws_url,
            token: String::new(),
            version: 2,
        },
        server_time: ServerTime {
            timestamp: now.as_millis() as i64,
            timezone_offset,
        },
        firmware: None,
        audio_params: AudioParams::default(),
    };

    Ok(Json(response))
}

/// 从请求头构建 WebSocket URL
///
/// 优先级：
/// 1. `X-Forwarded-Proto` + `X-Forwarded-Host`（反向代理场景）
/// 2. `Host` 请求头（直接访问）
/// 3. 回退到 `ws://localhost:9527/xiaozhi/ws`
fn build_ws_url(headers: &HeaderMap) -> String {
    // 判断是否使用安全协议
    let is_secure = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s == "https" || s == "wss")
        .unwrap_or(false);

    let scheme = if is_secure { "wss" } else { "ws" };

    let host = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .or_else(|| headers.get("host").and_then(|v| v.to_str().ok()))
        .unwrap_or("localhost:9527");

    format!("{}://{}/xiaozhi/ws", scheme, host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    /// 构建一个包含 Device-Id 头的最小请求
    fn make_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("device-id", HeaderValue::from_static("AA:BB:CC:DD:EE:FF"));
        headers
    }

    #[tokio::test]
    async fn test_missing_device_id() {
        let headers = HeaderMap::new();
        let body = OtaRequest {
            version: 2,
            mac_address: "AA:BB:CC:DD:EE:FF".to_string(),
            uuid: "test-uuid".to_string(),
            chip_model_name: None,
            application: None,
            board: None,
            flash_size: None,
            minimum_free_heap_size: None,
            ota: None,
        };

        let result = handle_ota(headers, Json(body)).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_successful_ota() {
        let headers = make_headers();
        let body = OtaRequest {
            version: 2,
            mac_address: "AA:BB:CC:DD:EE:FF".to_string(),
            uuid: "test-uuid".to_string(),
            chip_model_name: None,
            application: None,
            board: None,
            flash_size: None,
            minimum_free_heap_size: None,
            ota: None,
        };

        let result = handle_ota(headers, Json(body)).await;
        assert!(result.is_ok());

        let response = result.unwrap().0;
        assert!(response.websocket.url.contains("ws://"));
        assert!(response.websocket.url.contains("/xiaozhi/ws"));
        assert_eq!(response.websocket.version, 2);
        assert_eq!(response.audio_params.format, "opus");
        assert_eq!(response.audio_params.sample_rate, 24000);
    }

    #[tokio::test]
    async fn test_ota_with_host_header() {
        let mut headers = make_headers();
        headers.insert("host", HeaderValue::from_static("example.com:8080"));

        let body = OtaRequest {
            version: 2,
            mac_address: "AA:BB:CC:DD:EE:FF".to_string(),
            uuid: "test-uuid".to_string(),
            chip_model_name: None,
            application: None,
            board: None,
            flash_size: None,
            minimum_free_heap_size: None,
            ota: None,
        };

        let result = handle_ota(headers, Json(body)).await;
        assert!(result.is_ok());

        let response = result.unwrap().0;
        assert!(response.websocket.url.contains("example.com:8080"));
    }

    #[tokio::test]
    async fn test_ota_with_forwarded_headers() {
        let mut headers = make_headers();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("public.example.com"),
        );

        let body = OtaRequest {
            version: 2,
            mac_address: "AA:BB:CC:DD:EE:FF".to_string(),
            uuid: "test-uuid".to_string(),
            chip_model_name: None,
            application: None,
            board: None,
            flash_size: None,
            minimum_free_heap_size: None,
            ota: None,
        };

        let result = handle_ota(headers, Json(body)).await;
        assert!(result.is_ok());

        let response = result.unwrap().0;
        // X-Forwarded 优先于 Host
        assert!(response.websocket.url.contains("wss://"));
        assert!(response.websocket.url.contains("public.example.com"));
    }

    #[tokio::test]
    async fn test_server_time_fields() {
        let headers = make_headers();
        let body = OtaRequest {
            version: 2,
            mac_address: "AA:BB:CC:DD:EE:FF".to_string(),
            uuid: "test-uuid".to_string(),
            chip_model_name: None,
            application: None,
            board: None,
            flash_size: None,
            minimum_free_heap_size: None,
            ota: None,
        };

        let result = handle_ota(headers, Json(body)).await;
        assert!(result.is_ok());

        let response = result.unwrap().0;
        // timestamp 应该是正数（Unix 毫秒时间戳）
        assert!(response.server_time.timestamp > 1_700_000_000_000i64);
        // timezone_offset 应该在 -720 到 840 之间（合理范围）
        assert!(response.server_time.timezone_offset >= -720);
        assert!(response.server_time.timezone_offset <= 840);
    }
}
