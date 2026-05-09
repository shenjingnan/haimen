import type {
  Api,
  Context,
  KnownProvider,
  Message,
  Model,
  ProviderStreamOptions,
  UserMessage,
} from '@earendil-works/pi-ai';
import { getModels, getProviders } from '@earendil-works/pi-ai';
import type { ChatCompletionRequest } from '../../types.js';

export function generateId(): string {
  const chars = 'abcdefghijklmnopqrstuvwxyz0123456789';
  let result = 'chatcmpl-';
  for (let i = 0; i < 29; i++) {
    result += chars.charAt(Math.floor(Math.random() * chars.length));
  }
  return result;
}

/**
 * 在所有 provider 中搜索指定模型 ID
 */
export function findModelById(modelId: string): Model<Api> | undefined {
  for (const provider of getProviders()) {
    const models = getModels(provider as KnownProvider);
    const found = models.find((m) => m.id === modelId);
    if (found) {
      return found as unknown as Model<Api>;
    }
  }
  return undefined;
}

/**
 * 将 OpenAI 格式的消息转换为 pi-ai Context
 */
export function buildContext(req: ChatCompletionRequest): Context {
  const systemPrompt: string[] = [];
  const messages: Message[] = [];

  for (const msg of req.messages) {
    if (msg.role === 'system') {
      systemPrompt.push(msg.content);
    } else if (msg.role === 'user') {
      const userMsg: UserMessage = {
        role: 'user',
        content: msg.content,
        timestamp: Date.now(),
      };
      messages.push(userMsg);
    } else if (msg.role === 'assistant') {
      messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: msg.content }],
        api: '' as Api,
        provider: '',
        model: req.model,
        usage: {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason: 'stop',
        timestamp: Date.now(),
      } as Message);
    }
  }

  const systemPromptStr = systemPrompt.join('\n');
  return {
    ...(systemPromptStr ? { systemPrompt: systemPromptStr } : {}),
    messages,
  } as Context;
}

/**
 * 构建 stream options，处理 exactOptionalPropertyTypes
 */
export function buildStreamOptions(
  body: ChatCompletionRequest,
  apiKey: string | undefined
): ProviderStreamOptions {
  const options: ProviderStreamOptions = {};
  if (apiKey) {
    options.apiKey = apiKey;
  }
  if (body.temperature !== undefined) {
    options.temperature = body.temperature;
  }
  if (body.max_tokens !== undefined) {
    options.maxTokens = body.max_tokens;
  }
  return options;
}
