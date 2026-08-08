//! Agent 注册表
//!
//! 将 Agent 实现的"分发"从各处硬编码 match 收敛为集中式注册：
//! 新增 Agent 只需实现 [`AgentProvider`] 并在 [`builtin`] 中注册一行，
//! 无需再改动 `gateway::build_agent` / `cli::create_agent` / web verify 等调用点。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::config::settings::GatewayConfig;
use crate::gateway::provider::AgentProvider;
use haimen_claude_code::ClaudeAgent;
use haimen_codex::{CodexAgent, DEFAULT_SANDBOX};
use haimen_hermes::HermesAgent;
use haimen_openclaw::{DEFAULT_AGENT_ID, OpenClawAgent};

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

/// 从网关配置解析 codex 沙箱策略
///
/// 优先读取 `[gateway.providers.codex] sandbox`，缺省使用
/// [`haimen_codex::DEFAULT_SANDBOX`]。`GatewayConfig` 保留在主 crate，
/// 故该解析逻辑留在注册表而非 haimen-codex crate。
fn resolve_codex_sandbox(config: &GatewayConfig) -> String {
    config
        .providers
        .get("codex")
        .and_then(|p| p.get("sandbox"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_SANDBOX.to_string())
}

/// 从网关配置解析 openclaw agent id
///
/// 优先读取 `[gateway.providers.openclaw] agent`，缺省使用
/// [`haimen_openclaw::DEFAULT_AGENT_ID`]。`GatewayConfig` 保留在主 crate，
/// 故该解析逻辑留在注册表而非 haimen-openclaw crate。
fn resolve_openclaw_agent(config: &GatewayConfig) -> String {
    config
        .providers
        .get("openclaw")
        .and_then(|p| p.get("agent"))
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string())
}

