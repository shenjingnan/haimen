import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../config/store.js', () => ({
  getConfig: vi.fn().mockReturnValue({
    apiKeys: {},
    port: 6379,
    host: '127.0.0.1',
  }),
}));

// Mock @hono/node-server serve to avoid actually starting a server
vi.mock('@hono/node-server', () => ({
  serve: vi.fn(),
}));

const { startServer } = await import('../server/index.js');

describe('server', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should start server without error', async () => {
    // This test verifies that startServer doesn't throw
    // The actual server creation is mocked
    await expect(startServer(0, '127.0.0.1')).resolves.toBeUndefined();
  });
});
