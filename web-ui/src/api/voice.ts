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
  app_key?: string;
  access_token?: string;
}): Promise<AsrSettings> {
  const res = await apiFetch<ApiResponse<AsrSettings>>('/api/v1/settings/asr', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
  return ensureData(res);
}

export async function verifyAsrCredentials(
  appKey: string,
  accessToken: string,
): Promise<{ valid: boolean; message: string }> {
  const res = await apiFetch<ApiResponse<{ valid: boolean; message: string }>>(
    '/api/v1/settings/asr/verify',
    {
      method: 'POST',
      body: JSON.stringify({ app_key: appKey, access_token: accessToken }),
    },
  );
  return ensureData(res);
}

// ── TTS ──

export async function getTtsSettings(): Promise<TtsSettings> {
  const res = await apiFetch<ApiResponse<TtsSettings>>('/api/v1/settings/tts');
  return ensureData(res);
}

export async function updateTtsSettings(settings: Partial<TtsSettings>): Promise<TtsSettings> {
  const res = await apiFetch<ApiResponse<TtsSettings>>('/api/v1/settings/tts', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
  return ensureData(res);
}

export async function listTtsVoices(): Promise<TtsVoice[]> {
  const res = await apiFetch<ApiResponse<{ provider: string; voices: TtsVoice[] }>>(
    '/api/v1/settings/tts/voices',
  );
  return ensureData(res).voices;
}
