/// Settings - TOML 配置管理
///
/// 提供通用的配置读写功能，支持 ${env.VAR} 环境变量引用。
/// 配置文件存储在 `~/.{{project_name}}/settings.toml`。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const PROJECT_DIR: &str = ".haimen";
const SETTINGS_FILE: &str = "settings.toml";

/// 获取用户 home 目录（跨平台：macOS/Linux 用 $HOME，Windows 用 %USERPROFILE%）
pub fn get_home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string())
        .into()
}

/// 获取配置目录路径
pub fn get_settings_dir() -> PathBuf {
    get_home_dir().join(PROJECT_DIR)
}

/// 获取设置文件路径
pub fn get_settings_path() -> PathBuf {
    get_settings_dir().join(SETTINGS_FILE)
}

/// 解析 ${env.VAR} 引用
///
/// - "${env.MY_VAR}" → 从环境变量 MY_VAR 读取
/// - "plain-value" → 原样返回
pub fn resolve_env_ref(value: &str) -> Result<String, String> {
    if let Some(captures) = value
        .strip_prefix("${env.")
        .and_then(|s| s.strip_suffix('}'))
    {
        let env_var = captures;
        if env_var.is_empty() {
            return Err("环境变量名称为空".to_string());
        }
        match std::env::var(env_var) {
            Ok(resolved) => Ok(resolved),
            Err(_) => Err(format!(
                "环境变量 {} 未设置。请在 {} 中配置或设置环境变量 {}。",
                env_var, SETTINGS_FILE, env_var
            )),
        }
    } else {
        Ok(value.to_string())
    }
}

/// Lark 连接器配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LarkConfig {
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// lark-cli 可执行文件路径（默认从 PATH 查找）
    #[serde(default = "default_lark_cli_path")]
    pub lark_cli_path: String,
}

fn default_lark_cli_path() -> String {
    "lark-cli".to_string()
}

/// DingTalk 连接器配置（TOML 配置层）
///
/// 转换到 dingtalk::config::DingTalkConfig 传递给 Channel。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DingTalkConnectorConfig {
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// 钉钉应用 Client ID
    pub client_id: String,
    /// 钉钉应用 Client Secret
    pub client_secret: String,
    /// 允许的用户 ID 白名单，"," 分隔。"*" 表示全部允许。
    #[serde(default = "default_dingtalk_allow_from")]
    pub allow_from: String,
    /// 群聊中是否共享 Agent 会话
    #[serde(default)]
    pub share_session_in_channel: bool,
    /// 机器人编码（可选，默认等于 client_id）
    #[serde(default)]
    pub robot_code: String,
}

fn default_dingtalk_allow_from() -> String {
    "*".to_string()
}

impl From<DingTalkConnectorConfig> for crate::connectors::dingtalk::config::DingTalkConfig {
    fn from(cfg: DingTalkConnectorConfig) -> Self {
        Self {
            client_id: cfg.client_id,
            client_secret: cfg.client_secret,
            allow_from: cfg.allow_from,
            share_session_in_channel: cfg.share_session_in_channel,
            robot_code: cfg.robot_code,
        }
    }
}

/// 所有连接器的统一容器
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsSection {
    pub lark: Option<LarkConfig>,
    pub dingtalk: Option<DingTalkConnectorConfig>,
}

/// AI 网关配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayConfig {
    /// AI Agent 类型（如 "claude-code"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// API 密钥
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// 模型名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 默认工作目录（Claude Code session 绑定到此目录）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    /// 会话空闲超时（分钟），超过此时间无消息自动切新会话，默认 30
    #[serde(default = "default_session_idle_timeout")]
    pub session_idle_timeout_mins: u64,
    /// 会话最大轮次，达到后自动切新会话，默认 20
    #[serde(default = "default_session_max_turns")]
    pub session_max_turns: u32,
    /// MCP 服务器配置（haimen 作为客户端连接）
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// Agent 处理超时秒数，超过此时间未返回则放弃并继续处理下一条消息
    /// 默认 300 秒（5 分钟）
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout_secs: u64,
}

fn default_session_idle_timeout() -> u64 {
    30
}

fn default_session_max_turns() -> u32 {
    20
}

