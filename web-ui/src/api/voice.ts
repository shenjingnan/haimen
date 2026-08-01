import type { ApiResponse, AsrSettings, TtsSettings, TtsVoice } from '@/types';
import { apiFetch } from './client';

function ensureData<T>(res: ApiResponse<T>): T {
  if (res.data == null) throw new Error('Empty response');
  return res.data;
}

// ── ASR ──

export async function getAsrSettings(): Promise<AsrSettings> {
  const res = await apiFetch<ApiResponse<AsrSettings>>('/api/v1/settings/asr');
  return ensureData(res);
}

export async function updateAsrSettings(settings: {
  active_provider?: string;
  providers?: Record<string, Record<string, string>>;
}): Promise<AsrSettings> {
  const res = await apiFetch<ApiResponse<AsrSettings>>('/api/v1/settings/asr', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
  return ensureData(res);
}

export async function verifyAsrCredentials(
  creds: Record<string, string>,
  provider: string,
): Promise<{ valid: boolean; message: string }> {
  const body: Record<string, string> = { ...creds, provider };
  const res = await apiFetch<ApiResponse<{ valid: boolean; message: string }>>(
    '/api/v1/settings/asr/verify',
    {
      method: 'POST',
      body: JSON.stringify(body),
    },
  );
  return ensureData(res);
}

// ── TTS ──

export async function getTtsSettings(): Promise<TtsSettings> {
  const res = await apiFetch<ApiResponse<TtsSettings>>('/api/v1/settings/tts');
  return ensureData(res);
}

export async function updateTtsSettings(settings: {
  active_provider?: string;
  providers?: Record<string, Record<string, string>>;
  fixed_text_enabled?: boolean;
  fixed_text?: string | null;
}): Promise<TtsSettings> {
  const res = await apiFetch<ApiResponse<TtsSettings>>('/api/v1/settings/tts', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
  return ensureData(res);
}

export async function listTtsVoices(provider?: string, model?: string): Promise<TtsVoice[]> {
  const params = new URLSearchParams();
  if (provider) params.set('provider', provider);
  if (model) params.set('model', model);
  const qs = params.toString();
  const res = await apiFetch<ApiResponse<{ provider: string; voices: TtsVoice[] }>>(
    `/api/v1/settings/tts/voices${qs ? `?${qs}` : ''}`,
  );
  return ensureData(res).voices;
}

export async function verifyTtsCredentials(
  creds: Record<string, string>,
  provider: string,
): Promise<{ valid: boolean; message: string }> {
  const body: Record<string, string> = { ...creds, provider };
  const res = await apiFetch<ApiResponse<{ valid: boolean; message: string }>>(
    '/api/v1/settings/tts/verify',
    {
      method: 'POST',
      body: JSON.stringify(body),
    },
  );
  return ensureData(res);
}
