use std::sync::Arc;

use crate::config;
use clap::{CommandFactory, Parser, Subcommand};
use tokio_util::sync::CancellationToken;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "haimen",
    version = VERSION,
    about = "AI 网关基建 CLI",
    subcommand_required = true,
    arg_required_else_help = true,
    disable_help_subcommand = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
#[non_exhaustive]
pub enum Commands {
    /// 显示配置信息
    Config,
    /// 启动所有启用的连接器和 Agent
    Start {
        /// Echo 模式：收消息后直接返回，不经过 Agent 处理
        #[arg(long)]
        echo: bool,
        /// 不自动打开浏览器
        #[arg(long)]
        no_browser: bool,
    },
    /// AI Agent 调试
    #[command(subcommand)]
    Agent(AgentCommands),
    /// 启动 HTTP Web 服务器（xiaozhi WebSocket + GitHub Webhook）
    Serve {
        /// 监听地址
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        /// 监听端口
        #[arg(long, default_value_t = 9527)]
        port: u16,
        /// 不自动打开浏览器
        #[arg(long)]
        no_browser: bool,
        /// Echo 模式：收到音频后原样返回（默认是 LLM 模式）
        #[arg(long)]
        xiaozhi_echo: bool,
        /// xiaozhi TTS 测试模式：指定回放文本
        #[arg(long)]
        xiaozhi_tts_text: Option<String>,
        /// xiaozhi TTS 音色（可选，仅 TTS/ASR-TTS/LLM 模式有效）
        #[arg(long)]
        xiaozhi_tts_voice: Option<String>,
        /// ASR-TTS 模式：将设备语音识别为文字后重新合成语音回传
        #[arg(long)]
        xiaozhi_asr_tts: bool,
        /// ASR-LLM-TTS 模式（默认）：语音识别 → AI 处理 → 语音合成
        #[arg(long)]
        xiaozhi_llm: bool,
        /// LLM 提供者（仅 LLM 模式有效，默认从配置读取）
        #[arg(long)]
        xiaozhi_llm_provider: Option<String>,
    },
    /// 生成 Shell 补全脚本
    #[command(hide = true)]
    Completion {
        /// Shell 类型：bash、zsh、fish、powershell、elvish
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// 升级 haimen 到最新版本
    Upgrade,
    /// 卸载 haimen
    Uninstall,
}

/// AI Agent 调试命令
#[derive(Subcommand)]
pub enum AgentCommands {
    /// 单次运行 Agent，传入 prompt
    Run {
        /// AI 提供商（默认从配置读取）
        #[arg(long)]
        provider: Option<String>,
        /// 发送给 Agent 的消息
        prompt: String,
    },
    /// 交互式 Agent 会话（支持 resume）
    Chat {
        /// AI 提供商（默认从配置读取）
        #[arg(long)]
        provider: Option<String>,
    },
}

/// config 命令
fn cmd_config() -> Result<String, String> {
    let config = config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();
    serde_json::to_string_pretty(&config).map_err(|e| format!("序列化配置失败: {}", e))
}

/// completion 命令
fn cmd_completion<W: std::io::Write>(shell: clap_complete::Shell, writer: &mut W) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "haimen", writer);
}

/// 根据 provider 名称构造 AgentProvider
fn create_agent(
    provider: Option<String>,
) -> Result<Box<dyn crate::gateway::provider::AgentProvider>, String> {
    let config = config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    let agent_name = match provider {
        Some(p) => p,
        None => config.gateway.resolved_agent(),
    };

    match agent_name.as_str() {
        "claude-code" => Ok(Box::new(crate::agents::claude_code::agent::ClaudeAgent)),
        "codex" => Ok(Box::new(crate::agents::codex::agent::CodexAgent)),
        "mcp" => Err("MCP Agent 暂不支持直接调用".to_string()),
        other => Err(format!("不支持的 AI Agent: {}", other)),
    }
}

