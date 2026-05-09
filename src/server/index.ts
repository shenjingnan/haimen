import { serve } from '@hono/node-server';
import { Hono } from 'hono';
import { cors } from 'hono/cors';
import { getConfig } from '../config/store.js';
import { chatRoutes } from './routes/chat.js';
import { modelsRoutes } from './routes/models.js';

export async function startServer(port?: number, host?: string): Promise<void> {
  const config = getConfig();
  const listenPort = port ?? config.port;
  const listenHost = host ?? config.host;

  const app = new Hono();

  // CORS 允许所有来源（Agent 可能来自 localhost 或容器）
  app.use('/*', cors());

  // 注册路由
  app.route('/v1', chatRoutes);
  app.route('/v1', modelsRoutes);

  // 根路径
  app.get('/', (c) =>
    c.json({
      name: 'haimen',
      version: '0.1.0',
      description: 'AI 模型网关 - 基于 pi-ai 的多提供商代理',
      endpoints: {
        'GET /v1/models': '列出可用模型',
        'POST /v1/chat/completions': 'OpenAI 兼容的聊天补全',
      },
    })
  );

  console.log(`🚀 haimen server 启动于 http://${listenHost}:${listenPort}`);
  console.log(`   模型列表: http://${listenHost}:${listenPort}/v1/models`);
  console.log(`   聊天接口: POST http://${listenHost}:${listenPort}/v1/chat/completions`);

  serve(
    {
      fetch: app.fetch,
      port: listenPort,
      hostname: listenHost,
    },
    (info) => {
      console.log(`   监听地址: ${info.address}:${info.port}`);
    }
  );
}
