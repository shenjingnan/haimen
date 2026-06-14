use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::Response;
use rust_embed::RustEmbed;

/// 嵌入的 Web UI 静态资源
#[derive(RustEmbed)]
#[folder = "web-ui/dist"]
struct Assets;

/// 获取 MIME 类型
fn mime_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    }
}

/// 处理静态文件请求
/// 对于非 API 路径，先尝试查找嵌入文件，找不到则返回 index.html（SPA fallback）
pub async fn handle(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // 先尝试按路径查找
    if let Some(asset) = Assets::get(path) {
        return Response::builder()
            .header(header::CONTENT_TYPE, mime_type(path))
            .status(StatusCode::OK)
            .body(Body::from(asset.data.to_vec()))
            .unwrap_or_else(|_| internal_error());
    }

    // SPA fallback: 返回 index.html
    if let Some(asset) = Assets::get("index.html") {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .status(StatusCode::OK)
            .body(Body::from(asset.data.to_vec()))
            .unwrap_or_else(|_| internal_error());
    }

    internal_error()
}

fn internal_error() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("内部错误"))
        .unwrap()
}
