/** Agent 提供商元信息 */

export interface ProviderInfo {
  /** 唯一标识（如 "claude-code" "codex"） */
  id: string;
  /** 显示名称 */
  name: string;
}

/** 所有支持的 Agent 提供商 */
export const AGENT_PROVIDERS: ProviderInfo[] = [
  {
    id: 'claude-code',
    name: 'Claude Code',
  },
  {
    id: 'codex',
    name: 'Codex CLI',
  },
  {
    id: 'openclaw',
    name: 'OpenClaw',
  },
  {
    id: 'hermes',
    name: 'Hermes',
  },
];

/** 按 id 查找提供商信息 */
export function getAgentProviderInfo(id: string): ProviderInfo | undefined {
  return AGENT_PROVIDERS.find((p) => p.id === id);
}
