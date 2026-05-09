import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { HaimenConfig } from '../types.js';

const CONFIG_DIR = join(homedir(), '.haimen');
const CONFIG_PATH = join(CONFIG_DIR, 'config.json');

const DEFAULT_CONFIG: HaimenConfig = {
  apiKeys: {},
  port: 6379,
  host: '127.0.0.1',
};

function ensureConfigDir(): void {
  if (!existsSync(CONFIG_DIR)) {
    mkdirSync(CONFIG_DIR, { recursive: true });
  }
}

export function loadConfig(): HaimenConfig {
  try {
    if (existsSync(CONFIG_PATH)) {
      const data = readFileSync(CONFIG_PATH, 'utf-8');
      return { ...DEFAULT_CONFIG, ...JSON.parse(data) };
    }
  } catch {
    // ignore parse errors, return default
  }
  return { ...DEFAULT_CONFIG };
}

export function saveConfig(config: HaimenConfig): void {
  ensureConfigDir();
  writeFileSync(CONFIG_PATH, JSON.stringify(config, null, 2), 'utf-8');
}

export function getApiKey(provider: string): string | undefined {
  const config = loadConfig();
  return config.apiKeys[provider];
}

export function setApiKey(provider: string, apiKey: string): void {
  const config = loadConfig();
  config.apiKeys[provider] = apiKey;
  saveConfig(config);
}

export function removeApiKey(provider: string): boolean {
  const config = loadConfig();
  if (provider in config.apiKeys) {
    delete config.apiKeys[provider];
    saveConfig(config);
    return true;
  }
  return false;
}

export function getApiKeys(): Record<string, string> {
  const config = loadConfig();
  return { ...config.apiKeys };
}

export function getConfig(): HaimenConfig {
  return loadConfig();
}
