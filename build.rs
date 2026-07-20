//! Build script — 自动构建 Web 前端（Vite + React）并嵌入 Rust 二进制。
//!
//! # 行为
//!
//! - 如果 `web-ui/` 目录不存在：跳过（无前端需要构建）
//! - 如果 `pnpm` 可用：运行 `pnpm install --frozen-lockfile && pnpm build`
//! - 如果 `pnpm` 不可用但 `web-ui/dist/` 存在：使用现有构建产物
//! - 如果 `pnpm` 不可用且 `web-ui/dist/` 不存在：创建空目录并发出警告
//!
//! 对于 `cargo publish` 场景：build.rs 会优雅降级，不影响编译。

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let web_dir = manifest_dir.join("web-ui");

    if !web_dir.exists() || !web_dir.is_dir() {
        // 工作空间中不包含 web-ui 前端目录
        return;
    }

    // 声明触发重建的条件：只有 web 源码变更时才需要重新构建前端
    println!("cargo:rerun-if-changed=web-ui/package.json");
    println!("cargo:rerun-if-changed=web-ui/pnpm-lock.yaml");
    println!("cargo:rerun-if-changed=web-ui/vite.config.ts");
    println!("cargo:rerun-if-changed=web-ui/index.html");
    println!("cargo:rerun-if-changed=web-ui/src/");
    println!("cargo:rerun-if-changed=web-ui/public/");

    let dist_dir = web_dir.join("dist");

    match build_frontend(&web_dir) {
        Ok(()) => {
            println!("cargo:warning=Web 前端构建成功");
        }
        Err(e) => {
            if !dist_dir.exists() {
                // 确保 rust-embed 不会因目录缺失而编译失败
                let _ = std::fs::create_dir_all(&dist_dir);
                println!(
                    "cargo:warning=Web 前端未构建（{}），已创建空 web-ui/dist/",
                    e
                );
                println!("cargo:warning='haimen start' 命令需要前端构建产物才能运行 Web 控制台");
                println!("cargo:warning=执行: cd web-ui && pnpm install && pnpm build");
            }
        }
    }
}

/// 检查 pnpm 是否可用并构建前端。
fn build_frontend(web_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // 检查 pnpm 是否可用
    let pnpm = if cfg!(target_os = "windows") {
        "pnpm.cmd"
    } else {
        "pnpm"
    };
    if Command::new(pnpm).arg("--version").output().is_err() {
        return Err("pnpm 未安装".into());
    }

    let status = Command::new(pnpm)
        .args(["install", "--frozen-lockfile"])
        .current_dir(web_dir)
        .status()?;
    if !status.success() {
        return Err("pnpm install 失败".into());
    }

    let status = Command::new(pnpm)
        .args(["build"])
        .current_dir(web_dir)
        .status()?;
    if !status.success() {
        return Err("pnpm build 失败".into());
    }

    Ok(())
}
