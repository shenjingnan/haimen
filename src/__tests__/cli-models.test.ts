import type { Mock } from 'vitest';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mockGetApiKeys: Mock = vi.fn();
const mockGetProviders: Mock = vi.fn();
const mockGetModels: Mock = vi.fn();

vi.mock('../config/store.js', () => ({
  getApiKeys: () => mockGetApiKeys(),
}));

vi.mock('@earendil-works/pi-ai', () => ({
  getProviders: () => mockGetProviders(),
  getModels: (provider: string) => mockGetModels(provider),
}));

const { listModels } = await import('../cli/models.js');

describe('cli/models', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should print model list when keys are configured', () => {
    const logs: string[] = [];
    vi.spyOn(console, 'log').mockImplementation((msg) => {
      logs.push(String(msg));
    });

    mockGetProviders.mockReturnValue(['openai']);
    mockGetModels.mockReturnValue([
      {
        id: 'gpt-4o-mini',
        name: 'GPT-4o Mini',
        provider: 'openai',
        cost: { input: 0.15, output: 0.6, cacheRead: 0, cacheWrite: 0 },
      },
    ]);
    mockGetApiKeys.mockReturnValue({ openai: 'sk-test' });

    listModels();

    expect(logs.some((l) => l.includes('gpt-4o-mini'))).toBe(true);
    expect(logs.some((l) => l.includes('$0.15i/$0.60o/M'))).toBe(true);
  });

  it('should handle empty providers without error', () => {
    mockGetProviders.mockReturnValue([]);
    mockGetModels.mockReturnValue([]);
    mockGetApiKeys.mockReturnValue({});

    expect(() => listModels()).not.toThrow();
  });

  it('should print no models message when all providers have no models', () => {
    const logs: string[] = [];
    vi.spyOn(console, 'log').mockImplementation((msg) => {
      logs.push(String(msg));
    });

    mockGetProviders.mockReturnValue(['openai']);
    mockGetModels.mockReturnValue([]);
    mockGetApiKeys.mockReturnValue({});

    listModels();

    expect(logs.some((l) => l.includes('无可用模型'))).toBe(true);
  });

  it('should handle model without cost field', () => {
    const logs: string[] = [];
    vi.spyOn(console, 'log').mockImplementation((msg) => {
      logs.push(String(msg));
    });

    mockGetProviders.mockReturnValue(['custom']);
    mockGetModels.mockReturnValue([
      {
        id: 'custom-model',
        name: 'Custom Model',
        provider: 'custom',
        // no cost field
      },
    ]);
    mockGetApiKeys.mockReturnValue({});

    listModels();

    expect(logs.some((l) => l.includes('custom-model'))).toBe(true);
  });
});
