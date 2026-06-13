use clap::{CommandFactory, Parser, Subcommand};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "ai-rust-starter",
    version = VERSION,
    about = "A Rust project starter template",
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
    /// 向用户问好（演示命令参数用法）
    Greet {
        /// 你的名字
        #[arg(short, long)]
        name: String,
        /// 重复次数
        #[arg(short, long, default_value = "1")]
        count: u32,
    },
    /// 生成 Shell 补全脚本
    #[command(hide = true)]
    Completion {
        /// Shell 类型：bash、zsh、fish、powershell、elvish
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// config 命令
fn cmd_config() -> Result<String, String> {
    let config = serde_json::json!({
        "version": VERSION,
        "debug": false,
        "logLevel": "info",
    });
    Ok(serde_json::to_string_pretty(&config).unwrap_or_default())
}

/// greet 命令
fn cmd_greet(name: &str, count: u32) -> Result<(), String> {
    for _ in 0..count {
        println!("你好, {}！欢迎使用 ai-rust-starter。", name);
    }
    Ok(())
}

/// completion 命令
fn cmd_completion<W: std::io::Write>(shell: clap_complete::Shell, writer: &mut W) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "ai-rust-starter", writer);
}

/// CLI 入口
pub async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Some(Commands::Config) => {
            let output = cmd_config()?;
            println!("{}", output);
            Ok(())
        }
        Some(Commands::Greet { name, count }) => cmd_greet(&name, count),
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
            val["logLevel"],
            serde_json::Value::String("info".to_string())
        );
        assert_eq!(val.as_object().unwrap().len(), 3);
    }

    #[test]
    fn test_config_contains_version() {
        let output = cmd_config().unwrap();
        assert!(output.contains(VERSION));
    }

    #[test]
    fn test_greet_output() {
        // greet 直接打印到 stdout，验证不 panic
        cmd_greet("World", 1).expect("greet should succeed");
        cmd_greet("World", 0).expect("greet with 0 count should succeed");
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
        for sub in &["config", "greet", "completion"] {
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
        for sub in &["config", "greet", "completion"] {
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
        for sub in &["config", "greet", "completion"] {
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
        for sub in &["config", "greet", "completion"] {
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
            for sub in &["config", "greet", "completion"] {
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
    fn test_cli_parse_greet() {
        let cli = Cli::try_parse_from(&["test", "greet", "--name", "World"]).unwrap();
        match cli.command.unwrap() {
            Commands::Greet { name, count } => {
                assert_eq!(name, "World");
                assert_eq!(count, 1);
            }
            _ => panic!("Expected Greet command"),
        }
    }

    #[test]
    fn test_cli_parse_greet_with_count() {
        let cli = Cli::try_parse_from(&["test", "greet", "-n", "Test", "-c", "3"]).unwrap();
        match cli.command.unwrap() {
            Commands::Greet { name, count } => {
                assert_eq!(name, "Test");
                assert_eq!(count, 3);
            }
            _ => panic!("Expected Greet command"),
        }
    }

    #[test]
    fn test_cli_parse_config() {
        let cli = Cli::try_parse_from(&["test", "config"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Commands::Config));
    }
}
