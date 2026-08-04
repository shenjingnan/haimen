import { useState } from 'react';
import AgentLogs from '@/pages/AgentLogs';
import AgentSettings from '@/pages/Settings/AgentSettings';
import ConnectorSettings from '@/pages/Settings/ConnectorSettings';
import VoiceSettings from '@/pages/Settings/VoiceSettings';

type Page = 'voice' | 'agent' | 'connectors' | 'logs';

export default function App() {
  const [page, setPage] = useState<Page>('voice');

  return (
    <div className="min-h-screen bg-background">
      <nav className="border-b px-4 py-2">
        <div className="mx-auto max-w-4xl flex items-center gap-4">
          <h1 className="text-lg font-bold">Haimen</h1>
          <button
            type="button"
            onClick={() => setPage('voice')}
            className={`px-3 py-1 rounded text-sm cursor-pointer ${
              page === 'voice' ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'
            }`}
          >
            语音配置
          </button>
          <button
            type="button"
            onClick={() => setPage('agent')}
            className={`px-3 py-1 rounded text-sm cursor-pointer ${
              page === 'agent' ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'
            }`}
          >
            Agent 配置
          </button>
          <button
            type="button"
            onClick={() => setPage('connectors')}
            className={`px-3 py-1 rounded text-sm cursor-pointer ${
              page === 'connectors' ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'
            }`}
          >
            消息渠道
          </button>
          <button
            type="button"
            onClick={() => setPage('logs')}
            className={`px-3 py-1 rounded text-sm cursor-pointer ${
              page === 'logs' ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'
            }`}
          >
            对话记录
          </button>
        </div>
      </nav>
      {page === 'voice' && <VoiceSettings />}
      {page === 'agent' && <AgentSettings />}
      {page === 'connectors' && <ConnectorSettings />}
      {page === 'logs' && <AgentLogs />}
    </div>
  );
}
