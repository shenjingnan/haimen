/// 集成测试
use clap::Parser;
use haimen::cli::{self, Cli};

#[test]
fn test_cli_config_output() {
    // 验证 CLI 可以正确解析 config 命令
    let cli = Cli::try_parse_from(["test", "config"]).unwrap();
    assert!(matches!(cli.command.unwrap(), cli::Commands::Config));
}

#[test]
fn test_cli_feishu_subcommand_removed() {
    // feishu 子命令已被移除，用户应直接使用 lark-cli
    let result = Cli::try_parse_from(["test", "feishu", "auth", "status"]);
    assert!(
        result.is_err(),
        "feishu subcommand should have been removed"
    );

    let result = Cli::try_parse_from(["test", "feishu", "listen"]);
    assert!(
        result.is_err(),
        "feishu subcommand should have been removed"
    );
}

#[test]
fn test_cli_gateway_status_removed() {
    let result = Cli::try_parse_from(["test", "gateway", "status"]);
    assert!(
        result.is_err(),
        "gateway subcommand should have been removed"
    );
}

#[tokio::test]
async fn test_run_config_returns_ok() {
    let cli = Cli::try_parse_from(["test", "config"]).unwrap();
    let result = cli::run(cli).await;
    assert!(result.is_ok());
}

#[test]
fn test_datetime_iso_format() {
    let now = haimen::datetime::iso_timestamp_now();
    assert!(
        now.contains('T'),
        "ISO 8601 timestamp should contain T separator"
    );
}

#[test]
fn test_logging_init() {
    // 初始化日志不应 panic
    haimen::logging::init_logging();
}
