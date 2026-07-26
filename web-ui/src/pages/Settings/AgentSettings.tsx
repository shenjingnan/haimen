import { useCallback, useEffect, useState } from 'react';
import { getAgentSettings, updateAgentSettings, verifyAgentCredentials } from '@/api/agent';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { AGENT_PROVIDERS } from '@/data/agent-providers';

function AgentSettingsPanel() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editState, setEditState] = useState<Record<string, Record<string, string>>>({});
  const [activeProvider, setActiveProvider] = useState('claude-code');
  const [selectedTab, setSelectedTab] = useState('claude-code');
  const [saving, setSaving] = useState(false);
  const [saveResult, setSaveResult] = useState<string | null>(null);
  const [verifying, setVerifying] = useState(false);
  const [verifyResult, setVerifyResult] = useState<{ valid: boolean; message: string } | null>(
    null,
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getAgentSettings();
      setEditState(data.providers ?? {});
      setActiveProvider(data.active_provider ?? 'claude-code');
      setSelectedTab(data.active_provider ?? 'claude-code');
    } catch {
      setError('加载 Agent 配置失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  /** 重新加载 Agent 配置数据 */
  const reloadData = useCallback(async () => {
    try {
      const data = await getAgentSettings();
      setEditState(data.providers ?? {});
      setActiveProvider(data.active_provider ?? 'claude-code');
    } catch {
      // 静默失败
    }
  }, []);

  const handleSave = async () => {
    setSaving(true);
    try {
      await updateAgentSettings({
        active_provider: activeProvider,
        providers: editState,
      });
      setSaveResult('保存成功');
      setVerifyResult(null);
      await reloadData();
    } catch {
      setSaveResult('保存失败');
    } finally {
      setSaving(false);
      setTimeout(() => setSaveResult(null), 3000);
    }
  };

  const handleSetActive = async () => {
    setSaving(true);
    try {
      await updateAgentSettings({
        active_provider: selectedTab,
        providers: editState,
      });
      setActiveProvider(selectedTab);
      setSaveResult('已切换激活服务商');
      await reloadData();
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
      const result = await verifyAgentCredentials(selectedTab);
      setVerifyResult(result);
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
          <CardTitle>AI Agent 配置</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-48 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (error) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>AI Agent 配置</CardTitle>
        </CardHeader>
        <CardContent>
          <Alert variant="destructive">
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
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
        <CardTitle className="flex items-center gap-2 text-base">AI Agent 配置</CardTitle>
      </CardHeader>
      <CardContent>
        <Tabs value={selectedTab} onValueChange={setSelectedTab} className="w-full">
          {/* Tab 栏 */}
          <TabsList>
            {AGENT_PROVIDERS.map((p) => {
              const isActive = p.id === activeProvider;

              return (
                <TabsTrigger key={p.id} value={p.id}>
                  {p.name}
                  {isActive && (
                    <Badge variant="default" className="text-[10px] px-1.5 py-0 leading-4">
                      ✓
                    </Badge>
                  )}
                </TabsTrigger>
              );
            })}
          </TabsList>

          {/* 每个服务商的配置面板 */}
          {AGENT_PROVIDERS.map((p) => (
            <TabsContent key={p.id} value={p.id} className="space-y-4 mt-4">
              {/* 服务商名称和状态 */}
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <span className="font-medium text-foreground">{p.name}</span>
                {p.id === activeProvider ? (
                  <Badge variant="default" className="text-[10px] px-1.5">
                    当前激活
                  </Badge>
                ) : (
                  <Badge variant="outline" className="text-[10px] px-1.5">
                    未激活
                  </Badge>
                )}
              </div>

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
                {p.id !== activeProvider && (
                  <Button variant="secondary" onClick={handleSetActive} disabled={saving}>
                    {saving ? '保存中...' : '设为首选'}
                  </Button>
                )}
                {saveResult && (
                  <span
                    className={`text-sm ${
                      saveResult === '保存成功' || saveResult === '已切换激活服务商'
                        ? 'text-green-600'
                        : 'text-red-600'
                    }`}
                  >
                    {saveResult}
                  </span>
                )}
              </div>
            </TabsContent>
          ))}
        </Tabs>
      </CardContent>
    </Card>
  );
}

// ─── 页面 ───────────────────────────────────────────────────

export default function AgentSettings() {
  return (
    <div className="mx-auto max-w-2xl space-y-8 py-8 px-4">
      <div>
        <h1 className="text-2xl font-bold tracking-tight">Agent 配置</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          选择用于处理消息的 AI Agent。Claude Code 和 Codex 均使用本地安装的 CLI 工具，
          无需额外配置凭证。
        </p>
      </div>
      <AgentSettingsPanel />
    </div>
  );
}
