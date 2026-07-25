import { useCallback, useEffect, useState } from 'react';
import {
  getAsrSettings,
  getTtsSettings,
  listTtsVoices,
  updateAsrSettings,
  updateTtsSettings,
  verifyAsrCredentials,
} from '@/api/voice';
import { Eye, EyeOff } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Skeleton } from '@/components/ui/skeleton';
import VoiceSelector from '@/components/VoiceSelector';
import type { AsrSettings, TtsSettings, TtsVoice } from '@/types';

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
  const [state, setState] = useState<AsyncState<AsrSettings>>({ type: 'loading' });
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
      const data = await getAsrSettings();
      setAppKey(data.app_key ?? '');
      setAccessToken(data.access_token ?? '');
      setState({ type: 'loaded', data });
    } catch {
      setState({ type: 'error', message: '加载 ASR 配置失败' });
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleVerify = async () => {
    if (!appKey || !accessToken) return;
    setVerifying(true);
    try {
      const result = await verifyAsrCredentials(appKey, accessToken);
      setVerifyResult(result);
    } catch {
      setVerifyResult({ valid: false, message: '网络请求失败' });
    } finally {
      setVerifying(false);
    }
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await updateAsrSettings({
        app_key: appKey,
        access_token: accessToken,
      });
      setSaveResult('保存成功');
      setVerifyResult(null);
      load();
    } catch {
      setSaveResult('保存失败');
    } finally {
      setSaving(false);
      setTimeout(() => setSaveResult(null), 3000);
    }
  };

  if (state.type === 'loading') {
    return (
      <Card>
        <CardHeader>
          <CardTitle>ASR 语音识别</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-32 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (state.type === 'error') {
    return (
      <Card>
        <CardHeader>
          <CardTitle>ASR 语音识别</CardTitle>
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
          ASR 语音识别
          <Badge variant="secondary" className="text-xs">
            {state.data.provider}
          </Badge>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-2">
          <span className="text-sm font-medium">App Key</span>
          <PasswordInput
            id="asr-app-key"
            placeholder={state.data.app_key ?? '未设置，使用环境变量 DOUBAO_APP_KEY'}
            value={appKey}
            onChange={setAppKey}
          />
        </div>

        <div className="space-y-2">
          <span className="text-sm font-medium">Access Token</span>
          <PasswordInput
            id="asr-access-token"
            placeholder={state.data.access_token ?? '未设置，使用环境变量 DOUBAO_ACCESS_TOKEN'}
            value={accessToken}
            onChange={setAccessToken}
          />
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
            {saving ? '保存中...' : '保存 ASR'}
          </Button>
          {saveResult && (
            <span
              className={`text-sm ${saveResult === '保存成功' ? 'text-green-600' : 'text-red-600'}`}
            >
              {saveResult}
            </span>
          )}
          <ApiStatusBadge success={!!state.data.app_key} label="未配置" />
        </div>
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
  const [verifyResult, setVerifyResult] = useState<{ valid: boolean; message: string } | null>(null);
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
      const result = await verifyAsrCredentials(appKey, accessToken);
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
