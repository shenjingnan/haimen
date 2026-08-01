//! Build script — 自动构建 Web 前端（Vite + React）并嵌入 Rust 二进制。
//!
//! # 行为
//!
//! - 如果 `web-ui/` 目录不存在：创建空的 `web-ui/dist/` 以防止 rust-embed 编译失败
//! - 如果存在 `web-ui/package.json`（git 构建）：按回退链 `pnpm` → `corepack pnpm` →
//!   `npx pnpm` 运行 `pnpm install --frozen-lockfile && pnpm build` 构建前端
//! - 如果只有 `web-ui/dist` 而没有 `package.json`（crates.io 预构建包）：不构建，直接
//!   校验并嵌入提交的产物，不要求 Node（`cargo install` 场景）
//! - 产物校验：`dist/assets/` 非空，且 `index.html` 引用的每个资源都存在
//! - `HAIMEN_SKIP_WEB_UI=1`：跳过前端构建（逃生门，得到无 UI 的二进制）
//! - CI 环境下构建/校验失败会直接导致 cargo build 失败，防止发布损坏的二进制；
//!   本地环境下失败会降级（复用合法产物或建空目录）并发出警告

use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let web_dir = manifest_dir.join("web-ui");
    let dist_dir = web_dir.join("dist");

    let in_ci = env::var_os("GITHUB_ACTIONS").is_some() || env::var_os("CI").is_some();
    let skip = env::var_os("HAIMEN_SKIP_WEB_UI").is_some();
    let sources_present = web_dir.join("package.json").is_file();

    if !web_dir.is_dir() {
        // 兜底：web-ui 目录整体不存在，创建空 dist/ 使 rust-embed 能编译
        create_empty_dist(&dist_dir);
        println!("cargo:warning=web-ui 目录不存在，跳过前端构建（Web 控制台不可用）");
        return;
    }

    // 声明触发重建的条件：只有 web 源码/产物变更时才需要重新构建前端
    println!("cargo:rerun-if-changed=web-ui/package.json");
    println!("cargo:rerun-if-changed=web-ui/pnpm-lock.yaml");
    println!("cargo:rerun-if-changed=web-ui/vite.config.ts");
    println!("cargo:rerun-if-changed=web-ui/index.html");
    println!("cargo:rerun-if-changed=web-ui/src/");
    println!("cargo:rerun-if-changed=web-ui/public/");
    println!("cargo:rerun-if-changed=web-ui/dist/");

    // 逃生门：纯 Rust / 离线环境
    if skip {
        create_empty_dist(&dist_dir);
        println!("cargo:warning=HAIMEN_SKIP_WEB_UI=1：跳过前端构建，Web 控制台不可用");
        return;
    }

    if !sources_present {
        // crates.io 预构建包：crate 内只有 web-ui/dist，没有 package.json（源码不入库）
        // → 不构建，直接校验并嵌入提交的产物（cargo install 无需 Node）
        match validate_dist(&dist_dir) {
            Ok(()) => println!("cargo:warning=嵌入预构建 Web 产物（crates.io 包，无需 Node）"),
            Err(e) => fail_or_warn(&format!("预构建产物无效: {e}"), &dist_dir, in_ci),
        }
        return;
    }

    // git 构建（有源码）：尝试重建前端
    match build_frontend(&web_dir) {
        Ok(()) => {
            if let Err(e) = validate_dist(&dist_dir) {
                fail_or_warn(&format!("构建成功但产物校验失败: {e}"), &dist_dir, in_ci);
            } else {
                println!("cargo:warning=Web 前端构建并校验通过");
            }
        }
        Err(e) => fail_or_warn(&e.to_string(), &dist_dir, in_ci),
    }
}

/// 构建前端。回退链：pnpm → corepack pnpm → npx pnpm。
fn build_frontend(web_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    check_node_version()?;

    // 优先使用 PATH 中的 pnpm；否则用 Node 内置的 corepack（`corepack pnpm` 会按
    // package.json 的 packageManager 自动获取 pnpm@11.5.2）；最后用 npx 兜底
    let (cmd, pre): (&str, Vec<String>) = if have_cmd("pnpm") {
        ("pnpm", vec![])
    } else if have_cmd("corepack") {
        ("corepack", vec!["pnpm".to_string()])
    } else if have_cmd("npx") {
        ("npx", vec!["--yes".to_string(), pnpm_pin(web_dir)])
    } else {
        return Err(
            "未找到 pnpm/corepack/npx（需要 Node.js >= 22.12 且网络可访问 npm registry）".into(),
        );
    };

    run(cmd, &pre, &["install", "--frozen-lockfile"], web_dir)?;
    run(cmd, &pre, &["build"], web_dir)?;

    Ok(())
}

