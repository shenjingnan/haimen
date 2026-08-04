import { ChevronRight } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { getAgentLogs } from '@/api/agentLogs';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Skeleton } from '@/components/ui/skeleton';
import {
  formatTime,
  groupByChat,
  prettyJson,
  STATUS_META,
  sourceLabel,
  todayStr,
  UNGROUPED,
} from '@/lib/agentLogs';
import type { AgentLogEvent, AgentLogRecord, AgentLogSource, AgentLogStatus } from '@/types';

const MAX_LIMIT = 5000;
const INITIAL_LIMIT = 200;

const SOURCES: { value: AgentLogSource | 'all'; label: string }[] = [
  { value: 'all', label: '全部来源' },
  { value: 'gateway', label: '网关' },
  { value: 'xiaozhi', label: '语音' },
  { value: 'cli', label: 'CLI' },
];

const STATUSES: { value: AgentLogStatus | 'all'; label: string }[] = [
  { value: 'all', label: '全部状态' },
  { value: 'success', label: '成功' },
  { value: 'error', label: '失败' },
  { value: 'timeout', label: '超时' },
];

/** 筛选用的胶囊按钮 */
function FilterChip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`px-2.5 py-1 rounded-full text-xs cursor-pointer border ${
        active
          ? 'bg-primary text-primary-foreground border-transparent'
          : 'border-input text-muted-foreground hover:bg-muted'
      }`}
    >
      {children}
    </button>
  );
}

/** 事件在记录内是静态不可变列表，key 只需在本条记录内稳定即可 */
function eventKey(ev: AgentLogEvent, i: number): string {
  if (ev.type === 'tool_use') return `tool_use:${ev.id}`;
  if (ev.type === 'tool_result') return `tool_result:${ev.tool_use_id}`;
  return `thinking:${i}`;
}

/** 内容轨迹事件展示：思考 / 工具调用 / 工具结果（可折叠） */
function TraceEvent({ ev }: { ev: AgentLogEvent }) {
  if (ev.type === 'thinking') {
    return (
      <details className="group rounded-md border border-input bg-muted/50 px-2 py-1.5 text-xs text-muted-foreground">
        <summary className="flex cursor-pointer select-none list-none items-center gap-1.5 [&::-webkit-details-marker]:hidden">
          <ChevronRight className="h-3 w-3 transition-transform group-open:rotate-90" />💭 思考
        </summary>
        <pre className="mt-1.5 whitespace-pre-wrap break-words text-foreground">{ev.thinking}</pre>
      </details>
    );
  }

  if (ev.type === 'tool_use') {
    return (
      <div className="rounded-md border border-input bg-muted/50 px-2 py-1.5 text-xs">
        <div className="flex items-center gap-1.5 font-medium text-foreground">
          🔧 工具调用
          <span className="text-muted-foreground">{ev.name}</span>
        </div>
        <details className="group mt-1 text-muted-foreground">
          <summary className="flex cursor-pointer select-none list-none items-center gap-1.5 [&::-webkit-details-marker]:hidden">
            <ChevronRight className="h-3 w-3 transition-transform group-open:rotate-90" />
            入参
          </summary>
          <pre className="mt-1.5 whitespace-pre-wrap break-words text-foreground">
            {prettyJson(ev.input)}
          </pre>
        </details>
      </div>
    );
  }

  // tool_result
  return (
    <div className="rounded-md border border-input bg-muted/50 px-2 py-1.5 text-xs">
      <div className="flex items-center gap-1.5 font-medium text-foreground">
        ⚙️ 工具结果
        {ev.is_error && (
          <Badge
            variant="outline"
            className="bg-destructive/10 text-destructive border-transparent"
          >
            出错
          </Badge>
        )}
      </div>
      <details className="group mt-1 text-muted-foreground">
        <summary className="flex cursor-pointer select-none list-none items-center gap-1.5 [&::-webkit-details-marker]:hidden">
          <ChevronRight className="h-3 w-3 transition-transform group-open:rotate-90" />
          结果
        </summary>
        <pre className="mt-1.5 whitespace-pre-wrap break-words text-foreground">{ev.content}</pre>
      </details>
    </div>
  );
}

