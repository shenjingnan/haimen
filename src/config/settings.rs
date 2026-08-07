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

impl Default for LarkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lark_cli_path: default_lark_cli_path(),
        }
    }
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
    #[serde(default)]
    pub client_id: String,
    /// 钉钉应用 Client Secret
    #[serde(default)]
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

impl Default for DingTalkConnectorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            client_secret: String::new(),
            allow_from: default_dingtalk_allow_from(),
            share_session_in_channel: false,
            robot_code: String::new(),
        }
    }
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
///
/// # 配置格式
///
/// 支持多 Agent 提供商配置，通过 `active_provider` 切换当前使用的提供商：
///
/// ```toml
/// [gateway]
/// active_provider = "claude-code"
///
/// [gateway.providers.claude-code]
/// # CLI 工具无需额外凭证
/// # 可选：claude CLI 可执行文件路径（留空按 PATH 查找 "claude"；填绝对路径或
/// # 自定义命令名）。适用于 CLI 未装在标准 PATH 的环境。
/// # cli_path = "/opt/claude/bin/claude"
///
/// [gateway.providers.codex]
/// # CLI 工具无需额外凭证
/// # 可选：codex 沙箱策略（read-only / workspace-write / danger-full-access），
/// # 默认 danger-full-access（放开沙箱）。默认 workspace-write 会阻止子进程
/// # 访问 macOS 钥匙串等系统资源（如 lark-cli 读取凭据）。
/// # sandbox = "workspace-write"
/// # 可选：codex CLI 可执行文件路径（留空按 PATH 查找 "codex"）
/// # cli_path = "/opt/codex/bin/codex"
///
/// [gateway.providers.openclaw]
/// # CLI 工具无需额外凭证；建议 openclaw gateway 常驻（缺失时自动降级 embedded）
/// # 可选：openclaw agent id（默认 "main"，OpenClaw 保留 agent）
/// # agent = "ops"
/// # 可选：openclaw CLI 可执行文件路径（留空按 PATH 查找 "openclaw"）
/// # cli_path = "/opt/openclaw/bin/openclaw"
///
/// [gateway.providers.hermes]
/// # CLI 工具无需额外凭证
/// # 可选：hermes CLI 可执行文件路径（留空按 PATH 查找 "hermes"）
/// # cli_path = "/opt/hermes/bin/hermes"
/// ```
///
/// # 向后兼容
///
/// 旧格式 `agent = "claude-code"` 在加载时自动迁移到 `active_provider = "claude-code"`。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GatewayConfig {
    /// 当前激活的 AI Agent 提供商
    #[serde(default = "default_agent_provider")]
    pub active_provider: String,
    /// 所有已配置的 Agent 提供商的参数（{provider_name → {key → value}}）
    #[serde(default)]
    pub providers: HashMap<String, HashMap<String, String>>,
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

fn default_agent_provider() -> String {
    "claude-code".to_string()
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
            active_provider: default_agent_provider(),
            providers: HashMap::new(),
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

/// Agent 调用日志配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentLogConfig {
    /// 是否记录 Agent 调用日志，默认开启
    #[serde(default = "default_agent_log_enabled")]
    pub enabled: bool,
    /// 日志目录（未配置时默认 `~/.haimen/agent-logs`，支持 `~/` 展开）
    #[serde(default)]
    pub dir: Option<String>,
    /// 日志保留天数，超过自动清理，默认 30
    #[serde(default = "default_agent_log_retention")]
    pub retention_days: u64,
}

fn default_agent_log_enabled() -> bool {
    true
}

fn default_agent_log_retention() -> u64 {
    30
}

impl Default for AgentLogConfig {
    fn default() -> Self {
        Self {
            enabled: default_agent_log_enabled(),
            dir: None,
            retention_days: default_agent_log_retention(),
        }
    }
}

/// 旧格式 Gateway 配置（用于向后兼容反序列化）
#[derive(Debug, Clone, Deserialize)]
struct GatewayConfigLegacy {
    agent: Option<String>,
    active_provider: Option<String>,
    providers: Option<HashMap<String, HashMap<String, String>>>,
    api_key: Option<String>,
    model: Option<String>,
    work_dir: Option<String>,
    session_idle_timeout_mins: Option<u64>,
    session_max_turns: Option<u32>,
    mcp_servers: Option<HashMap<String, McpServerConfig>>,
    agent_timeout_secs: Option<u64>,
}

