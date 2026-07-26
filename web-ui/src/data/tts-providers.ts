/** TTS 提供商元信息 */

export interface ProviderField {
  /** 字段键名（存到 providers[providerId][key]） */
  key: string;
  /** 界面显示标签 */
  label: string;
  /** 输入类型 */
  type: 'password' | 'text' | 'select';
  /** 空值时的占位提示 */
  placeholder?: string;
  /** select 类型的选项列表 */
  options?: string[];
}

export interface ProviderInfo {
  /** 唯一标识（如 "doubao" "qwen"） */
  id: string;
  /** 显示名称 */
  name: string;
  /** 配置字段列表 */
  fields: ProviderField[];
}

/** 所有支持的 TTS 服务商 */
export const TTS_PROVIDERS: ProviderInfo[] = [
  {
    id: 'doubao',
    name: '火山引擎',
    fields: [
      {
        key: 'app_key',
        label: 'App Key',
        type: 'password',
        placeholder: '未设置，可用环境变量 DOUBAO_APP_KEY',
      },
      {
        key: 'access_token',
        label: 'Access Token',
        type: 'password',
        placeholder: '未设置，可用环境变量 DOUBAO_ACCESS_TOKEN',
      },
    ],
  },
  {
    id: 'qwen',
    name: '阿里通义千问',
    fields: [
      { key: 'api_key', label: 'API Key', type: 'password' },
      {
        key: 'model',
        label: '模型',
        type: 'select',
        placeholder: 'cosyvoice-v3-flash',
        options: ['cosyvoice-v1', 'cosyvoice-v2', 'cosyvoice-v3-flash', 'cosyvoice-v3-plus'],
      },
    ],
  },
  {
    id: 'glm',
    name: '智谱AI',
    fields: [{ key: 'api_key', label: 'API Key', type: 'password' }],
  },
  {
    id: 'minimax',
    name: 'MiniMax',
    fields: [
      { key: 'api_key', label: 'API Key', type: 'password' },
      {
        key: 'model',
        label: '模型',
        type: 'select',
        placeholder: 'speech-2.8-hd',
        options: [
          'speech-2.8-hd',
          'speech-2.6-hd',
          'speech-2.6-turbo',
          'speech-2.5-turbo',
          'speech-02-hd',
          'speech-02',
          'speech-01-hd',
          'speech-01',
        ],
      },
    ],
  },
];

/** 按 id 查找提供商信息 */
export function getProviderInfo(id: string): ProviderInfo | undefined {
  return TTS_PROVIDERS.find((p) => p.id === id);
}
