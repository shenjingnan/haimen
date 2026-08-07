use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::Duration;
use tracing;

/// 会话标识键 — 用 chat_id 区分不同对话
pub type SessionKey = String;

/// 单个会话信息
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Claude Code 返回的 session_id
    pub claude_session_id: String,
    /// 工作目录（session 绑定到目录）
    pub cwd: String,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后活跃时间
    pub last_active: DateTime<Utc>,
    /// 当前轮次
    pub turn_count: u32,
    /// 最大轮次（达到后自动切）
    pub max_turns: u32,
    /// 创建该会话时的 Agent 代数；与当前代数不一致视为失效
    pub agent_gen: u64,
}

impl SessionInfo {
    /// 判断会话是否因空闲超时而过期
    pub fn is_expired(&self, idle_timeout: Duration) -> bool {
        let elapsed = Utc::now() - self.last_active;
        // chrono::Duration (TimeDelta) 不能直接和 std::time::Duration 比较，需要转换
        let elapsed_std = elapsed.to_std().unwrap_or(std::time::Duration::MAX);
        elapsed_std > idle_timeout
    }

    /// 判断会话是否达到最大轮次
    pub fn is_max_turns_reached(&self) -> bool {
        self.turn_count >= self.max_turns
    }

    /// 记录一轮对话
    pub fn record_turn(&mut self) {
        self.turn_count += 1;
        self.last_active = Utc::now();
    }
}

/// 会话管理器
///
/// 管理多个聊天会话的创建、复用和轮转。
/// 每个会话绑定一个 chat_id（或 thread_id），对应一个 Claude Code 的 session_id。
#[derive(Debug)]
pub struct SessionManager {
    /// chat_id → SessionInfo
    sessions: HashMap<SessionKey, SessionInfo>,
    /// 空闲超时
    idle_timeout: Duration,
    /// 默认最大轮次
    default_max_turns: u32,
}

impl SessionManager {
    /// 创建会话管理器
    ///
    /// - `idle_timeout_mins`: 空闲超时（分钟），超过此时间无消息的会话会被轮转
    /// - `default_max_turns`: 默认最大轮次，达到后自动切新会话
    pub fn new(idle_timeout_mins: u64, default_max_turns: u32) -> Self {
        Self {
            sessions: HashMap::new(),
            idle_timeout: Duration::from_secs(idle_timeout_mins.saturating_mul(60)),
            default_max_turns,
        }
    }

    /// 获取或创建会话
    ///
    /// 返回 `(是否需要新会话, 可选的旧 session_id)`
    /// - `(true, None)` — 需要启动新会话（无旧会话、旧会话已过期，或 Agent 已换代）
    /// - `(false, Some(id))` — 应继续使用此 session_id 进行 resume
    ///
    /// `current_gen` 为当前 Agent 代数：会话绑定的 `agent_gen` 与当前不一致时
    /// 视为失效（Agent 切换后强制开新会话，避免把旧 Agent 的 session_id
    /// resume 到新 Agent）。
    pub fn get_or_create(&mut self, key: &SessionKey, current_gen: u64) -> (bool, Option<String>) {
        self.cleanup_expired();

        if let Some(session) = self.sessions.get(key) {
            let gen_changed = session.agent_gen != current_gen;
            if gen_changed
                || session.is_expired(self.idle_timeout)
                || session.is_max_turns_reached()
            {
                tracing::info!(
                    session_key = %key,
                    gen_changed = gen_changed,
                    agent_gen = session.agent_gen,
                    current_gen = current_gen,
                    expired = session.is_expired(self.idle_timeout),
                    max_turns = session.is_max_turns_reached(),
                    "会话需要轮转"
                );
                (true, None)
            } else {
                (false, Some(session.claude_session_id.clone()))
            }
        } else {
            (true, None)
        }
    }

