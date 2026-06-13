/// 集成测试示例
use ai_rust_starter::cli::{self, Cli};
use clap::Parser;

#[test]
fn test_cli_greet_output() {
    // 验证 CLI 可以正确解析 greet 命令
    let cli = Cli::try_parse_from(&["test", "greet", "--name", "World"]).unwrap();
    assert!(matches!(cli.command.unwrap(), cli::Commands::Greet { .. }));
}

#[test]
fn test_cli_config_output() {
    // 验证 CLI 可以正确解析 config 命令
    let cli = Cli::try_parse_from(&["test", "config"]).unwrap();
    assert!(matches!(cli.command.unwrap(), cli::Commands::Config));
}

#[tokio::test]
async fn test_run_config_returns_ok() {
    let cli = Cli::try_parse_from(&["test", "config"]).unwrap();
    let result = cli::run(cli).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_run_greet_returns_ok() {
    let cli = Cli::try_parse_from(&["test", "greet", "--name", "Integration"]).unwrap();
    let result = cli::run(cli).await;
    assert!(result.is_ok());
}

#[test]
fn test_datetime_iso_format() {
    let now = ai_rust_starter::datetime::iso_timestamp_now();
    assert!(
        now.contains('T'),
        "ISO 8601 timestamp should contain T separator"
    );
}

#[test]
fn test_logging_init() {
    // 初始化日志不应 panic
    ai_rust_starter::logging::init_logging();
}