impl<'de> Deserialize<'de> for GatewayConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let legacy = GatewayConfigLegacy::deserialize(deserializer)?;

        // 如果已有新格式字段，直接使用
        if let Some(active) = legacy.active_provider {
            return Ok(Self {
                active_provider: active,
                providers: legacy.providers.unwrap_or_default(),
                api_key: legacy.api_key,
                model: legacy.model,
                work_dir: legacy.work_dir,
                session_idle_timeout_mins: legacy
                    .session_idle_timeout_mins
                    .unwrap_or_else(default_session_idle_timeout),
                session_max_turns: legacy
                    .session_max_turns
                    .unwrap_or_else(default_session_max_turns),
                mcp_servers: legacy.mcp_servers.unwrap_or_default(),
                agent_timeout_secs: legacy
                    .agent_timeout_secs
                    .unwrap_or_else(default_agent_timeout),
            });
        }

        // 旧格式迁移：agent = "claude-code" → active_provider
        let active_provider = legacy.agent.unwrap_or_else(|| "claude-code".to_string());

        Ok(Self {
            active_provider,
            providers: legacy.providers.unwrap_or_default(),
            api_key: legacy.api_key,
            model: legacy.model,
            work_dir: legacy.work_dir,
            session_idle_timeout_mins: legacy
                .session_idle_timeout_mins
                .unwrap_or_else(default_session_idle_timeout),
            session_max_turns: legacy
                .session_max_turns
                .unwrap_or_else(default_session_max_turns),
            mcp_servers: legacy.mcp_servers.unwrap_or_default(),
            agent_timeout_secs: legacy
                .agent_timeout_secs
                .unwrap_or_else(default_agent_timeout),
        })
    }
}

impl GatewayConfig {
    /// 获取当前激活的 Agent 提供商名称
    pub fn resolved_agent(&self) -> String {
        self.active_provider.clone()
    }

    /// 获取当前激活提供商的某个配置值
    pub fn get_credential(&self, key: &str) -> Option<String> {
        self.providers
            .get(&self.active_provider)
            .and_then(|p| p.get(key))
            .filter(|v| !v.is_empty())
            .cloned()
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

/// ASR（语音识别）配置
///
/// 独立的基础能力，不绑定具体场景（xiaozhi 等），未来可被多种场景复用。
///
/// # 配置格式
///
/// 支持多服务商配置，通过 `active_provider` 切换当前使用的服务商：
///
/// ```toml
/// [asr]
/// active_provider = "doubao"
///
/// [asr.providers.doubao]
/// app_key = "..."
/// access_key = "..."
///
/// [asr.providers.qwen]
/// api_key = "..."
/// ```
///
/// # 向后兼容
///
/// 旧格式 `provider = "doubao"` + `app_key` / `access_token` 在加载时自动迁移。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AsrConfig {
    /// 当前激活的 ASR 提供商
    #[serde(default = "default_asr_provider")]
    pub active_provider: String,
    /// 所有已配置的 ASR 提供商的凭证（{provider_name → {key → value}}）
    #[serde(default)]
    pub providers: HashMap<String, HashMap<String, String>>,
}

fn default_asr_provider() -> String {
    "doubao".to_string()
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            active_provider: default_asr_provider(),
            providers: HashMap::new(),
        }
    }
}

/// 旧格式 ASR 配置（用于向后兼容反序列化）
#[derive(Debug, Clone, Deserialize)]
struct AsrConfigLegacy {
    provider: Option<String>,
    app_key: Option<String>,
    active_provider: Option<String>,
    providers: Option<HashMap<String, HashMap<String, String>>>,
}

impl<'de> Deserialize<'de> for AsrConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let legacy = AsrConfigLegacy::deserialize(deserializer)?;

        // 如果已有新格式字段，直接使用
        if let Some(active) = legacy.active_provider {
            return Ok(Self {
                active_provider: active,
                providers: legacy.providers.unwrap_or_default(),
            });
        }

        // 旧格式迁移
        let provider = legacy.provider.unwrap_or_else(|| "doubao".to_string());
        let mut providers = legacy.providers.unwrap_or_default();

        // 如果旧格式有凭证值，迁移到 providers（旧 app_key 迁移为新版 api_key，access_token 已废弃）
        if let Some(key) = legacy
            .app_key
            .filter(|s| !s.is_empty())
            .filter(|_| !providers.contains_key(&provider))
        {
            let mut creds = HashMap::new();
            creds.insert("api_key".to_string(), key);
            providers.insert(provider.clone(), creds);
        }

        Ok(Self {
            active_provider: provider,
            providers,
        })
    }
}

impl AsrConfig {
    /// 获取当前激活提供商的某个凭证值
    ///
    /// 查找顺序：providers 配置 → 环境变量（按提供商）
    pub fn get_credential(&self, key: &str) -> Option<String> {
        // 先从 providers 配置中查找
        if let Some(val) = self
            .providers
            .get(&self.active_provider)
            .and_then(|p| p.get(key))
            .filter(|v| !v.is_empty())
        {
            return Some(val.clone());
        }

        // 回退到环境变量（按提供商映射）
        match (self.active_provider.as_str(), key) {
            // doubao 新版控制台单一 API Key（X-Api-Key 鉴权），兼容旧 DOUBAO_APP_KEY
            ("doubao", "api_key") => std::env::var("DOUBAO_API_KEY")
                .ok()
                .or_else(|| std::env::var("DOUBAO_APP_KEY").ok()),
            ("qwen", "api_key") => std::env::var("QWEN_API_KEY").ok(),
            ("glm", "api_key") => std::env::var("GLM_API_KEY").ok(),
            ("mimo", "api_key") => std::env::var("MIMO_API_KEY").ok(),
            _ => None,
        }
    }