    /// 记录一个新会话
    pub fn create_session(
        &mut self,
        key: &SessionKey,
        claude_session_id: &str,
        cwd: &str,
        agent_gen: u64,
    ) {
        let session = SessionInfo {
            claude_session_id: claude_session_id.to_string(),
            cwd: cwd.to_string(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            turn_count: 0,
            max_turns: self.default_max_turns,
            agent_gen,
        };
        tracing::info!(
            session_key = %key,
            claude_session_id = %claude_session_id,
            "创建新会话"
        );
        self.sessions.insert(key.clone(), session);
    }

    /// 记录一轮对话（增加轮次，更新最后活跃时间）
    pub fn record_turn(&mut self, key: &SessionKey) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.record_turn();
        }
    }

    /// 删除指定会话
    pub fn remove_session(&mut self, key: &SessionKey) {
        tracing::info!(session_key = %key, "删除会话");
        self.sessions.remove(key);
    }

    /// 获取指定会话的信息
    pub fn get_session(&self, key: &SessionKey) -> Option<&SessionInfo> {
        self.sessions.get(key)
    }

    /// 列出所有活跃会话
    pub fn list_sessions(&self) -> Vec<(&SessionKey, &SessionInfo)> {
        self.sessions.iter().collect()
    }

    /// 清理所有过期会话
    pub fn cleanup_expired(&mut self) {
        let expired_keys: Vec<SessionKey> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.is_expired(self.idle_timeout))
            .map(|(k, _)| k.clone())
            .collect();
        for key in expired_keys {
            tracing::info!(session_key = %key, "清理过期会话");
            self.sessions.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration as StdDuration;

    #[test]
    fn test_session_info_not_expired() {
        let info = SessionInfo {
            claude_session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            turn_count: 0,
            max_turns: 20,
            agent_gen: 0,
        };
        assert!(!info.is_expired(Duration::from_secs(60)));
        assert!(!info.is_max_turns_reached());
    }

    #[test]
    fn test_session_info_expired() {
        let mut info = SessionInfo {
            claude_session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            turn_count: 0,
            max_turns: 20,
            agent_gen: 0,
        };
        // 模拟时间流逝
        info.last_active = Utc::now() - chrono::Duration::minutes(5);
        // 1分钟超时应该过期
        assert!(info.is_expired(Duration::from_secs(60)));
        // 10分钟超时应该不过期
        assert!(!info.is_expired(Duration::from_secs(600)));
    }

    #[test]
    fn test_session_info_max_turns() {
        let mut info = SessionInfo {
            claude_session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            turn_count: 0,
            max_turns: 3,
            agent_gen: 0,
        };
        assert!(!info.is_max_turns_reached());
        info.record_turn();
        info.record_turn();
        assert!(!info.is_max_turns_reached());
        info.record_turn();
        assert!(info.is_max_turns_reached());
    }

    #[test]
    fn test_session_record_turn_updates_last_active() {
        let mut info = SessionInfo {
            claude_session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            turn_count: 0,
            max_turns: 20,
            agent_gen: 0,
        };
        let before = info.last_active;
        thread::sleep(StdDuration::from_millis(10));
        info.record_turn();
        assert_eq!(info.turn_count, 1);
        assert!(info.last_active > before);
    }

    #[test]
    fn test_session_manager_new_session() {
        let mut mgr = SessionManager::new(30, 20);
        let key = "chat_123".to_string();

        let (need_new, session_id) = mgr.get_or_create(&key, 0);
        assert!(need_new);
        assert!(session_id.is_none());
    }

    #[test]
    fn test_session_manager_reuse_session() {
        let mut mgr = SessionManager::new(30, 20);
        let key = "chat_123".to_string();

        let (need_new, _) = mgr.get_or_create(&key, 0);
        assert!(need_new);

        mgr.create_session(&key, "s1", "/tmp", 0);
        mgr.record_turn(&key);

        let (need_new, session_id) = mgr.get_or_create(&key, 0);
        assert!(!need_new);
        assert_eq!(session_id, Some("s1".to_string()));
    }

    #[test]
    fn test_session_manager_max_turns_rotation() {
        let mut mgr = SessionManager::new(30, 3);
        let key = "chat_123".to_string();

        mgr.create_session(&key, "s1", "/tmp", 0);
        mgr.record_turn(&key);
        mgr.record_turn(&key);
        mgr.record_turn(&key);

        let (need_new, _) = mgr.get_or_create(&key, 0);
        assert!(need_new, "达到最大轮次应触发轮转");
    }

    #[test]
    fn test_session_manager_agent_generation_change_forces_new() {
        let mut mgr = SessionManager::new(30, 20);
        let key = "chat_123".to_string();

        // 代数 0 时创建会话
        mgr.create_session(&key, "s1", "/tmp", 0);
        mgr.record_turn(&key);

        // 同代数可复用
        let (need_new, session_id) = mgr.get_or_create(&key, 0);
        assert!(!need_new, "同代数应复用会话");
        assert_eq!(session_id, Some("s1".to_string()));

        // Agent 换代（代数 1）后旧会话失效，强制新会话
        let (need_new, session_id) = mgr.get_or_create(&key, 1);
        assert!(need_new, "换代后应强制新会话");
        assert!(session_id.is_none());
    }

    #[test]
    fn test_session_manager_generation_stored_per_session() {
        let mut mgr = SessionManager::new(30, 20);
        let key = "chat_123".to_string();

        mgr.create_session(&key, "gen0", "/tmp", 0);
        let info = mgr.get_session(&key).unwrap();
        assert_eq!(info.agent_gen, 0);

        // 新会话记录新代数
        mgr.create_session(&key, "gen5", "/tmp", 5);
        let info = mgr.get_session(&key).unwrap();
        assert_eq!(info.agent_gen, 5);
    }

    #[test]
    fn test_session_manager_remove_session() {
        let mut mgr = SessionManager::new(30, 20);
        let key = "chat_123".to_string();

        mgr.create_session(&key, "s1", "/tmp", 0);
        mgr.remove_session(&key);

        let (need_new, _) = mgr.get_or_create(&key, 0);
        assert!(need_new, "删除后应创建新会话");
    }

    #[test]
    fn test_session_manager_list_sessions() {
        let mut mgr = SessionManager::new(30, 20);

        mgr.create_session(&"chat_1".to_string(), "s1", "/tmp", 0);
        mgr.create_session(&"chat_2".to_string(), "s2", "/tmp", 0);

        let list = mgr.list_sessions();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_session_manager_multiple_keys() {
        let mut mgr = SessionManager::new(30, 20);
        let key1 = "chat_1".to_string();
        let key2 = "chat_2".to_string();

        mgr.create_session(&key1, "s1", "/tmp", 0);
        mgr.create_session(&key2, "s2", "/tmp", 0);

        let (need_new, sid) = mgr.get_or_create(&key1, 0);
        assert!(!need_new);
        assert_eq!(sid, Some("s1".to_string()));

        let (need_new, sid) = mgr.get_or_create(&key2, 0);
        assert!(!need_new);
        assert_eq!(sid, Some("s2".to_string()));
    }

    #[test]
    fn test_get_session_returns_none_for_unknown() {
        let mgr = SessionManager::new(30, 20);
        assert!(mgr.get_session(&"unknown".to_string()).is_none());
    }

    #[test]
    fn test_get_session_returns_info() {
        let mut mgr = SessionManager::new(30, 20);
        mgr.create_session(&"chat_1".to_string(), "s1", "/tmp", 0);
        let info = mgr.get_session(&"chat_1".to_string()).unwrap();
        assert_eq!(info.claude_session_id, "s1");
        assert_eq!(info.turn_count, 0);
    }
}
