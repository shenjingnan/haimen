// ── 通用 API 响应 ──

export interface ApiResponse<T = unknown> {
  success: boolean;
  data?: T;
  error?: string;
}

// ── 系统信息 ──

export interface SystemInfo {
  version: string;
  uptime: number;
  connectors: ConnectorStatus[];
  agents: AgentStatus[];
}

export interface ConnectorStatus {
  name: string;
  type: string;
  connected: boolean;
  error?: string;
}

export interface AgentStatus {
  name: string;
  type: string;
  available: boolean;
}

// ── 配置 ──

export interface GatewayConfig {
  connectors: ConnectorConfig[];
  agents: AgentConfig[];
  web: WebConfig;
}

export interface ConnectorConfig {
  name: string;
  type: 'lark' | 'github' | 'telegram';
  enabled: boolean;
  [key: string]: unknown;
}

export interface AgentConfig {
  name: string;
  type: 'claude_code' | 'mcp';
  enabled: boolean;
  [key: string]: unknown;
}

export interface WebConfig {
  host: string;
  port: number;
}

// ── 会话 ──

export interface Session {
  id: string;
  connector: string;
  created_at: string;
  updated_at: string;
  message_count: number;
}

// ── 日志 ──

export interface LogEntry {
  timestamp: string;
  level: 'info' | 'warn' | 'error' | 'debug';
  message: string;
  source?: string;
}

// ── 语音配置 ──

export interface AsrSettings {
  provider: string;
  app_key: string | null;
  access_token: string | null;
}

export interface TtsSettings {
  provider: string;
  voice: string | null;
  app_key: string | null;
  access_token: string | null;
  cluster: string | null;
  resource_id: string | null;
}

export interface TtsVoice {
  id: string;
  name: string;
  language: string;
}