/// 执行命令，失败返回错误。
fn run(
    cmd: &str,
    pre: &[String],
    args: &[&str],
    cwd: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new(cmd)
        .args(pre)
        .args(args)
        .current_dir(cwd)
        .status()?;
    if !status.success() {
        return Err(format!("`{cmd} {args:?}` 失败 (exit={status})").into());
    }
    Ok(())
}

/// 检查命令是否可用。
fn have_cmd(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 检查 Node 版本是否满足 Vite 8 要求（>= 20.19 或 >= 22.12）。
fn check_node_version() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("node").arg("--version").output()?;
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let v = raw.trim_start_matches('v');
    let mut parts = v.split('.');
    let major: u32 = parts
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or("无法解析 node 版本")?;
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);

    if major >= 22 || (major == 20 && minor >= 19) {
        Ok(())
    } else {
        Err(format!("Node 版本过低（{raw}），Vite 8 需要 >= 20.19 或 >= 22.12").into())
    }
}

/// 从 package.json 的 `"packageManager": "pnpm@11.5.2"` 提取 pnpm 版本；失败则回退 `pnpm@11`。
fn pnpm_pin(web_dir: &Path) -> String {
    if let Ok(s) = std::fs::read_to_string(web_dir.join("package.json")) {
        if let Some(i) = s.find("pnpm@") {
            let rest = &s[i + "pnpm@".len()..];
            let ver: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            if !ver.is_empty() {
                return format!("pnpm@{ver}");
            }
        }
    }
    "pnpm@11".into()
}

/// 校验 dist 是否为真实产物：`dist/assets/` 非空，且 index.html 引用的资源都存在。
/// 占位 index.html 引用不存在的 `/assets/*` 资源时校验失败。
fn validate_dist(dist_dir: &Path) -> Result<(), String> {
    let assets = dist_dir.join("assets");
    let assets_ok = assets.is_dir()
        && std::fs::read_dir(&assets)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
    if !assets_ok {
        return Err("dist/assets 缺失或为空（占位/未构建产物）".into());
    }

    let html = std::fs::read_to_string(dist_dir.join("index.html"))
        .map_err(|e| format!("缺少 index.html: {e}"))?;

    for name in scan_asset_refs(&html) {
        if !dist_dir.join(&name).exists() {
            return Err(format!("index.html 引用的资源缺失: {name}"));
        }
    }

    Ok(())
}

/// 从 HTML 文本中提取 `assets/xxx` 相对路径引用（到引号/空白/`>` 结束）。
fn scan_asset_refs(html: &str) -> Vec<String> {
    const NEEDLE: &[u8] = b"assets/";
    let bytes = html.as_bytes();
    let mut refs = Vec::new();
    let mut i = 0;

    while i + NEEDLE.len() <= bytes.len() {
        if &bytes[i..i + NEEDLE.len()] != NEEDLE {
            i += 1;
            continue;
        }

        let mut j = i + NEEDLE.len();
        while j < bytes.len() {
            let c = bytes[j];
            if matches!(c, b'"' | b'\'' | b' ' | b'\t' | b'\n' | b'\r' | b'>') {
                break;
            }
            j += 1;
        }

        let path = html[i..j].to_string();
        if path.starts_with("assets/") && !refs.contains(&path) {
            refs.push(path);
        }
        i = j;
    }

    refs
}

/// 创建空 dist 目录（保证 rust-embed 编译通过）。
fn create_empty_dist(dist_dir: &Path) {
    let _ = std::fs::create_dir_all(dist_dir);
}

/// CI 下失败即 panic（拒绝产出损坏的二进制）；本地降级并警告。
fn fail_or_warn(msg: &str, dist_dir: &Path, in_ci: bool) {
    if in_ci {
        panic!(
            "前端构建/校验失败（CI 环境，拒绝产出损坏的 Web 控制台）: {msg}\n\
             \t请确保 runner 有 Node >= 22.12（Vite 8 要求）与 pnpm（build.rs 会自动走 corepack/npx 回退）。\n\
             \t或设置 HAIMEN_SKIP_WEB_UI=1 跳过前端构建。"
        );
    }

    if validate_dist(dist_dir).is_ok() {
        println!("cargo:warning=前端构建失败（{msg}），已复用现有合法产物");
    } else {
        let _ = std::fs::create_dir_all(dist_dir);
        println!("cargo:warning=前端构建失败（{msg}）且无合法产物，已创建空 web-ui/dist/");
        println!("cargo:warning=Web 控制台不可用。请安装 Node >= 22.12 与 pnpm 后重新构建，");
        println!(
            "cargo:warning=或设置 HAIMEN_SKIP_WEB_UI=1 跳过；亦可直接使用 GitHub Release 预编译二进制。"
        );
    }
}
