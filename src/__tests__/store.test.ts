import { beforeEach, describe, expect, it, vi } from 'vitest';

const mockConfigDir = '/mock-home/.haimen';
const mockConfigPath = '/mock-home/.haimen/config.json';

const mockFs = {
  existsSync: vi.fn(),
  readFileSync: vi.fn(),
  writeFileSync: vi.fn(),
  mkdirSync: vi.fn(),
};

vi.mock('node:os', () => ({
  homedir: () => '/mock-home',
}));

vi.mock('node:fs', () => mockFs);

// 在 mock 之后导入被测试模块
const { loadConfig, saveConfig, getApiKey, setApiKey, removeApiKey, getApiKeys, getConfig } =
  await import('../config/store.js');

describe('config/store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe('loadConfig', () => {
    it('should return default config when config file does not exist', () => {
      mockFs.existsSync.mockReturnValue(false);
      const config = loadConfig();
      expect(config).toEqual({
        apiKeys: {},
        port: 6379,
        host: '127.0.0.1',
      });
      expect(mockFs.existsSync).toHaveBeenCalledWith(mockConfigPath);
    });

    it('should merge with defaults when config file exists', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(
        JSON.stringify({
          apiKeys: { openai: 'sk-test' },
          port: 9999,
        })
      );
      const config = loadConfig();
      expect(config).toEqual({
        apiKeys: { openai: 'sk-test' },
        port: 9999,
        host: '127.0.0.1',
      });
    });

    it('should return defaults when JSON is invalid', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue('invalid json');
      const config = loadConfig();
      expect(config).toEqual({
        apiKeys: {},
        port: 6379,
        host: '127.0.0.1',
      });
    });
  });

  describe('saveConfig', () => {
    it('should create directory and write config', () => {
      mockFs.existsSync.mockReturnValue(false);
      saveConfig({
        apiKeys: { test: 'key' },
        port: 6379,
        host: '127.0.0.1',
      });
      expect(mockFs.mkdirSync).toHaveBeenCalledWith(mockConfigDir, { recursive: true });
      expect(mockFs.writeFileSync).toHaveBeenCalledWith(
        mockConfigPath,
        JSON.stringify({ apiKeys: { test: 'key' }, port: 6379, host: '127.0.0.1' }, null, 2),
        'utf-8'
      );
    });

    it('should not create directory if it already exists', () => {
      mockFs.existsSync.mockReturnValue(true);
      saveConfig({
        apiKeys: {},
        port: 6379,
        host: '127.0.0.1',
      });
      expect(mockFs.mkdirSync).not.toHaveBeenCalled();
    });
  });

  describe('getApiKey', () => {
    it('should return the key for an existing provider', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(
        JSON.stringify({
          apiKeys: { anthropic: 'sk-ant-test' },
        })
      );
      expect(getApiKey('anthropic')).toBe('sk-ant-test');
    });

    it('should return undefined for a non-existing provider', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(JSON.stringify({ apiKeys: {} }));
      expect(getApiKey('nonexistent')).toBeUndefined();
    });
  });

  describe('setApiKey', () => {
    it('should save a new API key to config', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(JSON.stringify({ apiKeys: {} }));

      setApiKey('deepseek', 'sk-ds-test');

      expect(mockFs.writeFileSync).toHaveBeenCalledWith(
        mockConfigPath,
        expect.stringContaining('"deepseek": "sk-ds-test"'),
        'utf-8'
      );
    });

    it('should overwrite an existing API key', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(
        JSON.stringify({
          apiKeys: { openai: 'sk-old' },
        })
      );

      setApiKey('openai', 'sk-new');

      expect(mockFs.writeFileSync).toHaveBeenCalledWith(
        mockConfigPath,
        expect.stringContaining('"openai": "sk-new"'),
        'utf-8'
      );
    });
  });

  describe('removeApiKey', () => {
    it('should remove existing key and return true', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(
        JSON.stringify({
          apiKeys: { test: 'value' },
        })
      );

      const result = removeApiKey('test');
      expect(result).toBe(true);
      expect(mockFs.writeFileSync).toHaveBeenCalledWith(
        mockConfigPath,
        expect.not.stringContaining('"test"'),
        'utf-8'
      );
    });

    it('should return false for non-existing key', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(JSON.stringify({ apiKeys: {} }));

      const result = removeApiKey('nonexistent');
      expect(result).toBe(false);
      expect(mockFs.writeFileSync).not.toHaveBeenCalled();
    });
  });

  describe('getApiKeys', () => {
    it('should return a copy of all api keys', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(
        JSON.stringify({
          apiKeys: { a: '1', b: '2' },
        })
      );

      const keys = getApiKeys();
      expect(keys).toEqual({ a: '1', b: '2' });
    });

    it('should return empty object when no keys configured', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(JSON.stringify({ apiKeys: {} }));

      expect(getApiKeys()).toEqual({});
    });
  });

  describe('getConfig', () => {
    it('should return the full config', () => {
      mockFs.existsSync.mockReturnValue(true);
      mockFs.readFileSync.mockReturnValue(
        JSON.stringify({
          apiKeys: { x: 'y' },
          port: 7000,
          host: '0.0.0.0',
        })
      );

      const config = getConfig();
      expect(config).toEqual({
        apiKeys: { x: 'y' },
        port: 7000,
        host: '0.0.0.0',
      });
    });
  });
});
