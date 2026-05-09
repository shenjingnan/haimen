#!/usr/bin/env node
import { getApiKeys, getConfig, removeApiKey, setApiKey } from '../config/store.js';

const version = '0.1.0';

function showHelp() {
  console.log(`
haimen - AI 模型网关

用法:
  haimen serve [--port <port>] [--host <host>]    启动服务
  haimen keys add <provider> <apiKey>              添加 API Key
  haimen keys list                                  列出 API Key
  haimen keys remove <provider>                     删除 API Key
  haimen models                                     列出可用模型
  haimen --help                                     显示帮助
  haimen --version                                  显示版本

示例:
  haimen keys add openai sk-xxx
  haimen keys add anthropic sk-ant-xxx
  haimen keys add deepseek sk-xxx
  haimen serve
  haimen serve --port 6379 --host 0.0.0.0
  haimen models
`);
}

async function main() {
  const args = process.argv.slice(2);

  if (args.length === 0) {
    showHelp();
    process.exit(0);
  }

  const command = args[0];

  switch (command) {
    case 'serve':
    case 'server': {
      const { startServer } = await import('../server/index.js');
      const config = getConfig();
      let port = config.port;
      let host = config.host;

      for (let i = 1; i < args.length; i++) {
        if (args[i] === '--port' || args[i] === '-p') {
          const val = args[i + 1];
          if (val) port = Number(val);
          i++;
        } else if (args[i] === '--host' || args[i] === '-h') {
          const val = args[i + 1];
          if (val) host = val;
          i++;
        }
      }

      await startServer(port, host);
      break;
    }

    case 'keys': {
      const sub = args[1];

      if (sub === 'add') {
        const provider = args[2];
        const apiKey = args[3];
        if (!provider || !apiKey) {
          console.error('用法: haimen keys add <provider> <apiKey>');
          process.exit(1);
        }
        setApiKey(provider, apiKey);
        console.log(`✅ 已添加 ${provider} 的 API Key`);
      } else if (sub === 'list') {
        const keys = getApiKeys();
        const entries = Object.entries(keys);
        if (entries.length === 0) {
          console.log('未配置任何 API Key');
        } else {
          console.log('已配置的 API Key:');
          for (const [provider, key] of entries) {
            const masked = key.length > 8 ? `${key.slice(0, 4)}...${key.slice(-4)}` : '***';
            console.log(`  ${provider}: ${masked}`);
          }
        }
      } else if (sub === 'remove') {
        const provider = args[2];
        if (!provider) {
          console.error('用法: haimen keys remove <provider>');
          process.exit(1);
        }
        if (removeApiKey(provider)) {
          console.log(`✅ 已删除 ${provider} 的 API Key`);
        } else {
          console.error(`❌ 未找到 ${provider} 的 API Key`);
          process.exit(1);
        }
      } else {
        console.error('用法: haimen keys add|list|remove');
        process.exit(1);
      }
      break;
    }

    case 'models': {
      const { listModels } = await import('./models.js');
      listModels();
      break;
    }

    case '--version':
    case '-v':
      console.log(`haimen v${version}`);
      break;

    case '--help':
    case '-h':
      showHelp();
      break;

    default:
      console.error(`未知命令: ${command}`);
      showHelp();
      process.exit(1);
  }
}

main().catch((err) => {
  console.error('错误:', err instanceof Error ? err.message : err);
  process.exit(1);
});
