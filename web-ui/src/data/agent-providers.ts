/** Agent 提供商元信息 */

/** 单条可配置字段（沿用 ASR/TTS 的 ProviderField 本地声明模式） */
export interface ProviderField {
  /** 配置 key，对应 settings.toml 中 providers.<id>.key */
  key: string;
  /** 界面显示名 */
  label: string;
  /** 输入控件类型（当前 Agent 仅用 text） */
  type: 'password' | 'text' | 'select';
  placeholder?: string;
  options?: string[];
}

export interface ProviderInfo {
  /** 唯一标识（如 "claude-code" "codex"） */
  id: string;
  /** 显示名称 */
  name: string;
  /** 该提供商的可配置字段（如 cli_path） */
  fields: ProviderField[];
}

/** 所有支持的 Agent 提供商 */
export const AGENT_PROVIDERS: ProviderInfo[] = [
  {
    id: 'claude-code',
    name: 'Claude Code',
    fields: [
      {
        key: 'cli_path',
        label: 'CLI 路径',
        type: 'text',
        placeholder: '留空使用 PATH 查找 claude',
      },
    ],
  },
  {
    id: 'codex',
    name: 'Codex CLI',
    fields: [
      {
        key: 'cli_path',
        label: 'CLI 路径',
        type: 'text',
        placeholder: '留空使用 PATH 查找 codex',
      },
    ],
  },
  {
    id: 'openclaw',
    name: 'OpenClaw',
    fields: [
      {
        key: 'cli_path',
        label: 'CLI 路径',
        type: 'text',
        placeholder: '留空使用 PATH 查找 openclaw',
      },
    ],
  },
  {
    id: 'hermes',
    name: 'Hermes',
    fields: [
      {
        key: 'cli_path',
        label: 'CLI 路径',
        type: 'text',
        placeholder: '留空使用 PATH 查找 hermes',
      },
    ],
  },
];

/** 按 id 查找提供商信息 */
export function getAgentProviderInfo(id: string): ProviderInfo | undefined {
  return AGENT_PROVIDERS.find((p) => p.id === id);
}
