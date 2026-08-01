import { Eye, EyeOff } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import {
  getConnectorSettings,
  getConnectorsStatus,
  updateConnectorSettings,
} from '@/api/connectors';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Skeleton } from '@/components/ui/skeleton';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import type {
  ConnectorSettings as ConnectorSettingsData,
  ConnectorState,
  ConnectorStatusItem,
} from '@/types';

type ChannelId = 'lark' | 'dingtalk';

interface ChannelField {
  key: string;
  label: string;
  type?: 'text' | 'password';
  placeholder?: string;
}

interface ChannelToggle {
  key: string;
  label: string;
  hint: string;
}

interface ChannelDef {
  id: ChannelId;
  name: string;
  description: string;
  fields: ChannelField[];
  toggles: ChannelToggle[];
}

// 当前 Web 端仅展示飞书；钉钉暂不展示（后端能力保留，未来恢复只需在此添加条目）
const CHANNELS: ChannelDef[] = [
  {
    id: 'lark',
    name: '飞书',
    description: '通过 lark-cli 子进程桥接飞书 IM，需要本地安装并认证 lark-cli',
    fields: [],
    toggles: [],
  },
];

const STATUS_META: Record<ConnectorState, { label: string; className: string }> = {
  online: { label: '可用', className: 'bg-green-500/15 text-green-600 border-transparent' },
  auth_failed: {
    label: '认证失败',
    className: 'bg-destructive/10 text-destructive border-transparent',
  },
  misconfigured: {
    label: '配置不完整',
    className: 'bg-yellow-500/15 text-yellow-600 border-transparent',
  },
  disabled: { label: '未启用', className: 'text-muted-foreground' },
};

function PasswordInput({
  id,
  value,
  placeholder,
  onChange,
}: {
  id: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
}) {
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <Input
        id={id}
        type={show ? 'text' : 'password'}
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="pr-10"
      />
      <button
        type="button"
        aria-label={show ? '隐藏' : '显示'}
        onClick={() => setShow(!show)}
        tabIndex={-1}
        className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground cursor-pointer select-none"
      >
        {show ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
      </button>
    </div>
  );
}

function StatusBadge({ status }: { status: ConnectorState }) {
  const meta = STATUS_META[status];
  return (
    <Badge variant="outline" className={meta.className}>
      {meta.label}
    </Badge>
  );
}

/** 将后端返回的状态列表转换为按渠道 id 索引的映射 */
function toStatusMap(list: ConnectorStatusItem[]): Partial<Record<ChannelId, ConnectorStatusItem>> {
  const map: Partial<Record<ChannelId, ConnectorStatusItem>> = {};
  for (const item of list) {
    if (item.id === 'lark' || item.id === 'dingtalk') {
      map[item.id] = item;
    }
  }
  return map;
}

