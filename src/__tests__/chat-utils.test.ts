import type { Api, KnownProvider, Model } from '@earendil-works/pi-ai';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Mock pi-ai
const mockModels: Model<Api>[] = [
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
  {
    id: 'claude-sonnet-4-20250514',
    name: 'Claude Sonnet 4',
    api: 'anthropic-messages',
    provider: 'anthropic',
    baseUrl: 'https://api.anthropic.com/v1',
    reasoning: true,
    input: ['text', 'image'],
    cost: { input: 3, output: 15, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 200000,
    maxTokens: 8192,
  },
];

const mockGetProviders = vi.fn();
const mockGetModels = vi.fn();

vi.mock('@earendil-works/pi-ai', () => ({
  getProviders: () => mockGetProviders(),
  getModels: (provider: KnownProvider) => mockGetModels(provider),
}));

const { findModelById, buildContext, buildStreamOptions, generateId } = await import(
  '../server/routes/chat-utils.js'
);

describe('chat-utils', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe('generateId', () => {
    it('should generate an ID starting with chatcmpl-', () => {
      const id = generateId();
      expect(id).toMatch(/^chatcmpl-/);
    });

    it('should generate IDs of length 38 (chatcmpl- + 29 chars)', () => {
      const id = generateId();
      expect(id.length).toBe(38);
    });

    it('should generate different IDs on successive calls', () => {
      const id1 = generateId();
      const id2 = generateId();
      expect(id1).not.toBe(id2);
    });
  });

  describe('findModelById', () => {
    it('should find a model by its ID across providers', () => {
      mockGetProviders.mockReturnValue(['openai', 'anthropic']);
      mockGetModels.mockImplementation((provider: string) => {
        if (provider === 'openai') return [mockModels[0]];
        if (provider === 'anthropic') return [mockModels[1]];
        return [];
      });

      const model = findModelById('gpt-4o-mini');
      expect(model).toBeDefined();
      // biome-ignore lint/style/noNonNullAssertion: safe because we checked above
      expect(model!.id).toBe('gpt-4o-mini');
      // biome-ignore lint/style/noNonNullAssertion: safe because we checked above
      expect(model!.provider).toBe('openai');
    });

    it('should return undefined for unknown model', () => {
      mockGetProviders.mockReturnValue(['openai']);
      mockGetModels.mockReturnValue([mockModels[0]]);

      const model = findModelById('nonexistent-model');
      expect(model).toBeUndefined();
    });
  });

  describe('buildContext', () => {
    it('should convert system message to systemPrompt', () => {
      const ctx = buildContext({
        model: 'gpt-4o-mini',
        messages: [{ role: 'system', content: 'You are a helper' }],
      });
      expect(ctx.systemPrompt).toBe('You are a helper');
      expect(ctx.messages).toHaveLength(0);
    });

    it('should convert user messages to UserMessage', () => {
      const ctx = buildContext({
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'Hello' }],
      });
      expect(ctx.systemPrompt).toBeUndefined();
      expect(ctx.messages).toHaveLength(1);
      expect(ctx.messages[0]).toMatchObject({
        role: 'user',
        content: 'Hello',
      });
    });

    it('should convert assistant messages to AssistantMessage', () => {
      const ctx = buildContext({
        model: 'gpt-4o-mini',
        messages: [{ role: 'assistant', content: 'Hi there' }],
      });
      expect(ctx.messages).toHaveLength(1);
      expect(ctx.messages[0]).toMatchObject({
        role: 'assistant',
        content: [{ type: 'text', text: 'Hi there' }],
      });
    });

    it('should handle mixed messages correctly', () => {
      const ctx = buildContext({
        model: 'gpt-4o-mini',
        messages: [
          { role: 'system', content: 'Be concise' },
          { role: 'user', content: 'Question?' },
          { role: 'assistant', content: 'Answer.' },
        ],
      });
      expect(ctx.systemPrompt).toBe('Be concise');
      expect(ctx.messages).toHaveLength(2);
      expect(ctx.messages[0]).toMatchObject({ role: 'user', content: 'Question?' });
      expect(ctx.messages[1]).toMatchObject({
        role: 'assistant',
        content: [{ type: 'text', text: 'Answer.' }],
      });
    });

    it('should join multiple system messages with newline', () => {
      const ctx = buildContext({
        model: 'gpt-4o-mini',
        messages: [
          { role: 'system', content: 'Rule 1' },
          { role: 'user', content: 'Hi' },
          { role: 'system', content: 'Rule 2' },
        ],
      });
      expect(ctx.systemPrompt).toBe('Rule 1\nRule 2');
    });
  });

  describe('buildStreamOptions', () => {
    it('should include apiKey when provided', () => {
      const opts = buildStreamOptions(
        { model: 'test', messages: [{ role: 'user', content: 'Hi' }] },
        'sk-test'
      );
      expect(opts.apiKey).toBe('sk-test');
    });

    it('should not include apiKey when undefined', () => {
      const opts = buildStreamOptions(
        { model: 'test', messages: [{ role: 'user', content: 'Hi' }] },
        undefined
      );
      expect(opts.apiKey).toBeUndefined();
    });

    it('should include temperature and max_tokens when provided', () => {
      const opts = buildStreamOptions(
        {
          model: 'test',
          messages: [{ role: 'user', content: 'Hi' }],
          temperature: 0.7,
          max_tokens: 1000,
        },
        undefined
      );
      expect(opts.temperature).toBe(0.7);
      expect(opts.maxTokens).toBe(1000);
    });

    it('should omit temperature and max_tokens when not provided', () => {
      const opts = buildStreamOptions(
        { model: 'test', messages: [{ role: 'user', content: 'Hi' }] },
        undefined
      );
      expect(opts.temperature).toBeUndefined();
      expect(opts.maxTokens).toBeUndefined();
    });
  });
});