    /// 获取有效的 API Key（从当前激活提供商读取）
    pub fn resolved_api_key(&self) -> Result<String, String> {
        self.get_credential("api_key")
            .ok_or_else(|| "未设置 API Key（可在 settings.toml 或环境变量中设置）".to_string())
    }

    /// 获取指定提供商的指定凭证（不依赖 active_provider）
    pub fn get_provider_credential(&self, provider: &str, key: &str) -> Option<String> {
        self.providers
            .get(provider)
            .and_then(|p| p.get(key))
            .filter(|v| !v.is_empty())
            .cloned()
    }
}

/// TTS（语音合成）配置
///
/// 独立的基础能力，不绑定具体场景（xiaozhi 等），未来可被多种场景复用。
///
/// # 配置格式
///
/// 支持多服务商配置，通过 `active_provider` 切换当前使用的服务商：
///
/// ```toml
/// [tts]
/// active_provider = "doubao"
///
/// [tts.providers.doubao]
/// app_key = "..."
/// access_token = "..."
/// voice = "zh_female_xiaohe_uranus_bigtts"
/// cluster = "volcano_icl"
///
/// [tts.providers.qwen]
/// api_key = "..."
/// ```
///
/// # 向后兼容
///
/// 旧格式 `provider = "doubao"` + `app_key` / `access_token` 等字段在加载时自动迁移。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TtsConfig {
    /// 当前激活的 TTS 提供商
    #[serde(default = "default_tts_provider")]
    pub active_provider: String,
    /// 所有已配置的 TTS 提供商的凭证（{provider_name → {key → value}}）
    #[serde(default)]
    pub providers: HashMap<String, HashMap<String, String>>,
    /// 是否启用固定文本模式（开启后忽略 ASR 结果，播报固定文本）
    #[serde(default)]
    pub fixed_text_enabled: bool,
    /// 固定文本内容（仅 fixed_text_enabled 为 true 时生效）
    #[serde(default)]
    pub fixed_text: Option<String>,
    /// 是否在设备检测到唤醒词时主动播报问候（默认开启）
    #[serde(default)]
    pub wake_greeting_enabled: bool,
    /// 唤醒问候文案（None 或空串时回退为「你好」）
    #[serde(default)]
    pub wake_greeting: Option<String>,
    /// 无语音告别文案（None 或纯空白时回退为「拜拜」）
    #[serde(default)]
    pub no_speech_goodbye: Option<String>,
    /// 录音开始后累计无有效语音达到该毫秒数时，播报告别并关闭连接（0 表示禁用）
    #[serde(default = "default_no_speech_timeout_ms")]
    pub no_speech_timeout_ms: u64,
    /// 是否在等待 Agent 首个可播文本期间播报处理进度提示（默认开启，控制周期提示+超时兜底）
    #[serde(default)]
    pub thinking_feedback_enabled: bool,
    /// 进度提示播报间隔毫秒（0=禁用周期提示，仅保留超时兜底文案）
    #[serde(default = "default_thinking_feedback_interval_ms")]
    pub thinking_feedback_interval_ms: u64,
    /// 周期进度提示文案（None 或空串回退默认）
    #[serde(default)]
    pub thinking_feedback_text: Option<String>,
    /// 超时兜底文案（None 或空串回退默认）
    #[serde(default)]
    pub thinking_feedback_timeout_text: Option<String>,
}

/// 旧格式 TTS 配置（用于向后兼容反序列化）
#[derive(Debug, Clone, Deserialize)]
struct TtsConfigLegacy {
    provider: Option<String>,
    voice: Option<String>,
    app_key: Option<String>,
    cluster: Option<String>,
    resource_id: Option<String>,
    active_provider: Option<String>,
    providers: Option<HashMap<String, HashMap<String, String>>>,
    fixed_text_enabled: Option<bool>,
    fixed_text: Option<String>,
    wake_greeting_enabled: Option<bool>,
    wake_greeting: Option<String>,
    no_speech_goodbye: Option<String>,
    no_speech_timeout_ms: Option<u64>,
    thinking_feedback_enabled: Option<bool>,
    thinking_feedback_interval_ms: Option<u64>,
    thinking_feedback_text: Option<String>,
    thinking_feedback_timeout_text: Option<String>,
}

