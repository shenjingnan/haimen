use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing;

use haimen_core::provider::{
    AgentEventStream, AgentLogEvent, AgentOutput, AgentProvider, TextStream,
};

/// hermes 会话源标签（hermes 文档建议第三方集成用 'tool'，不出现在用户会话列表）
const DEFAULT_SOURCE: &str = "tool";

/// Hermes CLI Agent
///
/// 通过 `hermes chat --query=<msg> -Q` 子进程调用（Hermes Agent v0.19+）。
///
/// - **一次性纯文本输出，非流式**：`-Q` 静默机器模式下 stdout 只输出最终回复
///   文本，无 thinking/tool 事件轨迹（与 openclaw 同族）。
/// - **会话**：session_id 由 hermes 侧生成（`{ts}_{uuid}`，如
///   `20260807_161814_722259`），从 stderr 的 `session_id:` 行提取；
///   新会话必须提取，resume 时以 stderr 的值为准（hermes continuation 压缩
///   可能改变 id），stderr 缺失时兜底返回 resume id。
/// - **超时**：hermes 无 CLI 侧模型超时，haimen 侧等待子进程退出即模型生成
///   上限，用 timeout_secs（`[gateway] agent_timeout_secs`，默认 300）。
pub struct HermesAgent {
    /// hermes CLI 可执行文件路径（默认 "hermes"，由 build_command 按 PATH 查找）
    cli_path: String,
    /// 模型调用超时秒数（haimen 侧等待子进程退出的上限）
    timeout_secs: u64,
}

impl HermesAgent {
    /// 使用指定 CLI 路径与超时构造
    pub fn new(cli_path: impl Into<String>, timeout_secs: u64) -> Self {
        Self {
            cli_path: cli_path.into(),
            timeout_secs,
        }
    }

    /// 当前 CLI 路径（供测试断言使用）
    #[cfg(test)]
    pub(crate) fn cli_path(&self) -> &str {
        &self.cli_path
    }

    /// 当前超时秒数（供测试断言使用）
    #[cfg(test)]
    pub(crate) fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

#[async_trait]
impl AgentProvider for HermesAgent {
    fn name(&self) -> &str {
        "hermes"
    }

    /// 批处理：等待 hermes 全部输出完成后返回完整文本（无事件轨迹）
    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(AgentOutput, String), String> {
        let (mut stream, sid, _events_rx) =
            self.process_stream(message, session_id, work_dir).await?;
        let mut full_text = String::new();
        while let Some(chunk) = stream.next().await {
            full_text.push_str(&chunk);
        }
        let final_text = full_text.trim().to_string();
        if final_text.is_empty() {
            return Err("Hermes 返回为空".to_string());
        }
        Ok((
            AgentOutput {
                text: final_text,
                events: Vec::new(),
            },
            sid,
        ))
    }

    /// 流式处理：单块返回完整回复（hermes CLI 非流式），事件流为空
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(TextStream, String, AgentEventStream), String> {
        let (text, sid) = process_with_hermes(
            message,
            session_id,
            work_dir,
            &self.cli_path,
            self.timeout_secs,
        )
        .await?;
        // hermes 不暴露 thinking/tool 事件，事件流为空（sender 立即 drop）
        let (_tx, rx) = mpsc::channel::<AgentLogEvent>(64);
        let stream: TextStream = Box::pin(futures_util::stream::once(async move { text }));
        Ok((stream, sid, rx))
    }

