use std::path::Path;

fn main() {
    // 确保 web-ui/dist 目录存在，避免 rust-embed 编译错误
    let dist_dir = Path::new("web-ui/dist");
    if !dist_dir.exists() {
        std::fs::create_dir_all(dist_dir).expect("无法创建 web-ui/dist 目录");
        // 创建一个占位 index.html，避免嵌入空目录
        std::fs::write(
            dist_dir.join("index.html"),
            "<!DOCTYPE html><html><body><p>前端未构建，请运行: cd web-ui && pnpm build</p></body></html>",
        )
        .expect("无法写入占位 index.html");
    }
}
