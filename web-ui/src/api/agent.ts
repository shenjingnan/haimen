import type { AgentSettings, ApiResponse } from '@/types';
import { apiFetch } from './client';

function ensureData<T>(res: ApiResponse<T>): T {
  if (res.data == null) throw new Error('Empty response');
  return res.data;
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
): Promise<{ valid: boolean; message: string }> {
  const res = await apiFetch<ApiResponse<{ valid: boolean; message: string }>>(
    '/api/v1/settings/agent/verify',
    {
      method: 'POST',
      body: JSON.stringify({ provider }),
    },
  );
  return ensureData(res);
}