/// agent run 命令：单次调用 Agent
async fn cmd_agent_run(provider: Option<String>, prompt: String) -> Result<(), String> {
    let agent = create_agent(provider)?;
    agent.check_available().await?;

    println!("🤖 正在调用 {}...", agent.name());
    let (response, session_id) = agent.process(&prompt, None).await?;

    println!("{}", response);
    tracing::info!(response_len = response.len(), session_id = %session_id, "Agent 处理完成");
    Ok(())
}

/// agent chat 命令：交互式多轮对话
async fn cmd_agent_chat(provider: Option<String>) -> Result<(), String> {
    let agent = create_agent(provider)?;
    agent.check_available().await?;

    println!("🤖 {} 交互模式已启动（输入 /quit 退出）", agent.name());
    let mut session_id: Option<String> = None;

    loop {
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("读取输入失败: {}", e))?;

        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" || input == "/exit" {
            break;
        }

        if input == "/new" {
            session_id = None;
            println!("🔄 已创建新会话");
            continue;
        }

        let (response, new_session_id) = agent.process(&input, session_id.as_deref()).await?;
        session_id = Some(new_session_id);

        println!("{}", response);
    }

    Ok(())
}

/// CLI 入口
pub async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Some(Commands::Config) => {
            let output = cmd_config()?;
            println!("{}", output);
            Ok(())
        }
        Some(Commands::Start { echo, no_browser }) => {
            if echo {
                crate::gateway::start_echo().await
            } else {
                crate::gateway::start_all(no_browser).await
            }
        }
        Some(Commands::Agent(agent_cmd)) => match agent_cmd {
            AgentCommands::Run { provider, prompt } => cmd_agent_run(provider, prompt).await,
            AgentCommands::Chat { provider } => cmd_agent_chat(provider).await,
        },
        Some(Commands::Serve {
            host,
            port,
            no_browser,
            xiaozhi_echo,
            xiaozhi_tts_text,
            xiaozhi_tts_voice,
            xiaozhi_asr_tts,
            xiaozhi_llm: _,
            xiaozhi_llm_provider,
        }) => {
            let settings = crate::config::settings::load_settings()
                .ok()
                .flatten()
                .unwrap_or_default();

            // 构建 webhook 和 LLM 使用的 Agent
            let serve_agent: Arc<dyn crate::gateway::provider::AgentProvider> =
                create_agent(xiaozhi_llm_provider.clone()).map(Arc::from)?;

            let webhook_state = settings.github.map(|cfg| {
                let connector =
                    crate::connectors::github::GitHubConnector::new(cfg, serve_agent.clone());
                crate::gateway::webhook::WebhookState {
                    github: Some(Arc::new(connector)),
                }
            });

            let xiaozhi_strategy: Arc<dyn haimen_xiaozhi::ResponseStrategy> = if xiaozhi_echo {
                Arc::new(haimen_xiaozhi::EchoStrategy)
            } else if xiaozhi_asr_tts {
                Arc::new(crate::xiaozhi_asr_tts::AsrTtsStrategy::from_config(
                    &settings.asr,
                    &settings.tts,
                    xiaozhi_tts_voice,
                )?)
            } else if let Some(text) = xiaozhi_tts_text {
                Arc::new(crate::xiaozhi_tts::TtsStrategy::from_config(
                    text,
                    xiaozhi_tts_voice,
                    &settings.tts,
                ))
            } else {
                // 默认 ASR-LLM-TTS 模式
                Arc::new(crate::xiaozhi_asr_llm_tts::AsrLlmTtsStrategy::from_config(
                    &settings.asr,
                    &settings.tts,
                    xiaozhi_tts_voice,
                    serve_agent,
                )?)
            };

            let serve_config = crate::web::ServeConfig {
                host,
                port,
                auto_open: !no_browser,
            };
            crate::web::start(
                serve_config,
                webhook_state,
                Some(xiaozhi_strategy),
                CancellationToken::new(),
            )
            .await
        }
        Some(Commands::Completion { shell }) => {
            cmd_completion(shell, &mut std::io::stdout());
            Ok(())
        }
        Some(Commands::Upgrade) => crate::commands::upgrade::cmd_upgrade().await,
        Some(Commands::Uninstall) => crate::commands::uninstall::cmd_uninstall(),
        None => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_constant() {
        assert!(!VERSION.is_empty(), "VERSION should not be empty");
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "VERSION should be in semver format (X.Y.Z)");
        for part in &parts {
            assert!(!part.is_empty(), "semver part should not be empty");
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "semver part '{}' should be numeric",
                part
            );
        }
    }

    #[test]
    fn test_config_output() {
        let output = cmd_config().unwrap();
        let val: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(val["debug"], serde_json::Value::Bool(false));
        assert_eq!(
            val["log_level"],
            serde_json::Value::String("info".to_string())
        );
        assert!(
            val.get("gateway").is_some(),
            "config should contain gateway section"
        );
        assert!(
            val.get("connectors").is_some(),
            "config should contain connectors section"
        );
        assert!(
            val.get("http").is_some(),
            "config should contain http section"
        );
        assert_eq!(val["http"]["enabled"], serde_json::Value::Bool(true));
        assert_eq!(
            val["http"]["host"],
            serde_json::Value::String("0.0.0.0".to_string())
        );
        assert_eq!(
            val["http"]["port"],
            serde_json::Value::Number(serde_json::Number::from(9527))
        );
    }

    #[test]
    fn test_completion_bash() {
        let mut buf = Vec::new();
        cmd_completion(clap_complete::Shell::Bash, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("complete -F"),
            "bash completion should contain complete -F"
        );
        for sub in &[
            "config",
            "start",
            "serve",
            "agent",
            "completion",
            "upgrade",
            "uninstall",
        ] {
            assert!(
                output.contains(sub),
                "bash completion should contain subcommand {}",
                sub
            );
        }
        assert!(
            !output.contains("gateway"),
            "bash completion should NOT contain gateway"
        );
    }

    #[test]
    fn test_completion_zsh() {
        let mut buf = Vec::new();
        cmd_completion(clap_complete::Shell::Zsh, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("#compdef"),
            "zsh completion should start with #compdef"
        );
        for sub in &[
            "config",
            "start",
            "serve",
            "agent",
            "completion",
            "upgrade",
            "uninstall",
        ] {
            assert!(
                output.contains(sub),
                "zsh completion should contain subcommand {}",
                sub
            );
        }
        assert!(
            !output.contains("gateway"),
            "zsh completion should NOT contain gateway"
        );
    }

    #[test]
    fn test_completion_fish() {
        let mut buf = Vec::new();
        cmd_completion(clap_complete::Shell::Fish, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("complete -c"),
            "fish completion should contain complete -c"
        );
        for sub in &[
            "config",
            "start",
            "serve",
            "agent",
            "completion",
            "upgrade",
            "uninstall",
        ] {
            assert!(
                output.contains(sub),
                "fish completion should contain subcommand {}",
                sub
            );
        }
        assert!(
            !output.contains("gateway"),
            "fish completion should NOT contain gateway"
        );
    }

    #[test]
    fn test_completion_powershell() {
        let mut buf = Vec::new();
        cmd_completion(clap_complete::Shell::PowerShell, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("Register-ArgumentCompleter"),
            "powershell completion should register argument completer"
        );
        for sub in &[
            "config",
            "start",
            "serve",
            "agent",
            "completion",
            "upgrade",
            "uninstall",
        ] {
            assert!(
                output.contains(sub),
                "powershell completion should contain subcommand {}",
                sub
            );
        }
        assert!(
            !output.contains("gateway"),
            "powershell completion should NOT contain gateway"
        );
    }

    #[test]
    fn test_cli_parse_upgrade() {
        let cli = Cli::try_parse_from(["test", "upgrade"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Commands::Upgrade));
    }

    #[test]
    fn test_cli_parse_uninstall() {
        let cli = Cli::try_parse_from(["test", "uninstall"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Commands::Uninstall));
    }

    #[test]
    fn test_cli_parse_config() {
        let cli = Cli::try_parse_from(["test", "config"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Commands::Config));
    }

    #[test]
    fn test_cli_parse_start() {
        let cli = Cli::try_parse_from(["test", "start"]).unwrap();
        match cli.command.unwrap() {
            Commands::Start { echo, no_browser } => {
                assert!(!echo);
                assert!(!no_browser);
            }
            _ => panic!("Expected Start command"),
        }
    }

    #[test]
    fn test_cli_parse_start_echo() {
        let cli = Cli::try_parse_from(["test", "start", "--echo"]).unwrap();
        match cli.command.unwrap() {
            Commands::Start { echo, no_browser } => {
                assert!(echo);
                assert!(!no_browser);
            }
            _ => panic!("Expected Start --echo command"),
        }
    }

    #[test]
    fn test_cli_parse_start_no_browser() {
        let cli = Cli::try_parse_from(["test", "start", "--no-browser"]).unwrap();
        match cli.command.unwrap() {
            Commands::Start { echo, no_browser } => {
                assert!(!echo);
                assert!(no_browser);
            }
            _ => panic!("Expected Start --no-browser command"),
        }
    }

    #[test]
    fn test_cli_parse_gateway_listen_fails() {
        let result = Cli::try_parse_from(["test", "gateway", "listen"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_parse_gateway_status_fails() {
        let result = Cli::try_parse_from(["test", "gateway", "status"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_parse_feishu_subcommand_fails() {
        let result = Cli::try_parse_from(["test", "feishu"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_parse_serve() {
        let cli = Cli::try_parse_from(["test", "serve"]).unwrap();
        match cli.command.unwrap() {
            Commands::Serve {
                xiaozhi_echo,
                xiaozhi_tts_text,
                xiaozhi_tts_voice,
                no_browser,
                ..
            } => {
                assert!(!xiaozhi_echo, "default should NOT be echo mode");
                assert!(xiaozhi_tts_text.is_none());
                assert!(xiaozhi_tts_voice.is_none());
                assert!(!no_browser);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_parse_serve_echo() {
        let cli = Cli::try_parse_from(["test", "serve", "--xiaozhi-echo"]).unwrap();
        match cli.command.unwrap() {
            Commands::Serve { xiaozhi_echo, .. } => {
                assert!(xiaozhi_echo, "--xiaozhi-echo should enable echo mode");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_parse_serve_no_browser() {
        let cli = Cli::try_parse_from(["test", "serve", "--no-browser"]).unwrap();
        match cli.command.unwrap() {
            Commands::Serve { no_browser, .. } => {
                assert!(no_browser, "--no-browser should disable auto-open");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_parse_serve_with_xiaozhi_tts() {
        let cli = Cli::try_parse_from(["test", "serve", "--xiaozhi-tts-text", "你好"]).unwrap();
        match cli.command.unwrap() {
            Commands::Serve {
                xiaozhi_tts_text,
                xiaozhi_tts_voice,
                ..
            } => {
                assert_eq!(xiaozhi_tts_text.unwrap(), "你好");
                assert!(xiaozhi_tts_voice.is_none());
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_parse_serve_with_xiaozhi_tts_and_voice() {
        let cli = Cli::try_parse_from([
            "test",
            "serve",
            "--xiaozhi-tts-text",
            "今天天气不错",
            "--xiaozhi-tts-voice",
            "zh_female_xiaohe",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Commands::Serve {
                xiaozhi_tts_text,
                xiaozhi_tts_voice,
                ..
            } => {
                assert_eq!(xiaozhi_tts_text.unwrap(), "今天天气不错");
                assert_eq!(xiaozhi_tts_voice.unwrap(), "zh_female_xiaohe");
            }
            _ => panic!("Expected Serve command"),
        }
    }
}
