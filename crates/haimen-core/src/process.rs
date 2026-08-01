//! 跨平台子进程启动辅助。
//!
//! # 背景
//!
//! Windows 上通过 npm 全局安装的 CLI（如 `claude`、`codex`）实际是 `.cmd`/`.bat`
//! shim，而 `std::process::Command` 底层走 `CreateProcess`，无法直接执行
//! `.cmd`/`.bat`，导致 `Command::new("claude")` 在 Windows 上启动失败
//! （"程序未找到"）。macOS/Linux 没有这个问题。
//!
//! 本模块提供 [`build_command`]：Windows 上先按 `PATH` + `PATHEXT` 解析命令真实
//! 路径，命中 `.cmd`/`.bat` shim 时通过 `cmd.exe /S /C` 包装执行；macOS/Linux
//! 行为与直接 `Command::new(name)` 一致。
//!
//! # 已知限制
//!
//! 经 `cmd.exe /C` 执行时，若参数本身包含双引号，cmd 没有可靠的引号转义机制，
//! 参数可能被错误拆分。这在 Windows 上属 cmd 的固有局限，正常 prompt 极少触发。

use std::process::Command;

/// 构建可执行命令，自动适配平台。
///
/// - macOS / Linux：等价于 `Command::new(name).args(args)`。
/// - Windows：解析 `PATH`（含 `PATHEXT`）得到真实路径；若为 `.cmd`/`.bat` shim 则
///   包装为 `cmd.exe /S /C "<shim> <args...>"`，否则直接按解析出的路径启动。
///
/// 解析失败（程序不存在）时回退为 `Command::new(name)`，由调用方在 `spawn` 时得到
/// 原始错误。
///
/// 返回 `std::process::Command`；使用 tokio 时可用
/// `tokio::process::Command::from(cmd)` 转换（tokio 的 `Command` 即 std `Command`
/// 的包装，会完整保留 program / args / cwd / env / stdio）。
pub fn build_command(name: &str, args: &[impl AsRef<str>]) -> Command {
    // 统一转为 Vec<String>：`impl AsRef<str>` 不保证实现 `AsRef<OsStr>`，
    // 而 `Command::args` 要求后者（String 两者都满足）。
    let args: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();

    #[cfg(windows)]
    {
        if let Some(resolved) = which::which(name)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
        {
            if is_batch_shim(&resolved) {
                let mut cmd = Command::new("cmd");
                cmd.arg("/S")
                    .arg("/C")
                    .arg(build_cmd_exe_line(&resolved, &args));
                return cmd;
            }
            let mut cmd = Command::new(resolved);
            cmd.args(&args);
            return cmd;
        }
    }

    let mut cmd = Command::new(name);
    cmd.args(&args);
    cmd
}

/// 判断路径是否为 `.cmd` / `.bat` shim（必须通过 cmd.exe 执行）。
#[cfg(windows)]
fn is_batch_shim(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// 构造 `cmd.exe /S /C` 的完整命令行，形如 `""C:\path\claude.cmd" --print "hi world""`。
///
/// 这是 cmd 的标准 `""...""` 引号技巧：`/S /C` 会剥除命令行首尾各一个引号，
/// 于是程序名由外层剥除后的引号包裹、与后续参数分离，含空格的路径不会被拆分；
/// 参数含空白或 `&|<>^()` 等 cmd 特殊字符时单独加引号（引号内特殊字符按字面处理）。
///
/// 仅在 Windows 分支使用；为便于跨平台单测，此纯函数在所有平台编译。
#[cfg_attr(not(windows), allow(dead_code))]
fn build_cmd_exe_line(program: &str, args: &[impl AsRef<str>]) -> String {
    let mut line = String::from("\"\"");
    line.push_str(program);
    line.push('"');
    for arg in args {
        line.push(' ');
        line.push_str(&cmd_quote_arg(arg.as_ref()));
    }
    line.push('"');
    line
}

/// cmd 参数引号处理：参数为空或含空白 / 特殊字符（`&|<>^()`）时用双引号包裹。
#[cfg_attr(not(windows), allow(dead_code))]
fn cmd_quote_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || "&|<>^()".contains(c));
    if needs_quotes {
        format!("\"{}\"", arg)
    } else {
        arg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{build_cmd_exe_line, cmd_quote_arg};

    #[test]
    fn quote_plain_flag_unchanged() {
        assert_eq!(cmd_quote_arg("--print"), "--print");
        assert_eq!(cmd_quote_arg("--output-format"), "--output-format");
    }

    #[test]
    fn quote_arg_with_space() {
        assert_eq!(cmd_quote_arg("hello world"), "\"hello world\"");
    }

    #[test]
    fn quote_arg_with_cmd_special() {
        assert_eq!(cmd_quote_arg("a&b"), "\"a&b\"");
        assert_eq!(cmd_quote_arg("(x)"), "\"(x)\"");
    }

    #[test]
    fn quote_empty_arg() {
        assert_eq!(cmd_quote_arg(""), "\"\"");
    }

    #[test]
    fn build_line_plain_program() {
        let args = ["--print".to_string(), "hello world".to_string()];
        assert_eq!(
            build_cmd_exe_line("C:\\tools\\claude.cmd", &args),
            "\"\"C:\\tools\\claude.cmd\" --print \"hello world\"\""
        );
    }

    #[test]
    fn build_line_spaced_program() {
        let args = ["--version".to_string()];
        assert_eq!(
            build_cmd_exe_line("C:\\Program Files\\npm\\claude.cmd", &args),
            "\"\"C:\\Program Files\\npm\\claude.cmd\" --version\""
        );
    }
}