fn default_agent_timeout() -> u64 {
    300
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            agent: None,
            api_key: None,
            model: None,
            work_dir: None,
            session_idle_timeout_mins: default_session_idle_timeout(),
            session_max_turns: default_session_max_turns(),
            mcp_servers: HashMap::new(),
            agent_timeout_secs: default_agent_timeout(),
        }
    }
}

/// MCP 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerConfig {
    /// 连接类型: stdio
    #[serde(default = "default_mcp_type")]
    pub r#type: String,
    /// 可执行文件路径
    pub command: String,
    /// 启动参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 描述
    #[serde(default)]
    pub description: String,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            r#type: default_mcp_type(),
            command: String::new(),
            args: Vec::new(),
            description: String::new(),
        }
    }
}

fn default_mcp_type() -> String {
    "stdio".to_string()
}

/// HTTP 服务器配置（`haimen start` 自动启动 Web 控制台 + xiaozhi + GitHub Webhook）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpServerConfig {
    /// 是否启用 HTTP 服务器
    #[serde(default = "default_http_enabled")]
    pub enabled: bool,
    /// 监听地址
    #[serde(default = "default_http_host")]
    pub host: String,
    /// 监听端口
    #[serde(default = "default_http_port")]
    pub port: u16,
    /// 启动后自动打开浏览器
    #[serde(default = "default_auto_open_browser")]
    pub auto_open_browser: bool,
}

fn default_auto_open_browser() -> bool {
    true
}

fn default_http_enabled() -> bool {
    true
}

fn default_http_host() -> String {
    "0.0.0.0".to_string()
}

fn default_http_port() -> u16 {
    9527
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            enabled: default_http_enabled(),
            host: default_http_host(),
            port: default_http_port(),
            auto_open_browser: default_auto_open_browser(),
        }
    }
}

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    /// 调试模式
    #[serde(default)]
    pub debug: bool,
    /// 日志级别
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// 网关配置
    #[serde(default)]
    pub gateway: GatewayConfig,
    /// 连接器配置（统一容器）
    #[serde(default)]
    pub connectors: ConnectorsSection,
    /// HTTP 服务器配置（`haimen start` 自动启动）
    #[serde(default)]
    pub http: HttpServerConfig,
    /// GitHub Webhook 配置（后续方案 A 移入 connectors）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<crate::connectors::github::config::GitHubConfig>,
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            debug: false,
            log_level: default_log_level(),
            gateway: GatewayConfig::default(),
            connectors: ConnectorsSection::default(),
            http: HttpServerConfig::default(),
            github: None,
        }
    }
}

/// 加载 ~/.haimen/settings.toml
///
/// 文件不存在时返回 None，不报错。
pub fn load_settings() -> Result<Option<AppConfig>, String> {
    let file_path = get_settings_path();

    let content = match std::fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    };

    let config: AppConfig =
        toml::from_str(&content).map_err(|e| format!("TOML 格式错误: {}", e))?;

    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    fn write_toml_settings(home: &std::path::Path, content: &str) {
        let settings_dir = home.join(PROJECT_DIR);
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(settings_dir.join(SETTINGS_FILE), content).unwrap();
    }

    #[test]
    fn test_get_settings_path() {
        run_with_temp_home(|home| {
            let path = get_settings_path();
            assert_eq!(path, home.join(".haimen/settings.toml"));
        });
    }

    #[test]
    fn test_get_settings_dir() {
        run_with_temp_home(|home| {
            let dir = get_settings_dir();
            assert_eq!(dir, home.join(".haimen"));
        });
    }

    #[test]
    fn test_resolve_env_ref_plain_value() {
        assert_eq!(resolve_env_ref("plain-value").unwrap(), "plain-value");
        assert_eq!(
            resolve_env_ref("https://example.com").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn test_resolve_env_ref_from_env() {
        unsafe {
            std::env::set_var("TEST_MY_VAR", "test-resolved-value");
        }
        assert_eq!(
            resolve_env_ref("${env.TEST_MY_VAR}").unwrap(),
            "test-resolved-value"
        );
        unsafe {
            std::env::remove_var("TEST_MY_VAR");
        }
    }

    #[test]
    fn test_resolve_env_ref_missing_var() {
        let result = resolve_env_ref("${env.NONEXISTENT_VAR_XYZ}");
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("NONEXISTENT_VAR_XYZ"));
    }

    #[test]
    fn test_resolve_env_ref_empty() {
        assert_eq!(resolve_env_ref("").unwrap(), "");
    }

    #[test]
    fn test_resolve_env_ref_empty_env_var_name() {
        let result = resolve_env_ref("${env.}");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_settings_file_not_found() {
        run_with_temp_home(|_| {
            let result = load_settings().unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn test_load_settings_invalid_toml() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "{invalid}");
            let result = load_settings();
            assert!(result.is_err());
            assert!(result.err().unwrap().contains("TOML 格式错误"));
        });
    }

    #[test]
    fn test_load_settings_empty() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "");
            let result = load_settings().unwrap().unwrap();
            assert!(!result.debug);
            assert_eq!(result.log_level, "info");
            assert!(result.connectors.lark.is_none());
            assert!(result.connectors.dingtalk.is_none());
            assert!(result.http.enabled);
            assert_eq!(result.http.port, 9527);
        });
    }

    #[test]
    fn test_load_settings_with_connectors() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