    async fn check_available(&self) -> Result<(), String> {
        if check_hermes_available(&self.cli_path).await {
            Ok(())
        } else {
            Err(format!(
                "hermes CLI 不可用（路径: {}）。请检查 cli_path 配置或安装 Hermes Agent",
                self.cli_path
            ))
        }
    }
}

/// 启动 `hermes chat --query=<msg> -Q` 子进程，返回最终文本与新的 session_id
///
/// 会话语义：
/// - 新会话（session_id = None）→ 不传 `--resume`，hermes 生成新 id，
///   必须从 stderr 提取（缺失则报错）
/// - resume（session_id = Some）→ 传 `--resume <id>`；stderr 的 id 为准
///   （continuation 压缩可能变化），缺失时兜底返回 resume id，会话链不断
async fn process_with_hermes(
    prompt: &str,
    session_id: Option<&str>,
    work_dir: &str,
    cli_path: &str,
    timeout_secs: u64,
) -> Result<(String, String), String> {
    // hermes `-q ""`（空字符串）为 falsy 会进入交互模式挂起，须在 spawn 前拦截
    if prompt.trim().is_empty() {
        return Err("消息为空，无法调用 hermes（hermes 空 query 会进入交互模式）".to_string());
    }

    let args = build_hermes_args(prompt, session_id);

    tracing::debug!(args = ?args, "启动 hermes 子进程");

    // Windows 上 hermes 是 npm 安装的 shim，经 build_command 解析包装
    let mut child = Command::from(haimen_core::process::build_command(cli_path, &args))
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 hermes 失败: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 hermes stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取 hermes stderr".to_string())?;

    // 单发通道：stdout 全量文本 / stderr 全部非空行
    let (out_tx, mut out_rx) = mpsc::channel::<String>(1);
    let (err_tx, mut err_rx) = mpsc::channel::<Vec<String>>(1);

    // 后台读 stdout：全量收集后一次性送出（`-Q` 模式 stdout 即最终回复，不解析）
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut raw = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            raw.push_str(&line);
            raw.push('\n');
        }
        let _ = out_tx.send(raw).await;
    });

    // 后台读 stderr（session_id / 错误文案 / 进度日志都走 stderr）
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut stderr_buf = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                stderr_buf.push(line);
            }
        }
        if !stderr_buf.is_empty() {
            tracing::debug!(stderr_lines = stderr_buf.len(), "hermes stderr 输出：");
            for line in &stderr_buf {
                tracing::debug!(stderr = %line, "hermes stderr");
            }
        }
        let _ = err_tx.send(stderr_buf).await;
    });

    // 等待子进程退出——hermes 无 CLI 侧模型超时，haimen 侧等待即模型生成上限。
    // child.wait() 保持在主 task，超时后才能 start_kill() 杀子进程（与 openclaw 不同，
    // openclaw 把 child 移进线程内 wait，超时无法回收子进程）。
    let exit_code = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait(),
    )
    .await
    {
        Ok(status) => status
            .map_err(|e| format!("等待 hermes 退出失败: {}", e))?
            .code(),
        Err(_) => {
            tracing::warn!(timeout_secs, "hermes 调用超时，杀死子进程");
            let _ = child.start_kill();
            let _ = child.wait().await; // 回收僵尸进程
            return Err(format!("等待 hermes 输出超时 ({}s)", timeout_secs));
        }
    };

    // 子进程已退出 → 管道 EOF → 两个后台线程必然 send 完成，无死锁
    let raw = out_rx
        .recv()
        .await
        .ok_or_else(|| "无法从 hermes 输出中提取回复".to_string())?;
    let stderr_lines = err_rx
        .recv()
        .await
        .ok_or_else(|| "无法从 hermes stderr 中提取信息".to_string())?;

    let stderr_info = parse_hermes_stderr(&stderr_lines);
    assemble_hermes_result(exit_code, &raw, &stderr_info, session_id)
}

/// 构建 `hermes chat` 参数
///
/// - `--query=<prompt>` 而非 `--query <v>`：prompt 以 '-' 开头时 argparse
///   仍视为该 flag 的值，不会误解析成选项
/// - `-Q` 静默机器模式：stdout 只输出最终回复，session_id/错误走 stderr
/// - `--source tool`：标记为第三方集成，不出现在用户会话列表
/// - 有 resume id 时追加 `--resume <id>`
fn build_hermes_args(prompt: &str, resume_session_id: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "chat".to_string(),
        format!("--query={}", prompt),
        "-Q".to_string(),
        "--source".to_string(),
        DEFAULT_SOURCE.to_string(),
    ];
    if let Some(sid) = resume_session_id.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    }
    args
}

/// stderr 解析结果
#[derive(Debug, Default, PartialEq)]
struct HermesStderrInfo {
    /// 从 `session_id:` 行提取（last wins，退出时打印的权威值）
    session_id: Option<String>,
    /// 从 `Error:` 行提取的错误文案（last wins）
    error: Option<String>,
    /// 其余非空行（供报错兜底文案）
    lines: Vec<String>,
}