/** 单条调用记录的气泡（用户输入 + 内容轨迹 + Agent 输出/错误） */
function MessageBubble({ rec }: { rec: AgentLogRecord }) {
  const meta = STATUS_META[rec.status];
  return (
    <div className="flex flex-col gap-1.5">
      {/* 元信息行 */}
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>{formatTime(rec.timestamp)}</span>
        <span className="font-medium text-foreground">{rec.agent}</span>
        <Badge variant="outline" className={meta.className}>
          {meta.label}
        </Badge>
        <span>{rec.latency_ms} ms</span>
        {rec.connector && <span>{rec.connector}</span>}
      </div>
      {/* 用户输入（左） */}
      <div className="flex">
        <div className="max-w-[75%] rounded-lg rounded-tl-sm bg-muted px-3 py-2 text-sm whitespace-pre-wrap break-words">
          {rec.input}
        </div>
      </div>
      {/* 内容轨迹（思考 / 工具调用 / 工具结果），介于输入与最终输出之间 */}
      {rec.events && rec.events.length > 0 && (
        <div className="flex flex-col gap-1.5">
          {rec.events.map((ev, i) => (
            <TraceEvent key={eventKey(ev, i)} ev={ev} />
          ))}
        </div>
      )}
      {/* Agent 输出（右）；失败/超时展示错误信息 */}
      <div className="flex justify-end">
        {rec.output != null ? (
          <div className="max-w-[75%] rounded-lg rounded-tr-sm bg-primary/10 px-3 py-2 text-sm whitespace-pre-wrap break-words">
            {rec.output}
          </div>
        ) : rec.error ? (
          <Alert variant="destructive" className="max-w-[75%]">
            <AlertTitle>{rec.status === 'timeout' ? '调用超时' : '调用失败'}</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap break-words">
              {rec.error}
            </AlertDescription>
          </Alert>
        ) : null}
      </div>
    </div>
  );
}

/** 右侧会话详情：会话头 + 按时间正序的气泡流 */
function ConversationDetail({ records, chatId }: { records: AgentLogRecord[]; chatId: string }) {
  const convRecords = useMemo(
    () => records.filter((r) => (r.chat_id ?? UNGROUPED) === chatId).reverse(),
    [records, chatId],
  );
  const summary = groupByChat(convRecords)[0];

  return (
    <div className="h-full flex flex-col">
      {summary && (
        <div className="sticky top-0 z-10 border-b bg-background/95 backdrop-blur px-4 py-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium truncate">{summary.displayChatId}</span>
            <Badge variant="outline">{sourceLabel(summary.source)}</Badge>
            <span className="text-xs text-muted-foreground">{summary.messageCount} 条</span>
          </div>
        </div>
      )}
      <div className="flex-1 space-y-4 p-4">
        {convRecords.map((r) => (
          <MessageBubble key={r.timestamp} rec={r} />
        ))}
      </div>
    </div>
  );
}

