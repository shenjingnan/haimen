import type { KnownProvider } from '@earendil-works/pi-ai';
import { getModels, getProviders } from '@earendil-works/pi-ai';
import { Hono } from 'hono';
import { getApiKeys } from '../../config/store.js';

export const modelsRoutes = new Hono();

modelsRoutes.get('/models', (c) => {
  const keys = getApiKeys();
  const configuredProviders = new Set(Object.keys(keys));
  const providers = getProviders();

  const models: Array<{
    id: string;
    name: string;
    provider: string;
    api: string;
    contextWindow: number;
    maxTokens: number;
    configured: boolean;
  }> = [];

  for (const provider of providers) {
    const isConfigured = configuredProviders.has(provider);
    for (const model of getModels(provider as KnownProvider)) {
      models.push({
        id: model.id,
        name: model.name,
        provider,
        api: model.api,
        contextWindow: model.contextWindow,
        maxTokens: model.maxTokens,
        configured: isConfigured,
      });
    }
  }

  return c.json({
    object: 'list',
    data: models,
  });
});
