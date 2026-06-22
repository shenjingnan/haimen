use std::sync::Arc;

use crate::config;
use clap::{CommandFactory, Parser, Subcommand};
use haimen_lark as feishu;

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
    /// 飞书集成
    #[command(subcommand)]
    Feishu(FeishuCommands),
    /// 启动所有启用的连接器和 Agent
    Start {
        /// Echo 模式：收消息后直接返回，不经过 Agent 处理
        #[arg(long)]
        echo: bool,
    },
    /// AI Agent 调试
    #[command(subcommand)]
    Agent(AgentCommands),
    /// 启动 HTTP Web 服务器（仅 GitHub Webhook）
    Serve {
        /// 监听地址
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// 监听端口
        #[arg(long, default_value_t = 9527)]
        port: u16,
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

#[derive(Subcommand)]
pub enum FeishuCommands {
    /// 飞书认证管理
    Auth {
        #[command(subcommand)]
        action: FeishuAuthAction,
    },
    /// 群聊管理
    Chat {
        #[command(subcommand)]
        action: FeishuChatAction,
    },
    /// 监听飞书消息
    Listen {
        /// 监听模式: event（事件订阅）| poll（轮询）
        #[arg(long, default_value = "event")]
        mode: String,
        /// 聊天 ID（poll 模式必填）
        #[arg(long)]
        chat_id: Option<String>,
        /// 轮询间隔（秒）
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// 输出格式: pretty | json
        #[arg(long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
pub enum FeishuAuthAction {
    /// 查看飞书认证状态
    Status,
    /// 登录飞书（设备码授权）
    Login,
}

#[derive(Subcommand)]
pub enum FeishuChatAction {
    /// 列出可访问的群聊
    List,
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

/// feishu auth status 命令
async fn cmd_feishu_auth_status(bridge: &feishu::bridge::LarkCliBridge) -> Result<(), String> {
    let status = feishu::auth::show_auth_status(bridge).await?;
    println!("飞书认证状态:");
    println!("  App ID: {}", status.app_id);
    println!("  Brand: {}", status.brand);
    println!("  身份类型: {}", status.identity);
    println!(
        "  User: {} (可用: {})",
        status.identities.user.status, status.identities.user.available
    );
    println!(
        "  Bot: {} (可用: {})",
        status.identities.bot.status, status.identities.bot.available
    );
    Ok(())
}

/// feishu auth login 命令
async fn cmd_feishu_auth_login() -> Result<(), String> {
    feishu::auth::login().await
}

/// feishu chat list 命令
async fn cmd_feishu_chat_list(bridge: &feishu::bridge::LarkCliBridge) -> Result<(), String> {
    let chats = feishu::chat::list_chats(bridge).await?;
    if chats.is_empty() {
        println!("当前没有可访问的群聊。");
    } else {
        println!("可访问的群聊 ({}):", chats.len());
        for (i, chat) in chats.iter().enumerate() {
            let name = chat.name.as_deref().unwrap_or("(未命名)");
            println!("  {}. {} (ID: {})", i + 1, name, chat.chat_id);
        }
    }
    Ok(())
}

/// feishu listen 命令
async fn cmd_feishu_listen(
    bridge: &feishu::bridge::LarkCliBridge,
    mode: String,
    chat_id: Option<String>,
    interval: u64,
    format: String,
) -> Result<(), String> {
    let use_json = format == "json";

    match mode.as_str() {
        "event" => {
            feishu::listen::listen_events(bridge, use_json).await?;
        }
        "poll" => {
            let chat_id = chat_id.ok_or_else(|| "poll 模式需要指定 --chat-id".to_string())?;
            feishu::listen::listen_poll(bridge, &chat_id, interval, use_json).await?;
        }
        _ => return Err(format!("不支持的监听模式: {}. 可选: event, poll", mode)),
    }
    Ok(())
}

/// 根据 provider 名称构造 AgentProvider
fn create_agent(
    provider: Option<String>,
) -> Result<Box<dyn crate::gateway::provider::AgentProvider>, String> {
    let config = config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    let agent_name = provider
        .as_deref()
        .or(config.gateway.agent.as_deref())
        .unwrap_or("claude-code");

    match agent_name {
        "claude-code" => Ok(Box::new(crate::agents::claude_code::agent::ClaudeAgent)),
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

/// 构造 LarkCliBridge（从 connectors.lark 配置）
fn create_bridge() -> Result<feishu::bridge::LarkCliBridge, String> {
    let config = config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    let lark_config = config
        .connectors
        .lark
        .ok_or_else(|| "未配置 [connectors.lark]".to_string())?;

    Ok(feishu::bridge::LarkCliBridge::new(
        &lark_config.lark_cli_path,
    ))
}

/// CLI 入口
pub async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Some(Commands::Config) => {
            let output = cmd_config()?;
            println!("{}", output);
            Ok(())
        }
        Some(Commands::Feishu(feishu_cmd)) => {
            let bridge = create_bridge()?;
            match feishu_cmd {
                FeishuCommands::Auth { action } => match action {
                    FeishuAuthAction::Status => cmd_feishu_auth_status(&bridge).await,
                    FeishuAuthAction::Login => cmd_feishu_auth_login().await,
                },
                FeishuCommands::Chat { action } => match action {
                    FeishuChatAction::List => cmd_feishu_chat_list(&bridge).await,
                },
                FeishuCommands::Listen {
                    mode,
                    chat_id,
                    interval,
                    format,
                } => cmd_feishu_listen(&bridge, mode, chat_id, interval, format).await,
            }
        }
        Some(Commands::Start { echo }) => {
            if echo {
                crate::gateway::start_echo().await
            } else {
                crate::gateway::start_all().await
            }
        }
        Some(Commands::Agent(agent_cmd)) => match agent_cmd {
            AgentCommands::Run { provider, prompt } => cmd_agent_run(provider, prompt).await,
            AgentCommands::Chat { provider } => cmd_agent_chat(provider).await,
        },
        Some(Commands::Serve { host, port }) => {
            let settings = crate::config::settings::load_settings()
                .ok()
                .flatten()
                .unwrap_or_default();
            let agent: Arc<dyn crate::gateway::provider::AgentProvider> =
                Arc::new(crate::agents::claude_code::agent::ClaudeAgent);

            let webhook_state = settings.github.map(|cfg| {
                let connector = crate::connectors::github::GitHubConnector::new(cfg, agent.clone());
                crate::gateway::webhook::WebhookState {
                    github: Some(Arc::new(connector)),
                }
            });

            let serve_config = crate::web::ServeConfig { host, port };
            crate::web::start(serve_config, webhook_state).await
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
            "feishu",
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
            "feishu",
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
            "feishu",
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
            "feishu",
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
            Commands::Start { echo } => assert!(!echo),
            _ => panic!("Expected Start command"),
        }
    }

    #[test]
    fn test_cli_parse_start_echo() {
        let cli = Cli::try_parse_from(["test", "start", "--echo"]).unwrap();
        match cli.command.unwrap() {
            Commands::Start { echo } => assert!(echo),
            _ => panic!("Expected Start --echo command"),
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
    fn test_cli_parse_feishu_auth_status() {
        let cli = Cli::try_parse_from(["test", "feishu", "auth", "status"]).unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Auth { action }) => {
                assert!(matches!(action, FeishuAuthAction::Status));
            }
            _ => panic!("Expected Feishu auth status command"),
        }
    }

    #[test]
    fn test_cli_parse_feishu_chat_list() {
        let cli = Cli::try_parse_from(["test", "feishu", "chat", "list"]).unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Chat { action }) => {
                assert!(matches!(action, FeishuChatAction::List));
            }
            _ => panic!("Expected Feishu chat list command"),
        }
    }

    #[test]
    fn test_cli_parse_serve() {
        let cli = Cli::try_parse_from(["test", "serve"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Commands::Serve { .. }));
    }
}