impl<'de> Deserialize<'de> for TtsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let legacy = TtsConfigLegacy::deserialize(deserializer)?;

        // 如果已有新格式字段，直接使用
        if let Some(active) = legacy.active_provider {
            return Ok(Self {
                active_provider: active,
                providers: legacy.providers.unwrap_or_default(),
                fixed_text_enabled: legacy.fixed_text_enabled.unwrap_or(false),
                fixed_text: legacy.fixed_text,
                wake_greeting_enabled: legacy.wake_greeting_enabled.unwrap_or(true),
                wake_greeting: legacy.wake_greeting,
                no_speech_goodbye: legacy.no_speech_goodbye,
                no_speech_timeout_ms: legacy
                    .no_speech_timeout_ms
                    .unwrap_or_else(default_no_speech_timeout_ms),
                thinking_feedback_enabled: legacy.thinking_feedback_enabled.unwrap_or(true),
                thinking_feedback_interval_ms: legacy
                    .thinking_feedback_interval_ms
                    .unwrap_or_else(default_thinking_feedback_interval_ms),
                thinking_feedback_text: legacy.thinking_feedback_text,
                thinking_feedback_timeout_text: legacy.thinking_feedback_timeout_text,
            });
        }

        // 旧格式迁移
        let provider = legacy.provider.unwrap_or_else(|| "doubao".to_string());
        let mut providers = legacy.providers.unwrap_or_default();

        // 如果旧格式有凭证值，迁移到 providers
        if !providers.contains_key(&provider) {
            let mut creds = HashMap::new();
            if let Some(val) = legacy.voice.filter(|s| !s.is_empty()) {
                creds.insert("voice".to_string(), val);
            }
            if let Some(val) = legacy.app_key.filter(|s| !s.is_empty()) {
                // 旧 app_key 迁移为新版 api_key（access_token 已废弃）
                creds.insert("api_key".to_string(), val);
            }
            if let Some(val) = legacy.cluster.filter(|s| !s.is_empty()) {
                creds.insert("cluster".to_string(), val);
            }
            if let Some(val) = legacy.resource_id.filter(|s| !s.is_empty()) {
                creds.insert("resource_id".to_string(), val);
            }
            if !creds.is_empty() {
                providers.insert(provider.clone(), creds);
            }
        }

        Ok(Self {
            active_provider: provider,
            providers,
            fixed_text_enabled: legacy.fixed_text_enabled.unwrap_or(false),
            fixed_text: legacy.fixed_text,
            wake_greeting_enabled: legacy.wake_greeting_enabled.unwrap_or(true),
            wake_greeting: legacy.wake_greeting,
            no_speech_goodbye: legacy.no_speech_goodbye,
            no_speech_timeout_ms: legacy
                .no_speech_timeout_ms
                .unwrap_or_else(default_no_speech_timeout_ms),
            thinking_feedback_enabled: legacy.thinking_feedback_enabled.unwrap_or(true),
            thinking_feedback_interval_ms: legacy
                .thinking_feedback_interval_ms
                .unwrap_or_else(default_thinking_feedback_interval_ms),
            thinking_feedback_text: legacy.thinking_feedback_text,
            thinking_feedback_timeout_text: legacy.thinking_feedback_timeout_text,
        })
    }
}

fn default_tts_provider() -> String {
    "doubao".to_string()
}

fn default_no_speech_timeout_ms() -> u64 {
    10000
}

fn default_thinking_feedback_interval_ms() -> u64 {
    15000
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            active_provider: default_tts_provider(),
            providers: HashMap::new(),
            fixed_text_enabled: false,
            fixed_text: None,
            wake_greeting_enabled: true,
            wake_greeting: None,
            no_speech_goodbye: None,
            no_speech_timeout_ms: default_no_speech_timeout_ms(),
            thinking_feedback_enabled: true,
            thinking_feedback_interval_ms: default_thinking_feedback_interval_ms(),
            thinking_feedback_text: None,
            thinking_feedback_timeout_text: None,
        }
    }
}

impl TtsConfig {
    /// 获取当前激活提供商的某个凭证值
    ///
    /// 查找顺序：providers 配置 → 环境变量（按提供商）
    pub fn get_credential(&self, key: &str) -> Option<String> {
        // 先从 providers 配置中查找
        if let Some(val) = self
            .providers
            .get(&self.active_provider)
            .and_then(|p| p.get(key))
            .filter(|v| !v.is_empty())
        {
            return Some(val.clone());
        }

        // 回退到环境变量（按提供商映射）
        match (self.active_provider.as_str(), key) {
            // doubao 新版控制台单一 API Key（X-Api-Key 鉴权），兼容旧 DOUBAO_APP_KEY
            ("doubao", "api_key") => std::env::var("DOUBAO_API_KEY")
                .ok()
                .or_else(|| std::env::var("DOUBAO_APP_KEY").ok()),
            ("doubao", "voice") => std::env::var("DOUBAO_VOICE_TYPE").ok(),
            ("doubao", "cluster") => std::env::var("DOUBAO_CLUSTER").ok(),
            ("qwen", "api_key") => std::env::var("QWEN_API_KEY").ok(),
            ("glm", "api_key") => std::env::var("GLM_API_KEY").ok(),
            ("openai", "api_key") => std::env::var("OPENAI_API_KEY").ok(),
            ("minimax", "api_key") => std::env::var("MINIMAX_API_KEY").ok(),
            ("xfyun", "app_id") => std::env::var("XFYUN_APP_ID").ok(),
            ("xfyun", "api_key") => std::env::var("XFYUN_API_KEY").ok(),
            ("xfyun", "api_secret") => std::env::var("XFYUN_API_SECRET").ok(),
            ("gemini", "api_key") => std::env::var("GEMINI_API_KEY").ok(),
            _ => None,
        }
    }

