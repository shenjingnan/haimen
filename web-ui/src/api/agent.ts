import { AGENT_PROVIDERS, type ProviderInfo } from '@/data/agent-providers';
import type { AgentSettings, ApiResponse } from '@/types';
import { apiFetch } from './client';

function ensureData<T>(res: ApiResponse<T>): T {
  if (res.data == null) throw new Error('Empty response');
  return res.data;
}

/** 后端返回的 provider 原始结构（display_name） */
interface RawProvider {
  id: string;
  display_name: string;
}

/**
 * 从注册表拉取所有可用 Agent 提供商（后端驱动，新增 Agent 前端零改动）。
 * 返回结构归一化为前端的 ProviderInfo（name 字段），字段定义从静态列表合并
 * （后端只报 id + 显示名，字段 schema 属前端声明）。
 */
export async function getAgentProviders(): Promise<ProviderInfo[]> {
  const res = await apiFetch<ApiResponse<{ providers: RawProvider[] }>>('/api/v1/agent/providers');
  const data = ensureData(res);
  return (data.providers ?? []).map((p) => ({
    id: p.id,
    name: p.display_name,
    fields: AGENT_PROVIDERS.find((s) => s.id === p.id)?.fields ?? [],
  }));
}

export async function getAgentSettings(): Promise<AgentSettings> {
  const res = await apiFetch<ApiResponse<AgentSettings>>('/api/v1/settings/agent');
  return ensureData(res);
}

export async function updateAgentSettings(settings: {
  active_provider?: string;
  providers?: Record<string, Record<string, string>>;
}): Promise<AgentSettings> {
  const res = await apiFetch<ApiResponse<AgentSettings>>('/api/v1/settings/agent', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
  return ensureData(res);
}

export async function verifyAgentCredentials(
  provider: string,
  cliPath?: string,
): Promise<{ valid: boolean; message: string }> {
  const res = await apiFetch<ApiResponse<{ valid: boolean; message: string }>>(
    '/api/v1/settings/agent/verify',
    {
      method: 'POST',
      body: JSON.stringify({ provider, cli_path: cliPath ?? '' }),
    },
  );
  return ensureData(res);
}