function AgentLogsPanel() {
  const [day, setDay] = useState<string>(todayStr());
  const [source, setSource] = useState<AgentLogSource | 'all'>('all');
  const [status, setStatus] = useState<AgentLogStatus | 'all'>('all');
  const [limit, setLimit] = useState(INITIAL_LIMIT);
  const [records, setRecords] = useState<AgentLogRecord[]>([]);
  const [enabled, setEnabled] = useState(true);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedChatId, setSelectedChatId] = useState<string | null>(null);

  const load = useCallback(
    async (nextLimit: number) => {
      setLoading(true);
      setError(null);
      try {
        const data = await getAgentLogs({
          day: day || undefined,
          source: source === 'all' ? undefined : source,
          status: status === 'all' ? undefined : status,
          limit: nextLimit,
        });
        setEnabled(data.enabled);
        setRecords(data.records);
      } catch {
        setError('加载对话记录失败');
      } finally {
        setLoading(false);
      }
    },
    [day, source, status],
  );

  // 筛选条件变化时重置分页并重新加载
  useEffect(() => {
    setLimit(INITIAL_LIMIT);
    load(INITIAL_LIMIT);
  }, [load]);

  const conversations = useMemo(() => groupByChat(records), [records]);

  // 数据刷新后保持有效选中（当前会话被筛掉时自动切到第一个）
  useEffect(() => {
    if (conversations.length === 0) {
      setSelectedChatId(null);
      return;
    }
    setSelectedChatId((prev) =>
      prev && conversations.some((c) => c.chatId === prev) ? prev : conversations[0].chatId,
    );
  }, [conversations]);

  const handleLoadMore = () => {
    const next = limit >= MAX_LIMIT ? MAX_LIMIT : Math.min(limit * 3, MAX_LIMIT);
    setLimit(next);
    load(next);
  };

  if (loading && records.length === 0) {
    return (
      <Card>
        <CardContent className="p-4">
          <Skeleton className="h-[480px] w-full" />
        </CardContent>
      </Card>
    );
  }

  if (error) {
    return (
      <Card>
        <CardContent className="p-4">
          <Alert variant="destructive">
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
          <Button variant="outline" className="mt-2" onClick={() => load(limit)}>
            重试
          </Button>
        </CardContent>
      </Card>
    );
  }

  if (!enabled) {
    return (
      <Card>
        <CardContent className="p-4">
          <Alert>
            <AlertTitle>Agent 日志未启用</AlertTitle>
            <AlertDescription>
              请在 <code className="text-xs bg-muted px-1 rounded">~/.haimen/settings.toml</code> 中
              设置 <code className="text-xs bg-muted px-1 rounded">[agent_log] enabled = true</code>{' '}
              以记录每次 Agent 调用。
            </AlertDescription>
          </Alert>
        </CardContent>
      </Card>
    );
  }

  const selected = conversations.find((c) => c.chatId === selectedChatId) ?? null;
  const hasMore = records.length >= limit && limit < MAX_LIMIT;

  return (
    <Card>
      <CardContent className="p-4">
        {/* 筛选条 */}
        <div className="mb-4 flex flex-wrap items-center gap-2">
          <Input
            type="date"
            value={day}
            onChange={(e) => setDay(e.target.value)}
            className="w-[150px] h-8 text-xs"
            aria-label="日期"
            title="按日期过滤，清空为全部日期"
          />
          <div className="flex flex-wrap items-center gap-1.5">
            {SOURCES.map((s) => (
              <FilterChip
                key={s.value}
                active={source === s.value}
                onClick={() => setSource(s.value)}
              >
                {s.label}
              </FilterChip>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {STATUSES.map((s) => (
              <FilterChip
                key={s.value}
                active={status === s.value}
                onClick={() => setStatus(s.value)}
              >
                {s.label}
              </FilterChip>
            ))}
          </div>
          <Button
            variant="outline"
            size="sm"
            className="ml-auto"
            onClick={() => load(limit)}
            disabled={loading}
          >
            {loading ? '加载中...' : '刷新'}
          </Button>
        </div>

        {/* 主体两栏 */}
        <div className="grid grid-cols-[320px_1fr] gap-4 h-[calc(100vh-300px)] min-h-[360px]">
          {/* 左侧会话列表 */}
          <div className="overflow-y-auto rounded-md border">
            {conversations.length === 0 ? (
              <div className="p-8 text-center text-sm text-muted-foreground">暂无对话记录</div>
            ) : (
              conversations.map((c) => (
                <button
                  type="button"
                  key={c.chatId}
                  onClick={() => setSelectedChatId(c.chatId)}
                  className={`w-full text-left px-3 py-2.5 border-b last:border-b-0 cursor-pointer hover:bg-muted/60 ${
                    selectedChatId === c.chatId ? 'bg-muted' : ''
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-medium truncate">{c.displayChatId}</span>
                    <Badge variant="outline" className={STATUS_META[c.lastStatus].className}>
                      {STATUS_META[c.lastStatus].label}
                    </Badge>
                  </div>
                  <div className="mt-0.5 text-xs text-muted-foreground truncate">{c.preview}</div>
                  <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                    <span>{sourceLabel(c.source)}</span>
                    <span>{c.messageCount} 条</span>
                    <span>{formatTime(c.lastTime)}</span>
                  </div>
                </button>
              ))
            )}
            {hasMore && (
              <div className="p-2">
                <Button variant="ghost" size="sm" className="w-full" onClick={handleLoadMore}>
                  加载更多
                </Button>
              </div>
            )}
          </div>

          {/* 右侧会话详情 */}
          <div className="overflow-y-auto rounded-md border">
            {selected ? (
              <ConversationDetail records={records} chatId={selected.chatId} />
            ) : (
              <div className="h-full flex items-center justify-center text-sm text-muted-foreground">
                选择左侧会话查看对话
              </div>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

// ─── 页面 ───────────────────────────────────────────────────

export default function AgentLogs() {
  return (
    <div className="mx-auto max-w-6xl space-y-8 py-8 px-4">
      <div>
        <h1 className="text-2xl font-bold tracking-tight">对话记录</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          浏览每次 Agent 调用的输入与回复，按会话分组展示。
        </p>
      </div>
      <AgentLogsPanel />
    </div>
  );
}