debug = true
log_level = "debug"

[connectors.lark]
enabled = true
lark_cli_path = "my-lark-cli"

[connectors.dingtalk]
enabled = false
client_id = "test-id"
client_secret = "${env.DINGTALK_CLIENT_SECRET}"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert!(result.debug);
            assert_eq!(result.log_level, "debug");

            let lark = result.connectors.lark.unwrap();
            assert!(lark.enabled);
            assert_eq!(lark.lark_cli_path, "my-lark-cli");

            let dingtalk = result.connectors.dingtalk.unwrap();
            assert!(!dingtalk.enabled);
            assert_eq!(dingtalk.client_id, "test-id");
            assert_eq!(dingtalk.client_secret, "${env.DINGTALK_CLIENT_SECRET}");
        });
    }

    #[test]
    fn test_connectors_section_all_disabled() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.lark]
enabled = false

[connectors.dingtalk]
enabled = false
client_id = "id"
client_secret = "secret"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            let lark = result.connectors.lark.unwrap();
            assert!(!lark.enabled);
            let dingtalk = result.connectors.dingtalk.unwrap();
            assert!(!dingtalk.enabled);
        });
    }

    #[test]
    fn test_lark_config_default_cli_path() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[connectors.lark]
enabled = true
"#,
            );
            let result = load_settings().unwrap().unwrap();
            let lark = result.connectors.lark.unwrap();
            assert_eq!(lark.lark_cli_path, "lark-cli");
        });
    }

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert!(!config.debug);
        assert_eq!(config.log_level, "info");
        assert!(config.connectors.lark.is_none());
        assert!(config.connectors.dingtalk.is_none());
        assert!(config.gateway.agent.is_none());
        assert!(config.http.enabled);
    }

    #[test]
    fn test_app_config_serde_roundtrip() {
        let config = AppConfig {
            debug: true,
            log_level: "warn".to_string(),
            gateway: GatewayConfig {
                agent: Some("claude-code".to_string()),
                ..Default::default()
            },
            connectors: ConnectorsSection {
                lark: Some(LarkConfig {
                    enabled: true,
                    lark_cli_path: "my-lark".to_string(),
                }),
                dingtalk: None,
            },
            http: HttpServerConfig {
                enabled: true,
                host: "127.0.0.1".to_string(),
                port: 8080,
                auto_open_browser: true,
            },
            github: None,
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_gateway_config_default() {
        let config = GatewayConfig::default();
        assert!(config.agent.is_none());
        assert_eq!(config.session_idle_timeout_mins, 30);
        assert_eq!(config.session_max_turns, 20);
    }

    #[test]
    fn test_gateway_config_serde_roundtrip() {
        let config = GatewayConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: GatewayConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_agent_timeout_default() {
        let config = GatewayConfig::default();
        assert_eq!(config.agent_timeout_secs, 300);
    }

    #[test]
    fn test_agent_timeout_serde_roundtrip() {
        let config = GatewayConfig {
            agent_timeout_secs: 600,
            ..Default::default()
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: GatewayConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.agent_timeout_secs, 600);
    }
}
