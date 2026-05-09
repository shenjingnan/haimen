import { stream as piStream } from '@earendil-works/pi-ai';
import { Hono } from 'hono';
import { stream as honoStream } from 'hono/streaming';
import { getApiKey } from '../../config/store.js';
import type { ChatCompletionRequest, OpenAIChatChunk, OpenAIChatCompletion } from '../../types.js';
import { buildContext, buildStreamOptions, findModelById, generateId } from './chat-utils.js';

export { buildContext, buildStreamOptions, findModelById, generateId };

export const chatRoutes = new Hono();

chatRoutes.post('/chat/completions', async (c) => {
  const body = (await c.req.json()) as ChatCompletionRequest;

  if (!body.model) {
    return c.json({ error: { message: 'model 是必填字段', type: 'invalid_request_error' } }, 400);
  }

  if (!body.messages || body.messages.length === 0) {
    return c.json(
      { error: { message: 'messages 是必填字段', type: 'invalid_request_error' } },
      400
    );
  }

  // 查找模型
  const model = findModelById(body.model);
  if (!model) {
    return c.json(
      {
        error: {
          message: `未知模型: ${body.model}`,
          type: 'invalid_request_error',
        },
      },
      404
    );
  }

  // 获取 API Key：优先请求体中的 api_key，其次配置存储
  const apiKey = body.api_key || getApiKey(model.provider);
  const context = buildContext(body);
  const options = buildStreamOptions(body, apiKey);

  const chatId = generateId();
  const created = Math.floor(Date.now() / 1000);

  // 流式响应
  if (body.stream !== false) {
    return honoStream(c, async (s) => {
      // 发送角色前缀
      const roleChunk: OpenAIChatChunk = {
        id: chatId,
        object: 'chat.completion.chunk',
        created,
        model: body.model,
        choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }],
      };
      await s.write(`data: ${JSON.stringify(roleChunk)}\n\n`);

      try {
        const piStreamInstance = piStream(model, context, options);

        for await (const event of piStreamInstance) {
          if (event.type === 'text_delta') {
            const chunk: OpenAIChatChunk = {
              id: chatId,
              object: 'chat.completion.chunk',
              created,
              model: body.model,
              choices: [{ index: 0, delta: { content: event.delta }, finish_reason: null }],
            };
            await s.write(`data: ${JSON.stringify(chunk)}\n\n`);
          } else if (event.type === 'error') {
            const errorChunk: OpenAIChatChunk = {
              id: chatId,
              object: 'chat.completion.chunk',
              created,
              model: body.model,
              choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
            };
            await s.write(`data: ${JSON.stringify(errorChunk)}\n\n`);
          }
        }
      } catch {
        const errorChunk: OpenAIChatChunk = {
          id: chatId,
          object: 'chat.completion.chunk',
          created,
          model: body.model,
          choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
        };
        await s.write(`data: ${JSON.stringify(errorChunk)}\n\n`);
      }

      await s.write('data: [DONE]\n\n');
    });
  }

  // 非流式响应
  try {
    const { complete } = await import('@earendil-works/pi-ai');
    const result = await complete(model, context, options);

    const content = result.content
      .filter((b) => b.type === 'text')
      .map((b) => b.text)
      .join('');

    const response: OpenAIChatCompletion = {
      id: chatId,
      object: 'chat.completion',
      created,
      model: body.model,
      choices: [
        {
          index: 0,
          message: { role: 'assistant', content },
          finish_reason: result.stopReason === 'toolUse' ? 'tool_calls' : result.stopReason,
        },
      ],
      usage: {
        prompt_tokens: result.usage.input,
        completion_tokens: result.usage.output,
        total_tokens: result.usage.totalTokens,
      },
    };

    return c.json(response);
  } catch (err) {
    const errorMsg = err instanceof Error ? err.message : String(err);
    return c.json(
      {
        error: {
          message: errorMsg,
          type: 'api_error',
        },
      },
      502
    );
  }
});