    /// 获取有效的 API Key（从当前激活提供商读取）
    pub fn resolved_api_key(&self) -> Result<String, String> {
        self.get_credential("api_key")
            .ok_or_else(|| "未设置 API Key（可在 settings.toml 或环境变量中设置）".to_string())
    }

    /// 获取有效的音色（兼容旧接口，从当前激活提供商读取）
    pub fn resolved_voice(&self) -> String {
        self.get_credential("voice")
            .unwrap_or_else(|| "zh_female_xiaohe_uranus_bigtts".to_string())
    }

    /// 获取有效的 Cluster（兼容旧接口，从当前激活提供商读取）
    pub fn resolved_cluster(&self) -> Option<String> {
        self.get_credential("cluster")
    }

    /// 获取 resource_id（兼容旧接口，从当前激活提供商读取）
    pub fn resolved_resource_id(&self) -> Option<String> {
        self.get_credential("resource_id")
    }

    /// 获取指定提供商的指定凭证（不依赖 active_provider）
    pub fn get_provider_credential(&self, provider: &str, key: &str) -> Option<String> {
        self.providers
            .get(provider)
            .and_then(|p| p.get(key))
            .filter(|v| !v.is_empty())
            .cloned()
    }
}

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
    /// ASR 语音识别配置（独立能力）
    #[serde(default)]
    pub asr: AsrConfig,
    /// TTS 语音合成配置（独立能力）
    #[serde(default)]
    pub tts: TtsConfig,
    /// GitHub Webhook 配置（后续方案 A 移入 connectors）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<crate::connectors::github::config::GitHubConfig>,
    /// Agent 调用日志配置
    #[serde(default)]
    pub agent_log: AgentLogConfig,
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
            asr: AsrConfig::default(),
            tts: TtsConfig::default(),
            github: None,
            agent_log: AgentLogConfig::default(),
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

/// 保存完整配置到 ~/.haimen/settings.toml
///
/// 会覆盖整个文件（包括其他配置节），TOML 注释不会被保留。
pub fn save_settings(config: &AppConfig) -> Result<(), String> {
    let file_path = get_settings_path();

    // 确保配置目录存在
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {}", e))?;
    }

    let content = toml::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {}", e))?;

    std::fs::write(&file_path, content).map_err(|e| format!("写入配置文件失败: {}", e))?;

    Ok(())
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
        assert_eq!(config.gateway.active_provider, "claude-code");
        assert!(config.http.enabled);
        assert_eq!(config.asr.active_provider, "doubao");
        assert!(config.asr.providers.is_empty());
        assert_eq!(config.tts.active_provider, "doubao");
        assert!(config.tts.providers.is_empty());
    }

    #[test]
    fn test_app_config_serde_roundtrip() {
        let mut providers = HashMap::new();
        let mut doubao_creds = HashMap::new();
        doubao_creds.insert("api_key".to_string(), "test-key".to_string());
        providers.insert("doubao".to_string(), doubao_creds);

        let config = AppConfig {
            debug: true,
            log_level: "warn".to_string(),
            gateway: GatewayConfig {
                active_provider: "claude-code".to_string(),
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
            asr: AsrConfig {
                active_provider: "doubao".to_string(),
                providers,
            },
            tts: TtsConfig {
                active_provider: "doubao".to_string(),
                providers: {
                    let mut m = HashMap::new();
                    let mut creds = HashMap::new();
                    creds.insert(
                        "voice".to_string(),
                        "zh_female_vv_uranus_bigtts".to_string(),
                    );
                    m.insert("doubao".to_string(), creds);
                    m
                },
                ..Default::default()
            },
            github: None,
            agent_log: AgentLogConfig::default(),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_gateway_config_default() {
        let config = GatewayConfig::default();
        assert_eq!(config.active_provider, "claude-code");
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

    // ─── GatewayConfig 向后兼容测试 ──────────────────────

    #[test]
    fn test_gateway_old_format_agent_migration() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[gateway]
agent = "claude-code"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.gateway.active_provider, "claude-code");
        });
    }

    #[test]
    fn test_gateway_old_format_agent_migration_to_codex() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[gateway]
agent = "codex"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.gateway.active_provider, "codex");
        });
    }

    #[test]
    fn test_gateway_new_format_active_provider() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[gateway]
active_provider = "codex"