/// 解析 hermes stderr：提取 `session_id:` / `Error:` 行，其余归入 lines
///
/// `print(f"\nsession_id: ...")` 前导空行在收流时已被过滤，行式解析不受影响。
fn parse_hermes_stderr(lines: &[String]) -> HermesStderrInfo {
    let mut info = HermesStderrInfo::default();
    for line in lines {
        let trimmed = line.trim();
        if let Some(id) = trimmed.strip_prefix("session_id:") {
            let id = id.trim().to_string();
            if !id.is_empty() {
                info.session_id = Some(id);
            }
        } else if let Some(e) = trimmed.strip_prefix("Error:") {
            let e = e.trim().to_string();
            if !e.is_empty() {
                info.error = Some(e);
            }
        } else if !trimmed.is_empty() {
            info.lines.push(trimmed.to_string());
        }
    }
    info
}

/// 根据退出码 + stdout + stderr 信息组装最终结果
///
/// 判定顺序：
/// 1. 退出码非 0 → 优先 stderr `Error:` 文案；无则退出码 + stderr 首行
/// 2. 提取 session_id：stderr（权威）→ resume id（trim 非空兜底）→
///    都没有（新会话且 hermes 未打印）→ 报错
/// 3. stdout trim 空 → stderr 有 Error 则透传，否则报 "Hermes 返回为空"
fn assemble_hermes_result(
    exit_code: Option<i32>,
    stdout: &str,
    stderr_info: &HermesStderrInfo,
    resume_session_id: Option<&str>,
) -> Result<(String, String), String> {
    // 1. 退出码非 0 → 报错
    if let Some(code) = exit_code {
        if code != 0 {
            if let Some(e) = &stderr_info.error {
                return Err(format!("Hermes 调用失败: {}", e));
            }
            let hint = stderr_info
                .lines
                .first()
                .map(|s| s.as_str())
                .unwrap_or("无错误信息");
            return Err(format!("Hermes 调用失败（退出码 {code}）: {hint}"));
        }
    }

    // 2. 提取 session_id
    let session_id = stderr_info
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| resume_session_id.map(str::trim).filter(|s| !s.is_empty()))
        .ok_or_else(|| "无法从 hermes 输出中提取 session_id".to_string())?
        .to_string();

    // 3. stdout trim 空 → 报错
    let text = stdout.trim();
    if text.is_empty() {
        if let Some(e) = &stderr_info.error {
            return Err(format!("Hermes 返回错误: {}", e));
        }
        return Err("Hermes 返回为空".to_string());
    }

    Ok((text.to_string(), session_id))
}