function ConnectorSettingsPanel() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [settings, setSettings] = useState<ConnectorSettingsData | null>(null);
  const [statusMap, setStatusMap] = useState<Partial<Record<ChannelId, ConnectorStatusItem>>>({});
  const [selectedTab, setSelectedTab] = useState<ChannelId>('lark');
  const [saving, setSaving] = useState(false);
  const [saveResult, setSaveResult] = useState<string | null>(null);
  const [verifying, setVerifying] = useState(false);
  const [verifyResult, setVerifyResult] = useState<{ valid: boolean; message: string } | null>(
    null,
  );

  const refreshStatus = useCallback(async () => {
    try {
      const list = await getConnectorsStatus();
      setStatusMap(toStatusMap(list));
    } catch {
      // 状态刷新失败保持已有状态
    }
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [cfg, statuses] = await Promise.all([getConnectorSettings(), getConnectorsStatus()]);
      setSettings(cfg);
      setStatusMap(toStatusMap(statuses));
    } catch {
      setError('加载消息渠道配置失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleFieldChange = (key: string, value: string) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev };
      (next[selectedTab] as unknown as Record<string, unknown>)[key] = value;
      return next;
    });
  };

  const handleToggle = (key: string, value: boolean) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev };
      (next[selectedTab] as unknown as Record<string, unknown>)[key] = value;
      return next;
    });
  };

  const handleSave = async () => {
    if (!settings) return;
    setSaving(true);
    try {
      const payload =
        selectedTab === 'lark' ? { lark: settings.lark } : { dingtalk: settings.dingtalk };
      const saved = await updateConnectorSettings(payload);
      setSettings(saved);
      setSaveResult('保存成功');
      setVerifyResult(null);
      await refreshStatus();
    } catch {
      setSaveResult('保存失败');
    } finally {
      setSaving(false);
      setTimeout(() => setSaveResult(null), 3000);
    }
  };

  const handleVerify = async () => {
    setVerifying(true);
    try {
      const list = await getConnectorsStatus();
      const map = toStatusMap(list);
      setStatusMap(map);
      const item = map[selectedTab];
      if (!item) {
        setVerifyResult({ valid: false, message: '未找到该渠道状态' });
      } else if (item.status === 'online') {
        setVerifyResult({ valid: true, message: '渠道可用' });
      } else {
        setVerifyResult({ valid: false, message: item.detail || '渠道不可用' });
      }
    } catch {
      setVerifyResult({ valid: false, message: '网络请求失败' });
    } finally {
      setVerifying(false);
    }
  };

  if (loading) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>消息渠道</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-48 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (error || !settings) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>消息渠道</CardTitle>
        </CardHeader>
        <CardContent>
          <Alert variant="destructive">
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error ?? '配置为空'}</AlertDescription>
          </Alert>
          <Button variant="outline" className="mt-2" onClick={load}>
            重试
          </Button>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">消息渠道</CardTitle>
      </CardHeader>
      <CardContent>
        <Tabs
          value={selectedTab}
          onValueChange={(v) => setSelectedTab(v as ChannelId)}
          className="w-full"
        >
          {/* Tab 栏 */}
          <TabsList>
            {CHANNELS.map((c) => {
              const status = statusMap[c.id]?.status;
              return (
                <TabsTrigger key={c.id} value={c.id}>
                  {c.name}
                  {status && (
                    <Badge
                      variant="outline"
                      className={`ml-1 text-[10px] px-1.5 ${STATUS_META[status].className}`}
                    >
                      {STATUS_META[status].label}
                    </Badge>
                  )}
                </TabsTrigger>
              );
            })}
          </TabsList>

          {/* 每个渠道的配置面板 */}
          {CHANNELS.map((c) => {
            const status = statusMap[c.id];
            return (
              <TabsContent key={c.id} value={c.id} className="space-y-4 mt-4">
                {/* 渠道名称和状态 */}
                <div className="flex items-center gap-2 text-sm text-muted-foreground">
                  <span className="font-medium text-foreground">{c.name}</span>
                  {status ? (
                    <>
                      <StatusBadge status={status.status} />
                      {status.detail && (
                        <span className="text-xs text-muted-foreground">{status.detail}</span>
                      )}
                    </>
                  ) : (
                    <Badge variant="outline" className="text-muted-foreground">
                      状态未知
                    </Badge>
                  )}
                </div>

                <p className="text-xs text-muted-foreground">{c.description}</p>

                {/* 启用开关 */}
                <div className="flex items-center justify-between rounded-lg border p-4">
                  <div className="space-y-0.5">
                    <span className="text-sm font-medium">启用该渠道</span>
                    <p className="text-xs text-muted-foreground">关闭后停止接收该渠道的消息</p>
                  </div>
                  <Switch
                    checked={settings[c.id].enabled}
                    onCheckedChange={(v) => handleToggle('enabled', v)}
                  />
                </div>

                {/* 动态字段 */}
                {c.fields.map((field) => (
                  <div key={field.key} className="space-y-2">
                    <span className="text-sm font-medium">{field.label}</span>
                    {field.type === 'password' ? (
                      <PasswordInput
                        id={`connector-${c.id}-${field.key}`}
                        placeholder={field.placeholder ?? `输入${field.label}`}
                        value={(settings[c.id] as unknown as Record<string, string>)[field.key]}
                        onChange={(v) => handleFieldChange(field.key, v)}
                      />
                    ) : (
                      <Input
                        id={`connector-${c.id}-${field.key}`}
                        type="text"
                        placeholder={field.placeholder ?? `输入${field.label}`}
                        value={(settings[c.id] as unknown as Record<string, string>)[field.key]}
                        onChange={(e) => handleFieldChange(field.key, e.target.value)}
                      />
                    )}
                  </div>
                ))}

                {/* 额外开关 */}
                {c.toggles.map((t) => (
                  <div
                    key={t.key}
                    className="flex items-center justify-between rounded-lg border p-4"
                  >
                    <div className="space-y-0.5">
                      <span className="text-sm font-medium">{t.label}</span>
                      <p className="text-xs text-muted-foreground">{t.hint}</p>
                    </div>
                    <Switch
                      checked={Boolean(
                        (settings[c.id] as unknown as Record<string, unknown>)[t.key],
                      )}
                      onCheckedChange={(v) => handleToggle(t.key, v)}
                    />
                  </div>
                ))}

                {/* 验证结果 */}
                {verifyResult && (
                  <Alert variant={verifyResult.valid ? 'default' : 'destructive'}>
                    <AlertTitle>{verifyResult.valid ? '验证通过' : '验证失败'}</AlertTitle>
                    <AlertDescription>{verifyResult.message}</AlertDescription>
                  </Alert>
                )}

                {/* 操作按钮 */}
                <div className="flex items-center gap-2 pt-2 flex-wrap">
                  <Button variant="outline" onClick={handleVerify} disabled={verifying}>
                    {verifying ? '验证中...' : '验证可用性'}
                  </Button>
                  <Button onClick={handleSave} disabled={saving}>
                    {saving ? '保存中...' : '保存配置'}
                  </Button>
                  {saveResult && (
                    <span
                      className={`text-sm ${
                        saveResult === '保存成功' ? 'text-green-600' : 'text-red-600'
                      }`}
                    >
                      {saveResult}
                    </span>
                  )}
                </div>
              </TabsContent>
            );
          })}
        </Tabs>
      </CardContent>
    </Card>
  );
}

// ─── 页面 ───────────────────────────────────────────────────

export default function ConnectorSettings() {
  return (
    <div className="mx-auto max-w-2xl space-y-8 py-8 px-4">
      <div>
        <h1 className="text-2xl font-bold tracking-tight">消息渠道</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          配置飞书 / 钉钉 IM 渠道并查看可用状态。可用状态基于配置完整性与认证探测得出。
        </p>
      </div>
      <ConnectorSettingsPanel />
    </div>
  );
}
