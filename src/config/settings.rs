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

/// 飞书配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeishuConfig {
    /// lark-cli 可执行文件路径（默认从 PATH 查找）
    #[serde(default = "default_lark_cli_path")]
    pub lark_cli_path: String,
    /// 监听配置
    #[serde(default)]
    pub listen: FeishuListenConfig,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            lark_cli_path: default_lark_cli_path(),
            listen: FeishuListenConfig::default(),
        }
    }
}

fn default_lark_cli_path() -> String {
    "lark-cli".to_string()
}

/// 飞书监听配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeishuListenConfig {
    /// 监听模式: "event"（事件订阅）或 "poll"（轮询）
    #[serde(default = "default_listen_mode")]
    pub mode: String,
    /// 轮询间隔（秒）
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
}

impl Default for FeishuListenConfig {
    fn default() -> Self {
        Self {
            mode: default_listen_mode(),
            interval_secs: default_interval_secs(),
        }
    }
}

fn default_listen_mode() -> String {
    "event".to_string()
}

fn default_interval_secs() -> u64 {
    30
}

/// AI 网关配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayConfig {
    /// 是否启用 AI 处理
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// AI 提供商
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
    /// 默认 IM 通道: "lark" | "telegram" | "discord"
    #[serde(default = "default_channel")]
    pub channel: String,
}

fn default_channel() -> String {
    "lark".to_string()
}

fn default_session_idle_timeout() -> u64 {
    30
}

fn default_session_max_turns() -> u32 {
    20
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            provider: None,
            api_key: None,
            model: None,
            work_dir: None,
            session_idle_timeout_mins: default_session_idle_timeout(),
            session_max_turns: default_session_max_turns(),
            mcp_servers: HashMap::new(),
            channel: default_channel(),
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

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    /// 调试模式
    #[serde(default)]
    pub debug: bool,
    /// 日志级别
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// 自定义配置项
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<std::collections::HashMap<String, String>>,
    /// 飞书配置
    #[serde(default)]
    pub feishu: FeishuConfig,
    /// AI 网关配置
    #[serde(default)]
    pub gateway: GatewayConfig,
    /// GitHub Webhook 配置
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
            custom: None,
            feishu: FeishuConfig::default(),
            gateway: GatewayConfig::default(),
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
            assert!(result.custom.is_none());
            assert_eq!(result.feishu.lark_cli_path, "lark-cli");
            assert_eq!(result.feishu.listen.mode, "event");
            assert_eq!(result.feishu.listen.interval_secs, 30);
        });
    }

    #[test]
    fn test_load_settings_full() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                "debug = true\nlog_level = \"debug\"\n\n[custom]\nkey1 = \"value1\"\n",
            );
            let result = load_settings().unwrap().unwrap();
            assert!(result.debug);
            assert_eq!(result.log_level, "debug");
            assert_eq!(result.custom.unwrap().get("key1").unwrap(), "value1");
        });
    }

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert!(!config.debug);
        assert_eq!(config.log_level, "info");
        assert!(config.custom.is_none());
        assert_eq!(config.feishu.lark_cli_path, "lark-cli");
        assert_eq!(config.feishu.listen.mode, "event");
    }

    #[test]
    fn test_app_config_serde_roundtrip() {
        let config = AppConfig {
            debug: true,
            log_level: "warn".to_string(),
            custom: Some(std::collections::HashMap::new()),
            feishu: FeishuConfig::default(),
            gateway: GatewayConfig::default(),
            github: None,
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, deserialized);
    }
}