/// 检查 hermes CLI 是否可用
async fn check_hermes_available(cli_path: &str) -> bool {
    Command::from(haimen_core::process::build_command(
        cli_path,
        &["--version".to_string()],
    ))
    .output()
    .await
    .map(|o| o.status.success())
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hermes_agent_name() {
        let agent = HermesAgent::new("hermes", 300);
        assert_eq!(agent.name(), "hermes");
        assert_eq!(agent.cli_path(), "hermes");
        assert_eq!(agent.timeout_secs(), 300);
    }

    #[test]
    fn test_build_args_new_session() {
        let args = build_hermes_args("hello", None);
        assert_eq!(
            args,
            vec![
                "chat".to_string(),
                "--query=hello".to_string(),
                "-Q".to_string(),
                "--source".to_string(),
                "tool".to_string(),
            ]
        );
    }

    #[test]
    fn test_build_args_query_prefix_dash() {
        // prompt 以 '-' 开头必须是单个 --query= 参数，避免被 argparse 误判为选项
        let args = build_hermes_args("-你好", None);
        assert!(args.contains(&"--query=-你好".to_string()));
        assert!(!args.contains(&"-你好".to_string()));
    }

    #[test]
    fn test_build_args_query_contains_newline() {
        // prompt 含换行仍是单个 argv 元素
        let args = build_hermes_args("第一行\n第二行", None);
        assert!(args.contains(&"--query=第一行\n第二行".to_string()));
    }

    #[test]
    fn test_build_args_resume() {
        let args = build_hermes_args("hi", Some("20260807_161814_722259"));
        assert!(args.ends_with(&["--resume".to_string(), "20260807_161814_722259".to_string()]));
    }

    #[test]
    fn test_build_args_resume_ignores_empty() {
        for sid in [Some(""), Some("   "), None] {
            let args = build_hermes_args("hi", sid);
            assert!(!args.contains(&"--resume".to_string()));
        }
    }

    #[test]
    fn test_parse_stderr_session_id() {
        let lines = vec![
            String::new(),
            "session_id: 20260807_161814_722259".to_string(),
        ];
        let info = parse_hermes_stderr(&lines);
        assert_eq!(info.session_id.as_deref(), Some("20260807_161814_722259"));
        assert_eq!(info.error, None);
        assert!(info.lines.is_empty());
    }

    #[test]
    fn test_parse_stderr_error() {
        let lines = vec!["Error: invalid model slug".to_string()];
        let info = parse_hermes_stderr(&lines);
        assert_eq!(info.error.as_deref(), Some("invalid model slug"));
        assert_eq!(info.session_id, None);
    }

    #[test]
    fn test_parse_stderr_no_match() {
        let lines = vec!["progress log line".to_string()];
        let info = parse_hermes_stderr(&lines);
        assert_eq!(info.session_id, None);
        assert_eq!(info.error, None);
        assert_eq!(info.lines, vec!["progress log line".to_string()]);
    }

    #[test]
    fn test_parse_stderr_last_wins() {
        let lines = vec![
            "session_id: first_id".to_string(),
            "session_id: second_id".to_string(),
        ];
        let info = parse_hermes_stderr(&lines);
        assert_eq!(info.session_id.as_deref(), Some("second_id"));
    }

    fn stderr_with(
        session_id: Option<&str>,
        error: Option<&str>,
        lines: &[&str],
    ) -> HermesStderrInfo {
        HermesStderrInfo {
            session_id: session_id.map(|s| s.to_string()),
            error: error.map(|s| s.to_string()),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_assemble_success_new_session() {
        let info = stderr_with(Some("20260807_1"), None, &[]);
        let (text, sid) = assemble_hermes_result(Some(0), "hello\n", &info, None).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(sid, "20260807_1");
    }

    #[test]
    fn test_assemble_success_resume_compaction() {
        // resume 后 hermes continuation 压缩产生新 id → 以 stderr 的为准
        let info = stderr_with(Some("20260807_new"), None, &[]);
        let (_, sid) =
            assemble_hermes_result(Some(0), "hello", &info, Some("20260807_old")).unwrap();
        assert_eq!(sid, "20260807_new");
    }

    #[test]
    fn test_assemble_resume_fallback() {
        // resume 且 stderr 缺 id → 兜底返回 resume id，会话链不断
        let info = stderr_with(None, None, &[]);
        let (_, sid) =
            assemble_hermes_result(Some(0), "hello", &info, Some("20260807_old")).unwrap();
        assert_eq!(sid, "20260807_old");
    }

    #[test]
    fn test_assemble_new_session_missing_sid() {
        // 新会话且 stderr 无 id → 报错
        let info = stderr_with(None, None, &[]);
        let err = assemble_hermes_result(Some(0), "hello", &info, None).unwrap_err();
        assert!(err.contains("session_id"));
    }

    #[test]
    fn test_assemble_exit_nonzero_error_line() {
        let info = stderr_with(None, Some("model unavailable"), &[]);
        let err = assemble_hermes_result(Some(1), "", &info, None).unwrap_err();
        assert!(err.contains("model unavailable"));
    }

    #[test]
    fn test_assemble_exit_nonzero_generic() {
        let info = stderr_with(None, None, &["some context line"]);
        let err = assemble_hermes_result(Some(1), "", &info, None).unwrap_err();
        assert!(err.contains("退出码 1"));
        assert!(err.contains("some context line"));
    }

    #[test]
    fn test_assemble_empty_output_success() {
        let info = stderr_with(Some("20260807_1"), None, &[]);
        let err = assemble_hermes_result(Some(0), "  \n", &info, None).unwrap_err();
        assert_eq!(err, "Hermes 返回为空");
    }

    #[test]
    fn test_assemble_empty_output_with_error() {
        let info = stderr_with(Some("20260807_1"), Some("backend error"), &[]);
        let err = assemble_hermes_result(Some(0), "", &info, None).unwrap_err();
        assert_eq!(err, "Hermes 返回错误: backend error");
    }

    #[test]
    fn test_assemble_trim_output() {
        let info = stderr_with(Some("20260807_1"), None, &[]);
        let (text, _) = assemble_hermes_result(Some(0), "  hello world  \n", &info, None).unwrap();
        assert_eq!(text, "hello world");
    }
}
