//! Agent 契约重导出
//!
//! `AgentProvider` 与 `TextStream` 的权威定义已下沉至 `haimen-core`，
//! 此处仅做重导出以保持既有 `crate::gateway::provider::*` 导入路径兼容。

pub use haimen_core::{AgentEventReceiver, AgentLogEvent, AgentOutput, AgentProvider, TextStream};
