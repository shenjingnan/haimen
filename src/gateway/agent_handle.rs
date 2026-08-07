//! 共享 Agent 句柄 —— Agent 运行时热切换的权威状态
//!
//! 在 Web 端切换 Agent（`PUT /api/v1/settings/agent`）时，运行进程需要
//! "下一条消息就用新 Agent"，而非重启后生效。为此把"当前生效的 Agent"提升为
//! 一份跨线程共享、可在运行时替换的引用：[`SharedAgent`]。
//!
//! 设计要点：
//! - 三条消费路径（IM 网关 / xiaozhi 语音 / GitHub Webhook）各自在消费时
//!   通过 [`snapshot`] 原子读取当前 Agent，切换只影响"下一条/下一个事件"，
//!   正在处理的消息不受影响（持有的旧 `Arc` 仍然有效）。
//! - [`AgentHandle::generation`] 每次替换 +1：会话绑定创建时的代数，
//!   换代后旧会话自动判为失效（见 `session.rs` 的 `agent_gen`）。
//! - 热切换是"先构建候选 + `check_available`，成功才换入"的原子操作，
//!   失败时共享状态保持不变（自动回滚）。
//!
//! 放在主 crate 而非 `haimen-core`：本模块依赖 `crate::agents::registry` 与
//! `crate::config::settings::GatewayConfig`，放进 core 会形成环向依赖。
//! 与 `SharedAsrConfig` / `SharedTtsConfig` 定义在主 crate 的先例一致。

use std::sync::{Arc, RwLock};

use crate::agents::registry::registry;
use crate::config::settings::{AppConfig, GatewayConfig};
use crate::gateway::provider::AgentProvider;

/// 当前生效 Agent 的快照（含代数）
pub struct AgentHandle {
    /// 当前 Agent 实例
    pub agent: Arc<dyn AgentProvider>,
    /// 当前 Agent 名称（与 `agent` 保持一致）
    pub name: String,
    /// 递增代数：每次切换 +1，用于会话失效与切换检测
    pub generation: u64,
}

/// 共享句柄：全部消费路径共用同一个
pub type SharedAgent = Arc<RwLock<AgentHandle>>;

/// 一次原子读出的快照：agent / name / generation 三者一致
#[derive(Clone)]
pub struct AgentSnapshot {
    /// 快照时的 Agent 实例
    pub agent: Arc<dyn AgentProvider>,
    /// 快照时的 Agent 名称
    pub name: String,
    /// 快照时的代数
    pub generation: u64,
}

impl AgentHandle {
    /// 用初始 Agent 创建句柄（代数从 0 开始）
    pub fn new(agent: Arc<dyn AgentProvider>) -> Self {
        Self {
            generation: 0,
            name: agent.name().to_string(),
            agent,
        }
    }

    /// 原子换入新 Agent，代数 +1
    pub fn swap(&mut self, agent: Arc<dyn AgentProvider>) {
        self.generation += 1;
        self.name = agent.name().to_string();
        self.agent = agent;
    }
}

/// 单次锁内读取，保证 agent / name / generation 三者一致
pub fn snapshot(shared: &SharedAgent) -> AgentSnapshot {
    let guard = shared.read().expect("agent handle 锁中毒");
    AgentSnapshot {
        agent: guard.agent.clone(),
        name: guard.name.clone(),
        generation: guard.generation,
    }
}

/// 获取当前生效的 Agent 实例
pub fn current_agent(shared: &SharedAgent) -> Arc<dyn AgentProvider> {
    snapshot(shared).agent
}

/// 获取当前生效的 Agent 名称
pub fn current_name(shared: &SharedAgent) -> String {
    snapshot(shared).name
}

/// 获取当前生效的 Agent 代数
pub fn current_generation(shared: &SharedAgent) -> u64 {
    snapshot(shared).generation
}

/// 从 `Box<dyn AgentProvider>` 包装为共享句柄（代数从 0 开始）
///
/// 供持有 `Box` 形式的 Agent 的调用点（如测试、一次性构建）转为可替换引用。
pub fn into_shared(agent: Box<dyn AgentProvider>) -> SharedAgent {
    Arc::new(RwLock::new(AgentHandle::new(Arc::from(agent))))
}

/// 启动时构建共享句柄
///
/// - `provider_override` 支持 `serve --xiaozhi-llm-provider` 之类的临时覆盖；
///   `None` 时使用配置中的 `active_provider`。
/// - 仅构建不执行 `check_available`，保持与启动路径现状一致
///   （可用性检查由网关连接器分支 / 热切换入口负责）。
pub fn build_shared_agent(
    config: &AppConfig,
    provider_override: Option<&str>,
) -> Result<SharedAgent, String> {
    let agent: Arc<dyn AgentProvider> = match provider_override {
        Some(name) => Arc::from(registry().build(name, &config.gateway)?),
        None => Arc::from(crate::gateway::build_agent(config)?),
    };
    Ok(Arc::new(RwLock::new(AgentHandle::new(agent))))
}