/// 从网关配置解析某 Agent 的 CLI 可执行文件路径
///
/// 优先读取 `[gateway.providers.<name>] cli_path`；空值 / 纯空白 / 未配置时
/// 回退到默认裸命令名（如 "claude"），由 `build_command` 按 PATH 查找。
/// 支持绝对路径与 Windows `.cmd` shim（`build_command` 内部处理）。
fn resolve_cli_path(config: &GatewayConfig, provider: &str, default_binary: &str) -> String {
    config
        .providers
        .get(provider)
        .and_then(|p| p.get("cli_path"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_binary.to_string())
}

/// 内置 Agent 注册（新增 Agent 只需在此加一行）
fn builtin() -> AgentRegistry {
    let mut registry = AgentRegistry::new();
    registry
        .register("claude-code", "Claude Code", |config| {
            // cli_path 从 providers.claude-code.cli_path 读取，默认 "claude"（PATH 查找）
            let cli_path = resolve_cli_path(config, "claude-code", "claude");
            Ok(Box::new(ClaudeAgent::new(cli_path)))
        })
        .expect("内置 Agent claude-code 注册失败");
    registry
        .register("codex", "Codex CLI", |config| {
            // 沙箱策略从 providers.codex.sandbox 读取，默认放开沙箱：
            // Codex 默认 workspace-write 会阻止子进程访问 macOS 钥匙串等系统资源
            let cli_path = resolve_cli_path(config, "codex", "codex");
            let sandbox = resolve_codex_sandbox(config);
            Ok(Box::new(CodexAgent::new(cli_path, sandbox)))
        })
        .expect("内置 Agent codex 注册失败");
    registry
        .register("openclaw", "OpenClaw", |config| {
            // agent id 从 providers.openclaw.agent 读取，默认 "main"（OpenClaw 保留 agent）；
            // --timeout 与网关 agent_timeout_secs 对齐
            let cli_path = resolve_cli_path(config, "openclaw", "openclaw");
            let agent = resolve_openclaw_agent(config);
            let timeout = config.agent_timeout_secs;
            Ok(Box::new(OpenClawAgent::new(cli_path, agent, timeout)))
        })
        .expect("内置 Agent openclaw 注册失败");
    registry
        .register("hermes", "Hermes", |config| {
            // 极简：仅 timeout（haimen 侧等待子进程退出上限，hermes 无 CLI 侧超时）；
            // model/provider 透传留作后续扩展
            let cli_path = resolve_cli_path(config, "hermes", "hermes");
            Ok(Box::new(HermesAgent::new(
                cli_path,
                config.agent_timeout_secs,
            )))
        })
        .expect("内置 Agent hermes 注册失败");
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
        assert!(registry().has("openclaw"));
        assert!(registry().has("hermes"));
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

        let openclaw = registry()
            .build("openclaw", &test_config())
            .expect("openclaw 应可构造");
        assert_eq!(openclaw.name(), "openclaw");

        let hermes = registry()
            .build("hermes", &test_config())
            .expect("hermes 应可构造");
        assert_eq!(hermes.name(), "hermes");
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
        assert!(ids.contains(&"openclaw"));
        assert!(ids.contains(&"hermes"));
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let mut reg = AgentRegistry::new();
        reg.register("dup", "Dup", |_c| Ok(Box::new(ClaudeAgent::new("claude"))))
            .expect("首次注册应成功");
        let err = reg
            .register("dup", "Dup 2", |_c| {
                Ok(Box::new(ClaudeAgent::new("claude")))
            })
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
                Ok(Box::new(ClaudeAgent::new("claude")))
            } else {
                Err("不应走到".to_string())
            }
        })
        .expect("注册成功");
        let agent = reg.build("cfg-agent", &test_config()).expect("构造成功");
        assert_eq!(agent.name(), "claude-code");
    }

    #[test]
    fn test_resolve_codex_sandbox_default() {
        // 未配置时回退到默认（放开沙箱）
        let config = GatewayConfig::default();
        assert_eq!(resolve_codex_sandbox(&config), DEFAULT_SANDBOX);
    }

    #[test]
    fn test_resolve_codex_sandbox_custom() {
        // 配置 [gateway.providers.codex] sandbox 后应被读取
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("sandbox".to_string(), "workspace-write".to_string());
        providers.insert("codex".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_codex_sandbox(&config), "workspace-write");
    }

    #[test]
    fn test_resolve_codex_sandbox_ignores_other_providers() {
        // 其他 provider 的 sandbox 配置不影响 codex
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("sandbox".to_string(), "read-only".to_string());
        providers.insert("claude-code".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_codex_sandbox(&config), DEFAULT_SANDBOX);
    }

    #[test]
    fn test_resolve_openclaw_agent_default() {
        // 未配置时回退到默认 agent
        let config = GatewayConfig::default();
        assert_eq!(resolve_openclaw_agent(&config), DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_resolve_openclaw_agent_custom() {
        // 配置 [gateway.providers.openclaw] agent 后应被读取
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("agent".to_string(), "ops".to_string());
        providers.insert("openclaw".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_openclaw_agent(&config), "ops");
    }

    #[test]
    fn test_resolve_openclaw_agent_ignores_other_providers() {
        // 其他 provider 的 agent 配置不影响 openclaw
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("agent".to_string(), "whatever".to_string());
        providers.insert("codex".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_openclaw_agent(&config), DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_resolve_cli_path_default() {
        // 未配置时回退到默认裸命令名（PATH 查找）
        let config = GatewayConfig::default();
        assert_eq!(resolve_cli_path(&config, "codex", "codex"), "codex");
        assert_eq!(resolve_cli_path(&config, "claude-code", "claude"), "claude");
    }

    #[test]
    fn test_resolve_cli_path_custom() {
        // 配置 [gateway.providers.codex] cli_path 后应被读取
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("cli_path".to_string(), "/opt/codex/bin/codex".to_string());
        providers.insert("codex".to_string(), params);
        config.providers = providers;
        assert_eq!(
            resolve_cli_path(&config, "codex", "codex"),
            "/opt/codex/bin/codex"
        );
    }

    #[test]
    fn test_resolve_cli_path_empty_falls_back() {
        // 显式空串/纯空白回退到默认裸命令名
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("cli_path".to_string(), "   ".to_string());
        providers.insert("codex".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_cli_path(&config, "codex", "codex"), "codex");
    }

    #[test]
    fn test_resolve_cli_path_ignores_other_providers() {
        // 其他 provider 的 cli_path 配置不影响目标 provider
        let mut config = GatewayConfig::default();
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert("cli_path".to_string(), "/weird/path".to_string());
        providers.insert("codex".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_cli_path(&config, "hermes", "hermes"), "hermes");
    }
}
