use axum::Json;
use serde_json::Value;

/// GET /api/v1/system/info
pub async fn info() -> Json<Value> {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "service": "haimen",
        "uptime": null,
    }))
}
