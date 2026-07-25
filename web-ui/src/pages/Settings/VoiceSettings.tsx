import { Eye, EyeOff } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import {
  getAsrSettings,
  getTtsSettings,
  listTtsVoices,
  updateAsrSettings,
  updateTtsSettings,
  verifyAsrCredentials,
} from '@/api/voice';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Skeleton } from '@/components/ui/skeleton';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import VoiceSelector from '@/components/VoiceSelector';
import { ASR_PROVIDERS } from '@/data/asr-providers';
import type { TtsSettings, TtsVoice } from '@/types';

type AsyncState<T> =
  | { type: 'loading' }
  | { type: 'error'; message: string }
  | { type: 'loaded'; data: T };

function ApiStatusBadge({ success, label }: { success: boolean; label: string }) {
  return (
    <Badge variant={success ? 'default' : 'destructive'} className="ml-2">
      {success ? '已验证' : label}
    </Badge>
  );
}

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
      <span
        onClick={() => setShow(!show)}
        className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground cursor-pointer select-none"
        role="button"
        tabIndex={-1}
      >
        {show ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
      </span>
    </div>
  );
}

// ─── ASR 配置区域 ───────────────────────────────────────────

function AsrSettingsPanel() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editState, setEditState] = useState<Record<string, Record<string, string>>>({});
  const [activeProvider, setActiveProvider] = useState('doubao');
  const [selectedTab, setSelectedTab] = useState('doubao');
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
      const data = await getAsrSettings();
      setEditState(data.providers ?? {});
      setActiveProvider(data.active_provider ?? 'doubao');
      setSelectedTab(data.active_provider ?? 'doubao');
      setLoading(false);
    } catch {
      setError('加载 ASR 配置失败');
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleFieldChange = (key: string, value: string) => {
    setEditState((prev) => ({
      ...prev,
      [selectedTab]: {
        ...(prev[selectedTab] ?? {}),
        [key]: value,
      },
    }));
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await updateAsrSettings({
        active_provider: activeProvider,
        providers: editState,
      });
      setSaveResult('保存成功');
      setVerifyResult(null);
      await load();
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
      await updateAsrSettings({
        active_provider: selectedTab,
        providers: editState,
      });
      setActiveProvider(selectedTab);
      setSaveResult('已切换激活服务商');
      await load();
    } catch {
      setSaveResult('保存失败');
    } finally {
      setSaving(false);
      setTimeout(() => setSaveResult(null), 3000);
    }
  };

  const handleVerify = async () => {
    const creds = editState[selectedTab] ?? {};
    if (!Object.values(creds).some((v) => v.length > 0)) return;

    setVerifying(true);
    try {
      const result = await verifyAsrCredentials(
        creds,
        selectedTab,
      );
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
          <CardTitle>ASR 语音识别</CardTitle>
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
          <CardTitle>ASR 语音识别</CardTitle>
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
        <CardTitle className="flex items-center gap-2 text-base">
          ASR 语音识别
        </CardTitle>
      </CardHeader>
      <CardContent>
        <Tabs value={selectedTab} onValueChange={setSelectedTab} className="w-full">
          {/* Tab 栏 */}
          <TabsList>
            {ASR_PROVIDERS.map((p) => {
              const isActive = p.id === activeProvider;

              return (
                <TabsTrigger key={p.id} value={p.id}>
                  {p.name}
                  {isActive && (
                    <Badge variant="default" className="text-[10px] px-1.5 py-0 leading-4">✓</Badge>
                  )}
                </TabsTrigger>
              );
            })}
          </TabsList>

          {/* 每个服务商的配置面板 */}
          {ASR_PROVIDERS.map((p) => (
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

              {/* 动态字段 */}
              {p.fields.map((field) => (
                <div key={field.key} className="space-y-2">
                  <span className="text-sm font-medium">{field.label}</span>
                  <PasswordInput
                    id={`asr-${p.id}-${field.key}`}
                    placeholder={field.placeholder ?? `输入${field.label}`}
                    value={editState[p.id]?.[field.key] ?? ''}
                    onChange={(v) => handleFieldChange(field.key, v)}
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
                <Button
                  variant="outline"
                  onClick={handleVerify}
                  disabled={verifying || !p.fields.some((f) => editState[p.id]?.[f.key])}
                >
                  {verifying ? '验证中...' : '验证凭证'}
                </Button>
                <Button onClick={handleSave} disabled={saving}>
                  {saving ? '保存中...' : '保存配置'}
                </Button>
                {p.id !== activeProvider && (
                  <Button variant="secondary" onClick={handleSetActive} disabled={saving}>
                    {saving ? '保存中...' : '🌟 设为首选'}
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

// ─── TTS 配置区域 ───────────────────────────────────────────

function TtsSettingsPanel() {
  const [state, setState] = useState<AsyncState<TtsSettings>>({ type: 'loading' });
  const [voices, setVoices] = useState<TtsVoice[]>([]);
  const [voice, setVoice] = useState('');
  const [appKey, setAppKey] = useState('');
  const [accessToken, setAccessToken] = useState('');
  const [verifyResult, setVerifyResult] = useState<{ valid: boolean; message: string } | null>(
    null,
  );
  const [verifying, setVerifying] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveResult, setSaveResult] = useState<string | null>(null);

  const load = useCallback(async () => {
    setState({ type: 'loading' });
    try {
      const [data, voiceList] = await Promise.all([getTtsSettings(), listTtsVoices()]);
      setVoice(data.voice ?? '');
      setAppKey(data.app_key ?? '');
      setAccessToken(data.access_token ?? '');
      setVoices(voiceList);
      setState({ type: 'loaded', data });
    } catch {
      setState({ type: 'error', message: '加载 TTS 配置失败' });
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleSave = async () => {
    setSaving(true);
    try {
      await updateTtsSettings({
        voice: voice || null,
        app_key: appKey,
        access_token: accessToken,
      });
      setSaveResult('保存成功');
      load();
    } catch {
      setSaveResult('保存失败');
    } finally {
      setSaving(false);
      setTimeout(() => setSaveResult(null), 3000);
    }
  };

  const handleVerify = async () => {
    if (!appKey || !accessToken) return;
    setVerifying(true);
    try {
      const result = await verifyAsrCredentials({ app_key: appKey, access_key: accessToken }, 'doubao');
      setVerifyResult(result);
    } catch {
      setVerifyResult({ valid: false, message: '网络请求失败' });
    } finally {
      setVerifying(false);
    }
  };

  if (state.type === 'loading') {
    return (
      <Card>
        <CardHeader>
          <CardTitle>TTS 语音合成</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-64 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (state.type === 'error') {
    return (
      <Card>
        <CardHeader>
          <CardTitle>TTS 语音合成</CardTitle>
        </CardHeader>
        <CardContent>
          <Alert variant="destructive">
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{state.message}</AlertDescription>
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
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          TTS 语音合成
          <Badge variant="secondary" className="text-xs">
            {state.data.provider}
          </Badge>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-2">
          <span className="text-sm font-medium">App Key</span>
          <PasswordInput
            id="tts-app-key"
            placeholder={state.data.app_key ?? '未设置，使用环境变量'}
            value={appKey}
            onChange={setAppKey}
          />
        </div>

        <div className="space-y-2">
          <span className="text-sm font-medium">Access Token</span>
          <PasswordInput
            id="tts-access-token"
            placeholder={state.data.access_token ?? '未设置，使用环境变量'}
            value={accessToken}
            onChange={setAccessToken}
          />
        </div>

        <div className="space-y-2">
          <span className="text-sm font-medium" id="tts-voice-label">
            TTS 音色
          </span>
          <div>
            <VoiceSelector voices={voices} selectedVoice={voice || null} onChange={setVoice} />
          </div>
        </div>

        {verifyResult && (
          <Alert variant={verifyResult.valid ? 'default' : 'destructive'}>
            <AlertTitle>{verifyResult.valid ? '验证通过' : '验证失败'}</AlertTitle>
            <AlertDescription>{verifyResult.message}</AlertDescription>
          </Alert>
        )}

        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            onClick={handleVerify}
            disabled={verifying || !appKey || !accessToken}
          >
            {verifying ? '验证中...' : '验证凭证'}
          </Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving ? '保存中...' : '保存 TTS'}
          </Button>
          {saveResult && (
            <span
              className={`text-sm ${saveResult === '保存成功' ? 'text-green-600' : 'text-red-600'}`}
            >
              {saveResult}
            </span>
          )}
          <ApiStatusBadge success={!!state.data.voice} label="使用默认音色" />
        </div>
      </CardContent>
    </Card>
  );
}

// ─── 页面 ───────────────────────────────────────────────────

export default function VoiceSettings() {
  return (
    <div className="mx-auto max-w-2xl space-y-8 py-8 px-4">
      <div>
        <h1 className="text-2xl font-bold tracking-tight">语音配置</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          配置语音识别 (ASR) 和语音合成 (TTS) 的提供商和参数。 清空字段将回退到环境变量。
        </p>
      </div>
      <AsrSettingsPanel />
      <TtsSettingsPanel />
    </div>
  );
}
