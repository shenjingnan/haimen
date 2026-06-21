use serde::{Deserialize, Serialize};

use crate::config::settings::resolve_env_ref;

/// 钉钉通道配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DingTalkConfig {
    /// AppKey（钉钉开放平台创建应用时获取）
    pub client_id: String,

    /// AppSecret（钉钉开放平台创建应用时获取）
    pub client_secret: String,

    /// 允许的用户 ID 白名单，"," 分隔。"*" 表示全部允许。
    #[serde(default = "default_allow_from")]
    pub allow_from: String,

    /// 群聊中是否共享 Agent 会话。
    /// true = 群内所有用户共用一个会话，false = 按用户隔离会话
    #[serde(default)]
    pub share_session_in_channel: bool,

    /// 机器人编码（可选，默认等于 client_id）
    #[serde(default)]
    pub robot_code: String,
}

fn default_allow_from() -> String {
    "*".to_string()
}

impl Default for DingTalkConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            allow_from: default_allow_from(),
            share_session_in_channel: false,
            robot_code: String::new(),
        }
    }
}

impl DingTalkConfig {
    /// 验证配置是否有效
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.client_id.is_empty() {
            errors.push("client_id 不能为空".to_string());
        }
        if self.client_secret.is_empty() {
            errors.push("client_secret 不能为空".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// 获取有效的 robot_code，为空时返回 client_id
    pub fn effective_robot_code(&self) -> &str {
        if self.robot_code.is_empty() {
            &self.client_id
        } else {
            &self.robot_code
        }
    }

    /// 解析配置中 ${env.VAR} 环境变量引用，返回一个新的 DingTalkConfig
    pub fn resolve_env_refs(&self) -> Result<Self, String> {
        Ok(Self {
            client_id: resolve_env_ref(&self.client_id)?,
            client_secret: resolve_env_ref(&self.client_secret)?,
            allow_from: self.allow_from.clone(),
            share_session_in_channel: self.share_session_in_channel,
            robot_code: self.robot_code.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ok() {
        let cfg = DingTalkConfig {
            client_id: "dingxxx".into(),
            client_secret: "secret".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_missing_client_id() {
        let cfg = DingTalkConfig {
            client_id: String::new(),
            client_secret: "secret".into(),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains(&"client_id 不能为空".to_string()));
    }

    #[test]
    fn test_validate_missing_client_secret() {
        let cfg = DingTalkConfig {
            client_id: "dingxxx".into(),
            client_secret: String::new(),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains(&"client_secret 不能为空".to_string()));
    }

    #[test]
    fn test_validate_both_missing() {
        let cfg = DingTalkConfig::default();
        let err = cfg.validate().unwrap_err();
        assert_eq!(err.len(), 2);
    }

    #[test]
    fn test_effective_robot_code_default() {
        let cfg = DingTalkConfig {
            client_id: "dingxxx".into(),
            client_secret: "secret".into(),
            ..Default::default()
        };
        assert_eq!(cfg.effective_robot_code(), "dingxxx");
    }

    #[test]
    fn test_effective_robot_code_explicit() {
        let cfg = DingTalkConfig {
            client_id: "dingxxx".into(),
            client_secret: "secret".into(),
            robot_code: "my_robot".into(),
            ..Default::default()
        };
        assert_eq!(cfg.effective_robot_code(), "my_robot");
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = DingTalkConfig {
            client_id: "dingxxx".into(),
            client_secret: "secret".into(),
            allow_from: "user1,user2".into(),
            share_session_in_channel: true,
            robot_code: String::new(),
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        let deserialized: DingTalkConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(cfg, deserialized);
    }

    #[test]
    fn test_config_default_with_partial_toml() {
        let toml_str = r#"
            client_id = "dingxxx"
            client_secret = "secret"
        "#;
        let cfg: DingTalkConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.client_id, "dingxxx");
        assert_eq!(cfg.allow_from, "*");
        assert!(!cfg.share_session_in_channel);
    }
}
