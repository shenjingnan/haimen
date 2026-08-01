import type { ApiResponse, ConnectorSettings, ConnectorStatusItem } from '@/types';
import { apiFetch } from './client';

function ensureData<T>(res: ApiResponse<T>): T {
  if (res.data == null) throw new Error('Empty response');
  return res.data;
}

// ── 消息渠道状态 ──

/** 获取每个渠道的可用状态（L1 配置层 + L2 认证层） */
export async function getConnectorsStatus(): Promise<ConnectorStatusItem[]> {
  const res = await apiFetch<ApiResponse<{ connectors: ConnectorStatusItem[] }>>(
    '/api/v1/connectors/status',
  );
  return ensureData(res).connectors;
}

// ── 消息渠道配置 ──

export async function getConnectorSettings(): Promise<ConnectorSettings> {
  const res = await apiFetch<ApiResponse<ConnectorSettings>>('/api/v1/settings/connectors');
  return ensureData(res);
}

export async function updateConnectorSettings(
  settings: Partial<ConnectorSettings>,
): Promise<ConnectorSettings> {
  const res = await apiFetch<ApiResponse<ConnectorSettings>>('/api/v1/settings/connectors', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
  return ensureData(res);
}