[gateway.providers.codex]
extra = "value"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.gateway.active_provider, "codex");
            assert_eq!(
                result.gateway.get_credential("extra").as_deref(),
                Some("value")
            );
        });
    }

    #[test]
    fn test_gateway_resolved_agent() {
        let config = GatewayConfig {
            active_provider: "codex".to_string(),
            ..Default::default()
        };
        assert_eq!(config.resolved_agent(), "codex");
    }

    #[test]
    fn test_gateway_get_credential() {
        let mut providers = HashMap::new();
        let mut codex_creds = HashMap::new();
        codex_creds.insert("api_key".to_string(), "test-key".to_string());
        providers.insert("codex".to_string(), codex_creds);

        let config = GatewayConfig {
            active_provider: "codex".to_string(),
            providers,
            ..Default::default()
        };
        assert_eq!(
            config.get_credential("api_key").as_deref(),
            Some("test-key")
        );
        assert!(config.get_credential("nonexistent").is_none());
    }

    #[test]
    fn test_gateway_new_format_overrides_legacy() {
        run_with_temp_home(|home| {
            // 同时使用新格式和旧格式，新格式优先
            write_toml_settings(
                home,
                r#"
[gateway]
active_provider = "codex"
agent = "claude-code"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.gateway.active_provider, "codex");
        });
    }

    // ─── AsrConfig 测试 ───────────────────────────────────

    #[test]
    fn test_asr_config_default() {
        let cfg = AsrConfig::default();
        assert_eq!(cfg.active_provider, "doubao");
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn test_asr_resolved_api_key_from_config() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("api_key".to_string(), "config-key".to_string());
        providers.insert("doubao".to_string(), creds);

        let cfg = AsrConfig {
            active_provider: "doubao".to_string(),
            providers,
        };
        assert_eq!(cfg.resolved_api_key().unwrap(), "config-key");
    }

    #[test]
    fn test_asr_resolved_api_key_from_env() {
        unsafe {
            std::env::set_var("DOUBAO_API_KEY", "env-key");
        }
        let cfg = AsrConfig::default();
        assert_eq!(cfg.resolved_api_key().unwrap(), "env-key");
        unsafe {
            std::env::remove_var("DOUBAO_API_KEY");
        }
    }

    #[test]
    fn test_asr_resolved_api_key_config_overrides_env() {
        unsafe {
            std::env::set_var("DOUBAO_API_KEY", "env-key");
        }
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("api_key".to_string(), "config-key".to_string());
        providers.insert("doubao".to_string(), creds);

        let cfg = AsrConfig {
            active_provider: "doubao".to_string(),
            providers,
        };
        assert_eq!(cfg.resolved_api_key().unwrap(), "config-key");
        unsafe {
            std::env::remove_var("DOUBAO_API_KEY");
        }
    }

    #[test]
    fn test_asr_resolved_api_key_legacy_env_fallback() {
        unsafe {
            std::env::set_var("DOUBAO_APP_KEY", "legacy-env-key");
        }
        let cfg = AsrConfig::default();
        assert_eq!(cfg.resolved_api_key().unwrap(), "legacy-env-key");
        unsafe {
            std::env::remove_var("DOUBAO_APP_KEY");
        }
    }

    #[test]
    fn test_asr_resolved_api_key_missing() {
        let cfg = AsrConfig::default();
        let result = cfg.resolved_api_key();
        assert!(result.is_err());
    }

    #[test]
    fn test_asr_get_credential_different_provider() {
        let mut providers = HashMap::new();
        let mut qwen_creds = HashMap::new();
        qwen_creds.insert("api_key".to_string(), "qwen-key".to_string());
        providers.insert("qwen".to_string(), qwen_creds);
        let mut doubao_creds = HashMap::new();
        doubao_creds.insert("api_key".to_string(), "doubao-key".to_string());
        providers.insert("doubao".to_string(), doubao_creds);

        let cfg = AsrConfig {
            active_provider: "qwen".to_string(),
            providers,
        };
        assert_eq!(cfg.get_credential("api_key").unwrap(), "qwen-key");
        // qwen 没有 app_key
        assert!(cfg.get_credential("app_key").is_none());
    }

    #[test]
    fn test_asr_old_format_migration() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[asr]
provider = "doubao"
app_key = "old-key"
access_token = "old-token"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.asr.active_provider, "doubao");
            let doubao = result.asr.providers.get("doubao").unwrap();
            // 旧 app_key 迁移为新版 api_key，access_token 已废弃忽略
            assert_eq!(doubao.get("api_key").unwrap(), "old-key");
            assert!(doubao.get("access_key").is_none());
        });
    }

    #[test]
    fn test_asr_old_format_empty_migration() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[asr]
