import type { AgentLogRecord, AgentLogSource, AgentLogStatus } from '@/types';

/** 无 chat_id 记录（cli 来源）归入的分组哨兵 key */
export const UNGROUPED = '__none__';

export interface ConversationSummary {
  chatId: string;
  displayChatId: string;
  source: AgentLogSource;
  messageCount: number;
  firstTime: string;
  lastTime: string;
  lastStatus: AgentLogStatus;
  preview: string;
}

const SOURCE_LABELS: Record<AgentLogSource, string> = {
  gateway: '网关',
  xiaozhi: '语音',
  cli: 'CLI',
};

/** 记录状态展示元信息（Badge 着色） */
export const STATUS_META: Record<AgentLogStatus, { label: string; className: string }> = {
  success: { label: '成功', className: 'bg-green-500/15 text-green-600 border-transparent' },
  error: { label: '失败', className: 'bg-destructive/10 text-destructive border-transparent' },
  timeout: { label: '超时', className: 'bg-yellow-500/15 text-yellow-600 border-transparent' },
};

export function sourceLabel(source: AgentLogSource): string {
  return SOURCE_LABELS[source] ?? source;
}

/**
 * 聚合：按 chat_id 将日志记录分组成会话摘要，按最近活跃（lastTime）倒序。
 * records 来自后端（时间倒序），首个遇到的即组内最新，但聚合仍做比较以保证健壮。
 */
export function groupByChat(records: AgentLogRecord[]): ConversationSummary[] {
  const map = new Map<string, ConversationSummary>();
  for (const r of records) {
    const chatId = r.chat_id ?? UNGROUPED;
    const prev = map.get(chatId);
    if (!prev) {
      map.set(chatId, {
        chatId,
        displayChatId: r.chat_id ?? '(未分组)',
        source: r.source,
        messageCount: 1,
        firstTime: r.timestamp,
        lastTime: r.timestamp,
        lastStatus: r.status,
        preview: (r.input || r.output || '').slice(0, 80),
      });
      continue;
    }
    prev.messageCount += 1;
    if (r.timestamp < prev.firstTime) prev.firstTime = r.timestamp;
    // preview 与 lastStatus 都指向组内最新一条
    if (r.timestamp > prev.lastTime) {
      prev.lastTime = r.timestamp;
      prev.lastStatus = r.status;
      prev.preview = (r.input || r.output || '').slice(0, 80);
    }
    map.set(chatId, prev);
  }
  return [...map.values()].sort((a, b) => (a.lastTime < b.lastTime ? 1 : -1));
}

/** 格式化 ISO 时间戳 → `MM-DD HH:mm`，非法输入原样返回 */
export function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 今天日期字符串 `YYYY-MM-DD`（本地时区） */
export function todayStr(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}