/// 热切换：先构建候选 + `check_available`，成功才换入共享状态
///
/// - 构建失败（未注册的 provider）或 `check_available` 失败（CLI 未安装等）
///   时返回 `Err`，共享状态保持不变（自动回滚）。
/// - 成功后返回新 Agent 名称。
///
/// 注意：本函数只更新内存态。若需要同时持久化配置（Web API 场景），调用方
/// 应先 `save_settings` 再换入，保证磁盘与内存一致（见 `agent_settings.rs`）。
pub async fn try_switch_agent(
    shared: &SharedAgent,
    gateway: &GatewayConfig,
) -> Result<String, String> {
    let name = gateway.resolved_agent();
    let candidate: Arc<dyn AgentProvider> = Arc::from(registry().build(&name, gateway)?);
    candidate.check_available().await?;
    let mut guard = shared.write().expect("agent handle 锁中毒");
    guard.swap(candidate);
    Ok(guard.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::provider::{AgentOutput, AgentProvider};
    use async_trait::async_trait;

    struct MockAgent {
        name: &'static str,
        available: bool,
    }

    #[async_trait]
    impl AgentProvider for MockAgent {
        fn name(&self) -> &str {
            self.name
        }

        async fn check_available(&self) -> Result<(), String> {
            if self.available {
                Ok(())
            } else {
                Err("mock 不可用".to_string())
            }
        }

        async fn process(
            &self,
            _msg: &str,
            _session_id: Option<&str>,
            _work_dir: &str,
        ) -> Result<(AgentOutput, String), String> {
            Ok((AgentOutput::default(), "mock-session".to_string()))
        }
    }

    fn mock(name: &'static str) -> Arc<dyn AgentProvider> {
        Arc::new(MockAgent {
            name,
            available: true,
        })
    }

    fn shared_with(name: &'static str) -> SharedAgent {
        Arc::new(RwLock::new(AgentHandle::new(mock(name))))
    }

    #[test]
    fn test_handle_new_starts_at_generation_zero() {
        let handle = AgentHandle::new(mock("claude-code"));
        assert_eq!(handle.generation, 0);
        assert_eq!(handle.name, "claude-code");
        assert_eq!(handle.agent.name(), "claude-code");
    }

    #[test]
    fn test_swap_increments_generation_and_updates_name() {
        let mut handle = AgentHandle::new(mock("a"));
        handle.swap(mock("b"));
        assert_eq!(handle.generation, 1);
        assert_eq!(handle.name, "b");
        assert_eq!(handle.agent.name(), "b");

        handle.swap(mock("c"));
        assert_eq!(handle.generation, 2);
        assert_eq!(handle.name, "c");
    }

    #[test]
    fn test_snapshot_consistent_across_swap() {
        let shared = shared_with("a");
        let snap = snapshot(&shared);
        assert_eq!(snap.name, "a");
        assert_eq!(snap.agent.name(), "a");
        assert_eq!(snap.generation, 0);

        shared.write().unwrap().swap(mock("b"));
        let snap = snapshot(&shared);
        assert_eq!(snap.name, "b");
        assert_eq!(snap.agent.name(), "b");
        assert_eq!(snap.generation, 1);
    }

    #[test]
    fn test_current_helpers() {
        let shared = shared_with("codex");
        assert_eq!(current_agent(&shared).name(), "codex");
        assert_eq!(current_name(&shared), "codex");
        assert_eq!(current_generation(&shared), 0);

        shared.write().unwrap().swap(mock("openclaw"));
        assert_eq!(current_agent(&shared).name(), "openclaw");
        assert_eq!(current_name(&shared), "openclaw");
        assert_eq!(current_generation(&shared), 1);
    }

    #[tokio::test]
    async fn test_try_switch_unknown_provider_rolls_back() {
        let shared = shared_with("claude-code");
        let gateway = GatewayConfig {
            active_provider: "does-not-exist".to_string(),
            ..Default::default()
        };

        let result = try_switch_agent(&shared, &gateway).await;
        assert!(result.is_err(), "未知 provider 应返回 Err");
        assert!(result.unwrap_err().contains("does-not-exist"));

        // 回滚：共享状态保持原样
        let snap = snapshot(&shared);
        assert_eq!(snap.name, "claude-code");
        assert_eq!(snap.generation, 0);
    }

    #[tokio::test]
    async fn test_try_switch_known_provider_swaps() {
        let shared = shared_with("claude-code");
        let gateway = GatewayConfig {
            active_provider: "claude-code".to_string(),
            ..Default::default()
        };

        // 构建成功 + check_available 通过（真实 CLI 存在时）才换入；
        // 若 CLI 不可用则保持原样 —— 两种结果都不破坏状态一致性。
        match try_switch_agent(&shared, &gateway).await {
            Ok(name) => {
                assert_eq!(name, "claude-code");
                let snap = snapshot(&shared);
                assert_eq!(snap.name, "claude-code");
                assert_eq!(snap.generation, 1);
            }
            Err(_) => {
                // CLI 未安装等环境原因：不得换入
                let snap = snapshot(&shared);
                assert_eq!(snap.generation, 0);
            }
        }
    }
}