provider = "doubao"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.asr.active_provider, "doubao");
            // 没有凭证数据，providers 应为空
            assert!(result.asr.providers.is_empty());
        });
    }

    // ─── TtsConfig 测试 ───────────────────────────────────

    #[test]
    fn test_tts_config_default() {
        let cfg = TtsConfig::default();
        assert_eq!(cfg.active_provider, "doubao");
        assert!(cfg.providers.is_empty());
        assert!(cfg.thinking_feedback_enabled);
        assert_eq!(cfg.thinking_feedback_interval_ms, 15000);
        assert_eq!(cfg.thinking_feedback_text, None);
        assert_eq!(cfg.thinking_feedback_timeout_text, None);
    }

    #[test]
    fn test_tts_resolved_voice_from_config() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert(
            "voice".to_string(),
            "zh_female_vv_uranus_bigtts".to_string(),
        );
        providers.insert("doubao".to_string(), creds);
        let cfg = TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
            ..Default::default()
        };
        assert_eq!(cfg.resolved_voice(), "zh_female_vv_uranus_bigtts");
    }

    #[test]
    fn test_tts_resolved_voice_from_env() {
        unsafe {
            std::env::set_var("DOUBAO_VOICE_TYPE", "env-voice");
        }
        let cfg = TtsConfig::default();
        assert_eq!(cfg.resolved_voice(), "env-voice");
        unsafe {
            std::env::remove_var("DOUBAO_VOICE_TYPE");
        }
    }

    #[test]
    fn test_tts_resolved_voice_default() {
        let cfg = TtsConfig::default();
        let voice = cfg.resolved_voice();
        assert_eq!(voice, "zh_female_xiaohe_uranus_bigtts");
    }

    #[test]
    fn test_tts_resolved_voice_config_overrides_env() {
        unsafe {
            std::env::set_var("DOUBAO_VOICE_TYPE", "env-voice");
        }
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert(
            "voice".to_string(),
            "zh_female_vv_uranus_bigtts".to_string(),
        );
        providers.insert("doubao".to_string(), creds);
        let cfg = TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
            ..Default::default()
        };
        assert_eq!(cfg.resolved_voice(), "zh_female_vv_uranus_bigtts");
        unsafe {
            std::env::remove_var("DOUBAO_VOICE_TYPE");
        }
    }

    #[test]
    fn test_tts_resolved_cluster_from_config() {
        let mut providers = HashMap::new();
        let mut creds = HashMap::new();
        creds.insert("cluster".to_string(), "volcano_icl".to_string());
        providers.insert("doubao".to_string(), creds);
        let cfg = TtsConfig {
            active_provider: "doubao".to_string(),
            providers,
            ..Default::default()
        };
        assert_eq!(cfg.resolved_cluster().as_deref(), Some("volcano_icl"));
    }

    #[test]
    fn test_tts_resolved_cluster_none() {
        let cfg = TtsConfig::default();
        assert!(cfg.resolved_cluster().is_none());
    }

    #[test]
    fn test_tts_get_credential_multi_provider() {
        let mut providers = HashMap::new();
        let mut doubao_creds = HashMap::new();
        doubao_creds.insert("api_key".to_string(), "doubao-app-key".to_string());
        providers.insert("doubao".to_string(), doubao_creds);
        let mut qwen_creds = HashMap::new();
        qwen_creds.insert("api_key".to_string(), "qwen-key".to_string());
        providers.insert("qwen".to_string(), qwen_creds);

        // 当 active_provider 为 doubao 时
        let cfg = TtsConfig {
            active_provider: "doubao".to_string(),
            providers: providers.clone(),
            ..Default::default()
        };
        assert_eq!(cfg.get_credential("api_key").unwrap(), "doubao-app-key");

        // 切换为 qwen 时
        let cfg = TtsConfig {
            active_provider: "qwen".to_string(),
            providers,
            ..Default::default()
        };
        assert_eq!(cfg.get_credential("api_key").unwrap(), "qwen-key");
        assert!(cfg.get_credential("app_key").is_none());
    }

    #[test]
    fn test_tts_old_format_migration() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[tts]
provider = "doubao"
app_key = "old-key"
access_token = "old-token"
voice = "zh_female_vv_uranus_bigtts"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.tts.active_provider, "doubao");
            let doubao = result.tts.providers.get("doubao").unwrap();
            // 旧 app_key 迁移为新版 api_key，access_token 已废弃忽略
            assert_eq!(doubao.get("api_key").unwrap(), "old-key");
            assert!(doubao.get("access_token").is_none());
            assert_eq!(doubao.get("voice").unwrap(), "zh_female_vv_uranus_bigtts");
        });
    }

    #[test]
    fn test_tts_old_format_empty_migration() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[tts]
provider = "doubao"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.tts.active_provider, "doubao");
            assert!(result.tts.providers.is_empty());
        });
    }

    #[test]
    fn test_tts_new_format_direct() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[tts]
active_provider = "qwen"

[tts.providers.qwen]
api_key = "qwen-api-key"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.tts.active_provider, "qwen");
            assert_eq!(
                result
                    .tts
                    .providers
                    .get("qwen")
                    .unwrap()
                    .get("api_key")
                    .unwrap(),
                "qwen-api-key"
            );
        });
    }

    #[test]
    fn test_tts_wake_greeting_default_enabled() {
        let cfg = TtsConfig::default();
        assert!(cfg.wake_greeting_enabled, "唤醒问候默认应开启");
        assert_eq!(cfg.wake_greeting, None, "默认文案应为 None（回退「你好」）");
    }

    #[test]
    fn test_tts_wake_greeting_new_format() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[tts]
