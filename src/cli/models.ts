import type { KnownProvider } from '@earendil-works/pi-ai';
import { getModels, getProviders } from '@earendil-works/pi-ai';
import { getApiKeys } from '../config/store.js';

export function listModels(): void {
  const keys = getApiKeys();
  const configuredProviders = new Set(Object.keys(keys));
  const providers = getProviders();

  console.log('可用模型:');
  let hasModels = false;

  for (const provider of providers) {
    const hasKey = configuredProviders.has(provider);
    const label = hasKey ? '🔑' : '  ';
    const models = getModels(provider as KnownProvider);

    if (models.length === 0) continue;

    console.log(`\n${label} ${provider}${hasKey ? '' : ' (未配置 API Key)'}`);
    hasModels = true;

    for (const model of models) {
      const costStr = model.cost
        ? `$${model.cost.input.toFixed(2)}i/$${model.cost.output.toFixed(2)}o/M`
        : '';
      console.log(`  - ${model.id}${costStr ? ` (${costStr})` : ''}`);
    }
  }

  if (!hasModels) {
    console.log('  (无可用模型)');
  }
}
