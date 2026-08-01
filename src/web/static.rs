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

#[cfg(test)]
mod tests {
    use super::*;

    /// 校验 rust-embed 确实嵌入了完整的前端产物（而非占位/空产物）。
    /// 占位 index.html 引用的资源未被嵌入时，本测试失败。
    #[test]
    fn web_assets_are_embedded() {
        let files: Vec<String> = Assets::iter().map(|s| s.to_string()).collect();

        assert!(
            files.iter().any(|f| f == "index.html"),
            "index.html 必须被嵌入"
        );

        let html = Assets::get("index.html").expect("index.html 应被嵌入").data;
        let html = String::from_utf8_lossy(&html).to_string();
        assert!(
            html.contains("assets/"),
            "index.html 必须引用 /assets/* 资源"
        );

        let js_assets = files
            .iter()
            .filter(|f| f.starts_with("assets/") && f.ends_with(".js"))
            .count();
        assert!(
            js_assets >= 1,
            "必须嵌入至少一个 JS 资源（占位/空产物会导致 Web 控制台白屏）"
        );
    }
}
