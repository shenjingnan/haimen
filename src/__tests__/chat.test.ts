import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../config/store.js', () => ({
  getApiKey: vi.fn().mockReturnValue('sk-configured-key'),
}));

vi.mock('@earendil-works/pi-ai', () => ({
  getProviders: () => ['openai'],
  getModels: () => [
    {
      id: 'gpt-4o-mini',
      name: 'GPT-4o Mini',
      api: 'openai-completions',
      provider: 'openai',
      baseUrl: 'https://api.openai.com/v1',
      reasoning: false,
      input: ['text'],
      cost: { input: 0.15, output: 0.6, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000,
      maxTokens: 16384,
    },
  ],
  stream: vi.fn(),
  complete: vi.fn(),
}));

const { chatRoutes } = await import('../server/routes/chat.js');

describe('chat route', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should return 400 when model is missing', async () => {
    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ messages: [{ role: 'user', content: 'Hi' }] }),
    });
    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.error.message).toContain('model');
  });

  it('should return 400 when messages is missing', async () => {
    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model: 'gpt-4o-mini' }),
    });
    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.error.message).toContain('messages');
  });

  it('should return 400 when messages is empty', async () => {
    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model: 'gpt-4o-mini', messages: [] }),
    });
    expect(res.status).toBe(400);
  });

  it('should return 404 when model is not found', async () => {
    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'nonexistent-model',
        messages: [{ role: 'user', content: 'Hi' }],
      }),
    });
    expect(res.status).toBe(404);
    const body = await res.json();
    expect(body.error.message).toContain('未知模型');
  });

  it('should return 502 when pi-ai complete throws', async () => {
    const mockComplete = vi.mocked((await import('@earendil-works/pi-ai')).complete);
    mockComplete.mockRejectedValue(new Error('API key invalid'));

    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'Hi' }],
        stream: false,
      }),
    });

    expect(res.status).toBe(502);
    const body = await res.json();
    expect(body.error.message).toContain('API key invalid');
  });

  it('should return streaming response when stream=true', async () => {
    const mockStream = vi.mocked((await import('@earendil-works/pi-ai')).stream);
    const events = [
      { type: 'text_delta', contentIndex: 0, delta: 'Hello', partial: {} },
      { type: 'text_delta', contentIndex: 0, delta: '!', partial: {} },
    ];
    mockStream.mockReturnValue({
      [Symbol.asyncIterator]: () => {
        let i = 0;
        return {
          next: () => {
            if (i < events.length) {
              return Promise.resolve({ value: events[i++], done: false });
            }
            return Promise.resolve({ done: true, value: undefined });
          },
        };
      },
      result: () =>
        Promise.resolve({
          content: [{ type: 'text', text: 'Hello!' }],
          usage: { input: 10, output: 5, totalTokens: 15 },
          stopReason: 'stop',
        }),
      // biome-ignore lint/suspicious/noExplicitAny: simplified mock for streaming test
    } as any);

    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'Hi' }],
        stream: true,
      }),
    });

    expect(res.status).toBe(200);
    const text = await res.text();
    expect(text).toContain('data: ');
    expect(text).toContain('[DONE]');
    expect(text).toContain('Hello');
  });

  it('should handle streaming error events', async () => {
    const mockStream = vi.mocked((await import('@earendil-works/pi-ai')).stream);
    const events = [{ type: 'error', reason: 'error', error: { content: [], usage: {} } }];
    mockStream.mockReturnValue({
      [Symbol.asyncIterator]: () => {
        let i = 0;
        return {
          next: () => {
            if (i < events.length) {
              return Promise.resolve({ value: events[i++], done: false });
            }
            return Promise.resolve({ done: true, value: undefined });
          },
        };
      },
      // biome-ignore lint/suspicious/noExplicitAny: simplified mock for streaming test
    } as any);

    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'Hi' }],
        stream: true,
      }),
    });

    expect(res.status).toBe(200);
    const text = await res.text();
    expect(text).toContain('[DONE]');
  });

  it('should handle streaming throw', async () => {
    const mockStream = vi.mocked((await import('@earendil-works/pi-ai')).stream);
    mockStream.mockReturnValue({
      [Symbol.asyncIterator]: () => ({
        next: () => Promise.reject(new Error('stream failed')),
      }),
      // biome-ignore lint/suspicious/noExplicitAny: simplified mock for streaming test
    } as any);

    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'Hi' }],
        stream: true,
      }),
    });

    expect(res.status).toBe(200);
    const text = await res.text();
    expect(text).toContain('[DONE]');
  });

  it('should return non-streaming response on successful call', async () => {
    const mockComplete = vi.mocked((await import('@earendil-works/pi-ai')).complete);
    mockComplete.mockResolvedValue({
      role: 'assistant',
      content: [{ type: 'text', text: 'Hello!' }],
      api: 'openai-completions',
      provider: 'openai',
      model: 'gpt-4o-mini',
      usage: {
        input: 10,
        output: 5,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 15,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
      stopReason: 'stop',
      timestamp: Date.now(),
      // biome-ignore lint/suspicious/noExplicitAny: mock value for test
    } as any);

    const res = await chatRoutes.request('/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'Hi' }],
        stream: false,
      }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.choices[0].message.content).toBe('Hello!');
    expect(body.choices[0].finish_reason).toBe('stop');
    expect(body.usage).toEqual({
      prompt_tokens: 10,
      completion_tokens: 5,
      total_tokens: 15,
    });
  });
});