active_provider = "doubao"
wake_greeting_enabled = false
wake_greeting = "早上好"

[tts.providers.doubao]
api_key = "key"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert!(!result.tts.wake_greeting_enabled);
            assert_eq!(result.tts.wake_greeting.as_deref(), Some("早上好"));
        });
    }

    #[test]
    fn test_tts_wake_greeting_old_format_defaults_enabled() {
        // 旧格式迁移时未提供新字段，应回退为默认开启（保真既有行为不回归）
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                r#"
[tts]
provider = "doubao"
app_key = "old-key"
"#,
            );
            let result = load_settings().unwrap().unwrap();
            assert!(result.tts.wake_greeting_enabled);
            assert_eq!(result.tts.wake_greeting, None);
        });
    }

    #[test]
    fn test_tts_wake_greeting_roundtrip() {
        // save → load 往返，新字段不应丢失
        run_with_temp_home(|_home| {
            let mut providers = HashMap::new();
            let mut creds = HashMap::new();
            creds.insert("api_key".to_string(), "key".to_string());
            providers.insert("doubao".to_string(), creds);

            let config = AppConfig {
                tts: TtsConfig {
                    wake_greeting_enabled: true,
                    wake_greeting: Some("你好".to_string()),
                    ..Default::default()
                },
                ..AppConfig::default()
            };
            save_settings(&config).unwrap();
            let loaded = load_settings().unwrap().unwrap();
            assert!(loaded.tts.wake_greeting_enabled);
            assert_eq!(loaded.tts.wake_greeting.as_deref(), Some("你好"));
        });
    }

    // ─── save_settings 测试 ───────────────────────────────

    #[test]
    fn test_save_and_load_settings() {
        run_with_temp_home(|_home| {
            let mut providers = HashMap::new();
            let mut creds = HashMap::new();
            creds.insert("api_key".to_string(), "saved-key".to_string());
            providers.insert("doubao".to_string(), creds);

            let config = AppConfig {
                debug: true,
                asr: AsrConfig {
                    active_provider: "doubao".to_string(),
                    providers,
                },
                tts: TtsConfig {
                    active_provider: "doubao".to_string(),
                    providers: {
                        let mut m = HashMap::new();
                        let mut creds = HashMap::new();
                        creds.insert(
                            "voice".to_string(),
                            "zh_female_vv_uranus_bigtts".to_string(),
                        );
                        m.insert("doubao".to_string(), creds);
                        m
                    },
                    ..Default::default()
                },
                ..Default::default()
            };

            save_settings(&config).unwrap();

            let loaded = load_settings().unwrap().unwrap();
            assert_eq!(
                loaded
                    .asr
                    .providers
                    .get("doubao")
                    .and_then(|p| p.get("api_key")),
                Some(&"saved-key".to_string())
            );
            assert_eq!(
                loaded
                    .tts
                    .providers
                    .get("doubao")
                    .and_then(|p| p.get("voice")),
                Some(&"zh_female_vv_uranus_bigtts".to_string())
            );
        });
    }

    #[test]
    fn test_save_settings_creates_directory() {
        run_with_temp_home(|home| {
            let config = AppConfig::default();
            save_settings(&config).unwrap();
            assert!(home.join(".haimen/settings.toml").exists());
        });
    }

    // ---------------------------------------------------------------------------
    // Agent 调用日志配置
    // ---------------------------------------------------------------------------

    #[test]
    fn test_agent_log_config_default() {
        let cfg = AgentLogConfig::default();
        assert!(cfg.enabled, "默认应开启");
        assert!(cfg.dir.is_none(), "默认目录应为 None（用默认路径）");
        assert_eq!(cfg.retention_days, 30, "默认保留 30 天");
    }

    #[test]
    fn test_agent_log_config_toml_roundtrip() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[agent_log]\nenabled = true\nretention_days = 7\n");
            let cfg = load_settings().unwrap().unwrap();
            assert!(cfg.agent_log.enabled);
            assert_eq!(cfg.agent_log.retention_days, 7);
            assert!(cfg.agent_log.dir.is_none());
        });
    }

    #[test]
    fn test_agent_log_config_disabled_in_toml() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[agent_log]\nenabled = false\n");
            let cfg = load_settings().unwrap().unwrap();
            assert!(!cfg.agent_log.enabled);
        });
    }

    #[test]
    fn test_agent_log_config_missing_section_uses_default() {
        run_with_temp_home(|home| {
            // 无 [agent_log] 节 → 默认开启
            write_toml_settings(home, "[gateway]\nactive_provider = \"claude-code\"\n");
            let cfg = load_settings().unwrap().unwrap();
            assert!(cfg.agent_log.enabled);
            assert_eq!(cfg.agent_log.retention_days, 30);
        });
    }
}
