import { beforeEach, describe, expect, it, vi } from 'vitest';

const mockGetApiKeys = vi.fn();
vi.mock('../config/store.js', () => ({
  getApiKeys: () => mockGetApiKeys(),
}));

vi.mock('@earendil-works/pi-ai', () => ({
  getProviders: () => ['openai', 'anthropic'],
  getModels: (provider: string) => {
    if (provider === 'openai') {
      return [
        {
          id: 'gpt-4o-mini',
          name: 'GPT-4o Mini',
          api: 'openai-completions',
          provider: 'openai',
          baseUrl: 'https://api.openai.com/v1',
          contextWindow: 128000,
          maxTokens: 16384,
          cost: { input: 0.15, output: 0.6, cacheRead: 0, cacheWrite: 0 },
          reasoning: false,
          input: ['text'],
        },
      ];
    }
    if (provider === 'anthropic') {
      return [
        {
          id: 'claude-sonnet-4-20250514',
          name: 'Claude Sonnet 4',
          api: 'anthropic-messages',
          provider: 'anthropic',
          baseUrl: 'https://api.anthropic.com/v1',
          contextWindow: 200000,
          maxTokens: 8192,
          cost: { input: 3, output: 15, cacheRead: 0, cacheWrite: 0 },
          reasoning: true,
          input: ['text', 'image'],
        },
      ];
    }
    return [];
  },
}));

const { modelsRoutes } = await import('../server/routes/models.js');

describe('models route', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should return model list with configured flag when keys exist', async () => {
    mockGetApiKeys.mockReturnValue({ openai: 'sk-test' });
    const res = await modelsRoutes.request('/models');
    expect(res.status).toBe(200);

    const body = await res.json();
    expect(body.object).toBe('list');
    expect(body.data).toHaveLength(2);

    // biome-ignore lint/suspicious/noExplicitAny: dynamic response data
    const openaiModel = body.data.find((m: any) => m.provider === 'openai');
    expect(openaiModel.configured).toBe(true);

    // biome-ignore lint/suspicious/noExplicitAny: dynamic response data
    const anthropicModel = body.data.find((m: any) => m.provider === 'anthropic');
    expect(anthropicModel.configured).toBe(false);
  });

  it('should mark all models as not configured when no keys exist', async () => {
    mockGetApiKeys.mockReturnValue({});
    const res = await modelsRoutes.request('/models');
    const body = await res.json();

    for (const model of body.data) {
      expect(model.configured).toBe(false);
    }
  });

  it('should include correct model metadata', async () => {
    mockGetApiKeys.mockReturnValue({});
    const res = await modelsRoutes.request('/models');
    const body = await res.json();

    // biome-ignore lint/suspicious/noExplicitAny: dynamic response data
    const gpt = body.data.find((m: any) => m.id === 'gpt-4o-mini');
    expect(gpt).toMatchObject({
      id: 'gpt-4o-mini',
      provider: 'openai',
      api: 'openai-completions',
      contextWindow: 128000,
      maxTokens: 16384,
    });
  });
});
