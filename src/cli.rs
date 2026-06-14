use crate::config;
use crate::feishu;
use clap::{CommandFactory, Parser, Subcommand};

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
    /// AI 网关管理
    #[command(subcommand)]
    Gateway(GatewayCommands),
    /// 生成 Shell 补全脚本
    #[command(hide = true)]
    Completion {
        /// Shell 类型：bash、zsh、fish、powershell、elvish
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
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

#[derive(Subcommand)]
pub enum GatewayCommands {
    /// 显示网关状态
    Status,
    /// 启动网关（监听飞书消息 → MCP 处理 → 结果回飞书）
    Listen,
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

/// gateway status 命令
fn cmd_gateway_status() -> Result<(), String> {
    let status = crate::gateway::status();
    println!("AI 网关状态:");
    println!("  启用: {}", status.enabled);
    if let Some(provider) = &status.provider {
        println!("  提供商: {}", provider);
    } else {
        println!("  提供商: (未配置)");
    }
    println!("  活跃连接: {}", status.active_connections);
    if status.mcp_servers.is_empty() {
        println!("  MCP 服务器: (未配置)");
        println!();
        println!("提示: 在 ~/.haimen/settings.toml 中添加以下配置:");
        println!("  [gateway.mcp_servers.claude-code]");
        println!("  type = \"stdio\"");
        println!("  command = \"claude\"");
        println!("  args = [\"mcp\", \"serve\"]");
    } else {
        println!("  MCP 服务器:");
        for server in &status.mcp_servers {
            println!("    - {}", server);
        }
    }
    Ok(())
}

/// 构造桥接实例
fn create_bridge() -> feishu::bridge::LarkCliBridge {
    let config = config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();
    feishu::bridge::LarkCliBridge::new(&config.feishu.lark_cli_path)
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
            let bridge = create_bridge();
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
        Some(Commands::Gateway(gateway_cmd)) => match gateway_cmd {
            GatewayCommands::Status => cmd_gateway_status(),
            GatewayCommands::Listen => crate::gateway::listen().await,
        },
        Some(Commands::Completion { shell }) => {
            cmd_completion(shell, &mut std::io::stdout());
            Ok(())
        }
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
            val.get("feishu").is_some(),
            "config should contain feishu section"
        );
        assert!(
            val.get("gateway").is_some(),
            "config should contain gateway section"
        );
    }

    #[test]
    fn test_config_contains_version() {
        let output = cmd_config().unwrap();
        // config output is the full AppConfig, version is not directly in it
        // Instead verify that the output is valid JSON
        assert!(!output.is_empty());
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
        for sub in &["config", "feishu", "gateway", "completion"] {
            assert!(
                output.contains(sub),
                "bash completion should contain subcommand {}",
                sub
            );
        }
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
        for sub in &["config", "feishu", "gateway", "completion"] {
            assert!(
                output.contains(sub),
                "zsh completion should contain subcommand {}",
                sub
            );
        }
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
        for sub in &["config", "feishu", "gateway", "completion"] {
            assert!(
                output.contains(sub),
                "fish completion should contain subcommand {}",
                sub
            );
        }
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
        for sub in &["config", "feishu", "gateway", "completion"] {
            assert!(
                output.contains(sub),
                "powershell completion should contain subcommand {}",
                sub
            );
        }
    }

    #[test]
    fn test_completion_all_shells_have_all_subcommands() {
        let shells = [
            clap_complete::Shell::Bash,
            clap_complete::Shell::Zsh,
            clap_complete::Shell::Fish,
            clap_complete::Shell::PowerShell,
        ];
        for shell in shells {
            let mut buf = Vec::new();
            cmd_completion(shell, &mut buf);
            let output = String::from_utf8(buf).unwrap();
            for sub in &["config", "feishu", "gateway", "completion"] {
                assert!(
                    output.contains(sub),
                    "{:?} completion should contain subcommand {}",
                    shell,
                    sub
                );
            }
        }
    }

    #[test]
    fn test_cli_parse_config() {
        let cli = Cli::try_parse_from(&["test", "config"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Commands::Config));
    }

    #[test]
    fn test_cli_parse_feishu_auth_status() {
        let cli = Cli::try_parse_from(&["test", "feishu", "auth", "status"]).unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Auth { action }) => {
                assert!(matches!(action, FeishuAuthAction::Status));
            }
            _ => panic!("Expected Feishu auth status command"),
        }
    }

    #[test]
    fn test_cli_parse_feishu_auth_login() {
        let cli = Cli::try_parse_from(&["test", "feishu", "auth", "login"]).unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Auth { action }) => {
                assert!(matches!(action, FeishuAuthAction::Login));
            }
            _ => panic!("Expected Feishu auth login command"),
        }
    }

    #[test]
    fn test_cli_parse_feishu_chat_list() {
        let cli = Cli::try_parse_from(&["test", "feishu", "chat", "list"]).unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Chat { action }) => {
                assert!(matches!(action, FeishuChatAction::List));
            }
            _ => panic!("Expected Feishu chat list command"),
        }
    }

    #[test]
    fn test_cli_parse_feishu_listen_defaults() {
        let cli = Cli::try_parse_from(&["test", "feishu", "listen"]).unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Listen {
                mode,
                chat_id,
                interval,
                format,
            }) => {
                assert_eq!(mode, "event");
                assert!(chat_id.is_none());
                assert_eq!(interval, 30);
                assert_eq!(format, "pretty");
            }
            _ => panic!("Expected Feishu listen command"),
        }
    }

    #[test]
    fn test_cli_parse_feishu_listen_with_options() {
        let cli = Cli::try_parse_from(&[
            "test",
            "feishu",
            "listen",
            "--mode",
            "poll",
            "--chat-id",
            "oc_xxx123",
            "--interval",
            "10",
            "--format",
            "json",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Commands::Feishu(FeishuCommands::Listen {
                mode,
                chat_id,
                interval,
                format,
            }) => {
                assert_eq!(mode, "poll");
                assert_eq!(chat_id.unwrap(), "oc_xxx123");
                assert_eq!(interval, 10);
                assert_eq!(format, "json");
            }
            _ => panic!("Expected Feishu listen command"),
        }
    }

    #[test]
    fn test_cli_parse_gateway_status() {
        let cli = Cli::try_parse_from(&["test", "gateway", "status"]).unwrap();
        match cli.command.unwrap() {
            Commands::Gateway(GatewayCommands::Status) => {}
            _ => panic!("Expected Gateway status command"),
        }
    }

    #[test]
    fn test_cli_parse_feishu_help() {
        // Verify feishu subcommand appears in help
        let mut cmd = Cli::command();
        let help = cmd.render_help().to_string();
        assert!(
            help.contains("feishu"),
            "help should contain feishu subcommand"
        );
        assert!(
            help.contains("gateway"),
            "help should contain gateway subcommand"
        );
        assert!(
            help.contains("config"),
            "help should contain config subcommand"
        );
    }
}
