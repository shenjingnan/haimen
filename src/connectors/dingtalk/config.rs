use serde::{Deserialize, Serialize};

/// 钉钉通道配置（TOML 配置层）
///
/// 对应 ~/.haimen/settings.toml 中的 [connectors.dingtalk] 段落。
/// 认证方式由 dws CLI 独立管理（`dws auth login`），无需在配置中填写密钥。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DingTalkConnectorConfig {
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// dws 可执行文件路径（默认从 PATH 查找 "dws"）
    #[serde(default = "default_dws_path")]
    pub dws_path: String,
    /// 群聊中是否共享 Agent 会话
    #[serde(default)]
    pub share_session_in_channel: bool,
}

fn default_dws_path() -> String {
    "dws".to_string()
}

impl Default for DingTalkConnectorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dws_path: default_dws_path(),
            share_session_in_channel: false,
        }
    }
}

impl From<DingTalkConnectorConfig> for haimen_dingtalk::DingTalkConfig {
    fn from(cfg: DingTalkConnectorConfig) -> Self {
        Self {
            dws_path: cfg.dws_path,
            share_session_in_channel: cfg.share_session_in_channel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dingtalk_connector_config_default() {
        let cfg = DingTalkConnectorConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.dws_path, "dws");
        assert!(!cfg.share_session_in_channel);
    }

    #[test]
    fn test_dingtalk_connector_config_custom() {
        let cfg = DingTalkConnectorConfig {
            enabled: true,
            dws_path: "/opt/bin/dws".into(),
            share_session_in_channel: true,
        };
        assert!(cfg.enabled);
        assert_eq!(cfg.dws_path, "/opt/bin/dws");
        assert!(cfg.share_session_in_channel);
    }

    #[test]
    fn test_conversion_to_haimen_dingtalk_config() {
        let cfg = DingTalkConnectorConfig {
            enabled: true,
            dws_path: "/usr/local/bin/dws".into(),
            share_session_in_channel: true,
        };
        let haimen_cfg: haimen_dingtalk::DingTalkConfig = cfg.into();
        assert_eq!(haimen_cfg.dws_path, "/usr/local/bin/dws");
        assert!(haimen_cfg.share_session_in_channel);
    }

    #[test]
    fn test_toml_deserialize_minimal() {
        let toml_str = r#"
            enabled = true
        "#;
        let cfg: DingTalkConnectorConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.dws_path, "dws");
        assert!(!cfg.share_session_in_channel);
    }

    #[test]
    fn test_toml_deserialize_full() {
        let toml_str = r#"
            enabled = true
            dws_path = "/custom/path/dws"
            share_session_in_channel = true
        "#;
        let cfg: DingTalkConnectorConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.dws_path, "/custom/path/dws");
        assert!(cfg.share_session_in_channel);
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = DingTalkConnectorConfig {
            enabled: true,
            dws_path: "my-dws".into(),
            share_session_in_channel: true,
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        let deserialized: DingTalkConnectorConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(cfg, deserialized);
    }
}
