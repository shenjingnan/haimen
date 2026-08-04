import { describe, expect, it } from 'vitest';
import type { AgentLogRecord } from '@/types';
import { formatTime, groupByChat, todayStr, UNGROUPED } from './agentLogs';

function makeRecord(overrides: Partial<AgentLogRecord> & { timestamp: string }): AgentLogRecord {
  return {
    source: 'gateway',
    agent: 'claude-code',
    connector: null,
    chat_id: 'chat-1',
    sender_id: null,
    session_id: null,
    work_dir: '/tmp',
    input: '你好',
    output: '你好！',
    status: 'success',
    error: null,
    latency_ms: 100,
    ...overrides,
  };
}

describe('groupByChat', () => {
  it('按 chat_id 聚合多条记录，聚合首末时间与消息数', () => {
    const records = [
      makeRecord({ timestamp: '2026-08-04T12:00:00+08:00', chat_id: 'chat-a' }),
      makeRecord({ timestamp: '2026-08-04T11:00:00+08:00', chat_id: 'chat-a' }),
      makeRecord({ timestamp: '2026-08-04T10:00:00+08:00', chat_id: 'chat-b' }),
    ];
    const groups = groupByChat(records);
    expect(groups).toHaveLength(2);
    const a = groups.find((g) => g.chatId === 'chat-a');
    expect(a?.messageCount).toBe(2);
    expect(a?.firstTime).toBe('2026-08-04T11:00:00+08:00');
    expect(a?.lastTime).toBe('2026-08-04T12:00:00+08:00');
  });

  it('按最近活跃（lastTime）倒序排会话', () => {
    const records = [
      makeRecord({ timestamp: '2026-08-04T09:00:00+08:00', chat_id: 'old' }),
      makeRecord({ timestamp: '2026-08-04T12:00:00+08:00', chat_id: 'new' }),
    ];
    const groups = groupByChat(records);
    expect(groups.map((g) => g.chatId)).toEqual(['new', 'old']);
  });

  it('chat_id 为 null 的记录归入哨兵分组 (未分组)', () => {
    const records = [makeRecord({ timestamp: '2026-08-04T10:00:00+08:00', chat_id: null })];
    const groups = groupByChat(records);
    expect(groups[0].chatId).toBe(UNGROUPED);
    expect(groups[0].displayChatId).toBe('(未分组)');
  });

  it('lastStatus 取组内最新一条的状态', () => {
    const records = [
      makeRecord({ timestamp: '2026-08-04T12:00:00+08:00', chat_id: 'chat-a', status: 'success' }),
      makeRecord({ timestamp: '2026-08-04T11:00:00+08:00', chat_id: 'chat-a', status: 'error' }),
    ];
    const groups = groupByChat(records);
    expect(groups[0].lastStatus).toBe('success');
  });

  it('preview 取最新一条的输入摘要（截断 80 字符）', () => {
    const long = 'x'.repeat(100);
    const records = [
      makeRecord({ timestamp: '2026-08-04T12:00:00+08:00', chat_id: 'chat-a', input: long }),
      makeRecord({ timestamp: '2026-08-04T11:00:00+08:00', chat_id: 'chat-a', input: '旧' }),
    ];
    const groups = groupByChat(records);
    expect(groups[0].preview).toBe(long.slice(0, 80));
  });
});

describe('formatTime', () => {
  it('ISO 时间格式化为 MM-DD HH:mm', () => {
    expect(formatTime('2026-08-04T09:05:00+08:00')).toBe('08-04 09:05');
  });

  it('非法时间返回原串', () => {
    expect(formatTime('not-a-date')).toBe('not-a-date');
  });
});

describe('todayStr', () => {
  it('返回今天 YYYY-MM-DD', () => {
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, '0');
    expect(todayStr()).toBe(`${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`);
  });
});
