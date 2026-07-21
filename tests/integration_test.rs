/// 集成测试
use clap::Parser;
use haimen::cli::{self, Cli};
use haimen::gateway::channel::MessageChannel;

// ── CLI 命令解析 ──────────────────────────────────────────────

#[test]
fn test_cli_config_output() {
    let cli = Cli::try_parse_from(["test", "config"]).unwrap();
    assert!(matches!(cli.command.unwrap(), cli::Commands::Config));
}

#[test]
fn test_cli_feishu_subcommand_removed() {
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

// ── DingTalk 连接器配置 ────────────────────────────────────────

#[test]
fn test_dingtalk_connector_config_defaults() {
    let cfg: haimen::config::settings::DingTalkConnectorConfig =
        toml::from_str("enabled = true").unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.dws_path, "dws");
    assert!(!cfg.share_session_in_channel);
}

#[test]
fn test_dingtalk_connector_config_full_toml() {
    let toml_str = r#"
        enabled = true
        dws_path = "/custom/dws"
        share_session_in_channel = true
    "#;
    let cfg: haimen::config::settings::DingTalkConnectorConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.dws_path, "/custom/dws");
    assert!(cfg.share_session_in_channel);
}

#[test]
fn test_dingtalk_connector_conversion_to_haimen_config() {
    let src = haimen::config::settings::DingTalkConnectorConfig {
        enabled: true,
        dws_path: "/opt/bin/dws".into(),
        share_session_in_channel: true,
    };
    let target: haimen_dingtalk::DingTalkConfig = src.into();
    assert_eq!(target.dws_path, "/opt/bin/dws");
    assert!(target.share_session_in_channel);
}

#[test]
fn test_dingtalk_channel_name() {
    let config = haimen_dingtalk::DingTalkConfig::default();
    let channel = haimen_dingtalk::DingTalkChannel::new(config);
    assert_eq!(channel.name(), "dingtalk");
}

// ── DingTalk 健康检查 ──────────────────────────────────────────

#[tokio::test]
async fn test_dingtalk_health_check_fails_without_dws() {
    let config = haimen_dingtalk::DingTalkConfig {
        dws_path: "nonexistent-dws-binary".into(),
        share_session_in_channel: false,
    };
    let channel = haimen_dingtalk::DingTalkChannel::new(config);
    let result = channel.health_check().await;
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(
        err.contains("dws") || err.contains("未安装"),
        "错误信息应提及 dws: {}",
        err
    );
}

#[test]
fn test_dingtalk_channel_creation() {
    let channel = haimen_dingtalk::DingTalkChannel::new(haimen_dingtalk::DingTalkConfig::default());
    assert_eq!(channel.name(), "dingtalk");
}

// ── 工具函数 ──────────────────────────────────────────────────

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
    haimen::logging::init_logging();
}

// ── 网关构建器 ────────────────────────────────────────────────

#[test]
fn test_build_connectors_empty_config() {
    let config = haimen::config::settings::AppConfig::default();
    let connectors = haimen::gateway::build_connectors(&config).unwrap();
    assert!(connectors.is_empty(), "默认配置不应包含任何连接器");
}

#[test]
fn test_build_connectors_with_dingtalk() {
    use haimen::config::settings::AppConfig;
    let config = AppConfig {
        connectors: haimen::config::settings::ConnectorsSection {
            dingtalk: Some(haimen::config::settings::DingTalkConnectorConfig {
                enabled: true,
                dws_path: "dws".into(),
                share_session_in_channel: false,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let connectors = haimen::gateway::build_connectors(&config).unwrap();
    assert_eq!(connectors.len(), 1);
    assert_eq!(connectors[0].0, "dingtalk");
}
