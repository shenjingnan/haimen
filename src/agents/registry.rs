//! Agent 注册表
//!
//! 将 Agent 实现的"分发"从各处硬编码 match 收敛为集中式注册：
//! 新增 Agent 只需实现 [`AgentProvider`] 并在 [`builtin`] 中注册一行，
//! 无需再改动 `gateway::build_agent` / `cli::create_agent` / web verify 等调用点。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::config::settings::GatewayConfig;
use crate::gateway::provider::AgentProvider;

use super::claude_code::agent::ClaudeAgent;
use super::codex::agent::CodexAgent;

/// Agent 提供商的展示信息（供 Web API / 前端渲染）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    /// 提供商 id，与 [`AgentProvider::name`] 保持一致
    pub id: &'static str,
    /// UI 显示名
    pub display_name: &'static str,
}

/// Agent 工厂：根据配置构造一个 Agent 实例
pub type AgentFactory = fn(&GatewayConfig) -> Result<Box<dyn AgentProvider>, String>;

/// 静态 Agent 注册表
pub struct AgentRegistry {
    entries: HashMap<&'static str, (ProviderInfo, AgentFactory)>,
}

impl AgentRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// 注册一个 Agent 提供商。
    ///
    /// `id` 与 [`AgentProvider::name`] 一致；重复注册返回 `Err` 防止撞名。
    pub fn register(
        &mut self,
        id: &'static str,
        display_name: &'static str,
        factory: AgentFactory,
    ) -> Result<(), String> {
        if self.entries.contains_key(id) {
            return Err(format!("Agent 重复注册: {}", id));
        }
        self.entries
            .insert(id, (ProviderInfo { id, display_name }, factory));
        Ok(())
    }

    /// 按名称构造 Agent。未注册的名称返回与历史一致的"不支持的 AI Agent"文案。
    pub fn build(
        &self,
        name: &str,
        config: &GatewayConfig,
    ) -> Result<Box<dyn AgentProvider>, String> {
        match self.entries.get(name) {
            Some((_, factory)) => factory(config),
            None => Err(format!("不支持的 AI Agent: {}", name)),
        }
    }

    /// 列出所有已注册的提供商信息
    pub fn list(&self) -> Vec<ProviderInfo> {
        self.entries
            .values()
            .map(|(info, _)| info.clone())
            .collect()
    }

    /// 是否已注册指定名称
    pub fn has(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 内置 Agent 注册（新增 Agent 只需在此加一行）
fn builtin() -> AgentRegistry {
    let mut registry = AgentRegistry::new();
    registry
        .register("claude-code", "Claude Code", |_config| {
            Ok(Box::new(ClaudeAgent))
        })
        .expect("内置 Agent claude-code 注册失败");
    registry
        .register("codex", "Codex CLI", |config| {
            // 沙箱策略从 providers.codex.sandbox 读取，默认放开沙箱：
            // Codex 默认 workspace-write 会阻止子进程访问 macOS 钥匙串等系统资源
            let sandbox = super::codex::agent::resolve_sandbox(config);
            Ok(Box::new(CodexAgent::new(sandbox)))
        })
        .expect("内置 Agent codex 注册失败");
    registry
}

static REGISTRY: OnceLock<AgentRegistry> = OnceLock::new();

/// 获取全局注册表（惰性初始化，不可变）
pub fn registry() -> &'static AgentRegistry {
    REGISTRY.get_or_init(builtin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> GatewayConfig {
        GatewayConfig::default()
    }

    #[test]
    fn test_builtin_registers_default_provider() {
        // 默认 active_provider 是 "claude-code"，必须恒注册，否则默认启动即报错
        assert!(registry().has("claude-code"));
        assert!(registry().has("codex"));
    }

    #[test]
    fn test_build_known_agent() {
        let agent = registry()
            .build("claude-code", &test_config())
            .expect("claude-code 应可构造");
        assert_eq!(agent.name(), "claude-code");

        let codex = registry()
            .build("codex", &test_config())
            .expect("codex 应可构造");
        assert_eq!(codex.name(), "codex");
    }

    #[test]
    fn test_build_unknown_agent() {
        let result = registry().build("unknown-agent", &test_config());
        match result {
            Ok(_) => panic!("未知 agent 应返回 Err"),
            Err(err) => assert_eq!(err, "不支持的 AI Agent: unknown-agent"),
        }
    }

    #[test]
    fn test_list_contains_builtin() {
        let list = registry().list();
        let ids: Vec<&str> = list.iter().map(|info| info.id).collect();
        assert!(ids.contains(&"claude-code"));
        assert!(ids.contains(&"codex"));
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let mut reg = AgentRegistry::new();
        reg.register("dup", "Dup", |_c| Ok(Box::new(ClaudeAgent)))
            .expect("首次注册应成功");
        let err = reg
            .register("dup", "Dup 2", |_c| Ok(Box::new(ClaudeAgent)))
            .expect_err("重复注册应返回 Err");
        assert_eq!(err, "Agent 重复注册: dup");
    }

    #[test]
    fn test_factory_receives_config() {
        // 验证工厂能拿到 config（为 MCP 等需要配置的 Agent 铺路）
        let mut reg = AgentRegistry::new();
        reg.register("cfg-agent", "Cfg", |config| {
            let wd = config.work_dir.clone().unwrap_or_default();
            if wd.is_empty() {
                Ok(Box::new(ClaudeAgent))
            } else {
                Err("不应走到".to_string())
            }
        })
        .expect("注册成功");
        let agent = reg.build("cfg-agent", &test_config()).expect("构造成功");
        assert_eq!(agent.name(), "claude-code");
    }
}
