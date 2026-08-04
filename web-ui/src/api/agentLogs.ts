import type { AgentLogsData, ApiResponse } from '@/types';
import { apiFetch } from './client';

function ensureData<T>(res: ApiResponse<T>): T {
  if (res.data == null) throw new Error('Empty response');
  return res.data;
}

export interface AgentLogsParams {
  /** YYYY-MM-DD，缺省扫全部日期 */
  day?: string;
  /** 来源：gateway | xiaozhi | cli */
  source?: string;
  /** 状态：success | error | timeout */
  status?: string;
  /** chat_id 精确匹配 */
  chat?: string;
  /** 返回条数上限（后端钳制 1..=5000） */
  limit?: number;
}

/** 查询 Agent 调用日志（按时间倒序） */
export async function getAgentLogs(params: AgentLogsParams = {}): Promise<AgentLogsData> {
  const qs = new URLSearchParams();
  if (params.day) qs.set('day', params.day);
  if (params.source) qs.set('source', params.source);
  if (params.status) qs.set('status', params.status);
  if (params.chat) qs.set('chat', params.chat);
  if (params.limit != null) qs.set('limit', String(params.limit));
  const res = await apiFetch<ApiResponse<AgentLogsData>>(`/api/v1/agent/logs?${qs.toString()}`);
  return ensureData(res);
}
