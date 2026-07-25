# TTS 多服务商配置——后端重构及 Web 管理界面

> 参考 ASR 多服务商模式，将 TTS 从单服务商（Doubao）改造为支持多服务商配置、按需切换的架构。

## 1. 现状分析

### 1.1 TTS 配置现状

当前 `TtsConfig` 采用扁平结构，直接绑定 Doubao（火山引擎）的凭证字段：

```rust
pub struct TtsConfig {
    pub provider: String,            // 固定 "doubao"，未参与运行时决策
    pub voice: Option<String>,
    pub app_key: Option<String>,
    pub access_token: Option<String>,
    pub cluster: Option<String>,
    pub resource_id: Option<String>,
}
```

所有 TTS 凭证字段都是顶级属性，扩展新的提供商必须在结构体中追加字段。

### 1.2 TTS 使用现状

三个 xiaozhi 响应策略均硬编码 `DoubaoTts::new(...)`：

| 策略 | 文件 | TTS 使用方式 |
|---|---|---|
| `TtsStrategy` | `src/xiaozhi_tts.rs` | `from_config()` 读取 `TtsConfig` 字段 → 直接构造 `DoubaoTts` |
| `AsrTtsStrategy` | `src/xiaozhi_asr_tts.rs` | 同上，硬编码 `DoubaoTts` |
| `AsrLlmTtsStrategy` | `src/xiaozhi_asr_llm_tts.rs` | 同上，硬编码 `DoubaoTts` |
| `build_xiaozhi_strategy()` | `src/gateway/mod.rs` | 仅从环境变量读取，不读配置 |

此外 API 层 `GET /api/v1/settings/tts/voices` 硬编码 `univoice::tts::voices::doubao::list_voices()`，不感知提供商切换。

### 1.3 前端现状

- `TtsSettings` TypeScript 类型是扁平结构，与后端 `TtsConfig` 一一对应
- `TtsSettingsPanel` 组件渲染固定表单（App Key / Access Token / 音色选择），无 Tab 切换
- 凭证验证复用 ASR 的 verify 端点，硬编码 `provider = "doubao"`
- 无 `tts-providers.ts` 元信息文件（ASR 侧有 `asr-providers.ts`）

### 1.4 ASR 参考实现

ASR 已完成多服务商改造，其模式为本次 TTS 改造的参考目标：

```rust
pub struct AsrConfig {
    pub active_provider: String,                          // 当前激活的提供商
    pub providers: HashMap<String, HashMap<String, String>>,  // {provider → {key → value}}
}
```

核心特征：
- **配置容器化**：凭证存放在 `HashMap` 中，不绑定具体字段名
- **激活机制**：`active_provider` 字段切换当前使用的提供商
- **向后兼容**：自定义 `Deserialize` 自动迁移旧格式
- **环境变量回退**：`get_credential()` 按提供商映射到对应环境变量
- **API 风格**：GET 返回完整 providers 映射，PUT 接收完整替换

## 2. 目标架构

### 2.1 改造目标

1. **配置层**：`TtsConfig` 改为 `active_provider` + `providers` 容器，支持 N 个提供商共存
2. **API 层**：GET/PUT 端点适配多提供商格式；新增 verify 端点；voices 端点支持按提供商查询
3. **前端层**：Tabs 面板切换各提供商配置（同 ASR 面板风格）
4. **使用侧**：策略代码根据 `active_provider` 动态选择 `TtsProvider` 实现
5. **向后兼容**：旧单服务商配置自动迁移到新格式
6. **univoice 库**：利用已存在的注册工厂和 8 个 Provider 实现

### 2.2 支持的服务商

根据 univoice 库已实现的 Rust TTS Provider：

| ID | 名称 | 必要凭证 | 可选参数 |
|---|---|---|---|
| `doubao` | 火山引擎 | `app_key`, `access_token` | `voice`, `cluster`, `resource_id` |
| `qwen` | 阿里通义千问 | `api_key` | `voice`, `model` |
| `qwen_realtime` | 阿里通义千问 Realtime | `api_key` | - |
| `glm` | 智谱AI | `api_key` | `voice` |
| `minimax` | MiniMax | `api_key` | `voice`, `model` |
| `openai` | OpenAI | `api_key` | `voice`, `model` |
| `xfyun` | 讯飞 | `app_id`, `api_key`, `api_secret` | - |
| `gemini` | Google Gemini | `api_key` | - |

### 2.3 音色数据来源

univoice 库已提供内置的音色列表：

| 提供商 | 音色模块 | 函数 |
|---|---|---|
| doubao | `voices::doubao` | `list_voices()` |
| glm | `voices::glm` | `list_voices()` |
| minimax | `voices::minimax` | `list_voices()` |
| qwen | `voices::qwen` | `list_voices()` / `list_voices_for_model()` |
| qwen_realtime | `voices::qwen_realtime` | `list_voices()` |

其余的（openai、xfyun、gemini）可调用 Provider 的 `list_voices()` trait 方法或返回空列表。

## 3. 详细技术方案

### 3.1 配置层改造（settings.rs）

```rust
/// TTS（语音合成）配置
///
/// 支持多服务商配置，通过 `active_provider` 切换当前使用的服务商：
///
/// ```toml
/// [tts]
/// active_provider = "doubao"
///
/// [tts.providers.doubao]
/// app_key = "..."
/// access_token = "..."
/// voice = "zh_female_xiaohe_uranus_bigtts"
/// cluster = "volcano_icl"
///
/// [tts.providers.qwen]
/// api_key = "..."
/// voice = "longxiaochun_v3"
/// model = "cosyvoice-v3-flash"
/// ```
///
/// # 向后兼容
///
/// 旧格式 `provider = "doubao"` + `app_key` / `access_token` 等字段在加载时自动迁移。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TtsConfig {
    /// 当前激活的 TTS 提供商
    #[serde(default = "default_tts_provider")]
    pub active_provider: String,
    /// 所有已配置的 TTS 提供商凭证 {provider_name → {key → value}}
    #[serde(default)]
    pub providers: HashMap<String, HashMap<String, String>>,
}
```

**向后兼容设计**（旧 → 新）：

```
# 旧格式（自动迁移到新格式）
[tts]
provider = "doubao"          # → active_provider
app_key = "xxx"              # → providers.doubao.app_key
access_token = "yyy"         # → providers.doubao.access_key
voice = "zh_female_..."      # → providers.doubao.voice
cluster = "volcano_icl"      # → providers.doubao.cluster
resource_id = "seed-..."     # → providers.doubao.resource_id
```

通过 `TtsConfigLegacy` 辅助结构体 + 自定义 `Deserialize` 实现（与 `AsrConfigLegacy` 模式一致）。

**凭证解析方法**：

```rust
impl TtsConfig {
    /// 获取当前激活提供商的某个凭证值（配置优先 → 环境变量回退）
    pub fn get_credential(&self, key: &str) -> Option<String> {
        if let Some(val) = self.providers.get(&self.active_provider)
            .and_then(|p| p.get(key))
            .filter(|v| !v.is_empty())
        {
            return Some(val.clone());
        }
        // 按提供商映射到环境变量
        match (self.active_provider.as_str(), key) {
            ("doubao", "app_key")       => std::env::var("DOUBAO_APP_KEY").ok(),
            ("doubao", "access_token")  => std::env::var("DOUBAO_ACCESS_TOKEN").ok(),
            ("doubao", "voice")         => std::env::var("DOUBAO_VOICE_TYPE").ok(),
            ("doubao", "cluster")       => std::env::var("DOUBAO_CLUSTER").ok(),
            ("qwen", "api_key")         => std::env::var("QWEN_API_KEY").ok(),
            ("glm", "api_key")          => std::env::var("GLM_API_KEY").ok(),
            ("openai", "api_key")       => std::env::var("OPENAI_API_KEY").ok(),
            ("minimax", "api_key")      => std::env::var("MINIMAX_API_KEY").ok(),
            ("xfyun", "app_id")         => std::env::var("XFYUN_APP_ID").ok(),
            ("xfyun", "api_key")        => std::env::var("XFYUN_API_KEY").ok(),
            ("xfyun", "api_secret")     => std::env::var("XFYUN_API_SECRET").ok(),
            ("gemini", "api_key")       => std::env::var("GEMINI_API_KEY").ok(),
            _ => None,
        }
    }
    
    /// 获取音色（兼容旧接口，从 providers 或环境变量读取）
    pub fn resolved_voice(&self) -> Option<String> {
        self.get_credential("voice")
    }
}
```

**注意**：`cluster`、`resource_id` 归入 doubao 的 providers 字段中，不再作为顶级属性。

### 3.2 API 层改造（voice_settings.rs）

**现有端点改造**：

| 方法 | 路径 | 变更 |
|---|---|---|
| `GET` | `/api/v1/settings/tts` | 返回 `{active_provider, providers, resolved}` 格式（同 ASR） |
| `PUT` | `/api/v1/settings/tts` | 接收 `{active_provider, providers}`，完整替换 |
| `GET` | `/api/v1/settings/tts/voices` | 新增 `?provider=` 查询参数；无参数时返回当前激活提供商的音色 |

**新增端点**：

| 方法 | 路径 | 功能 |
|---|---|---|
| `POST` | `/api/v1/settings/tts/verify` | 验证指定提供商的凭证有效性 |

**GET /api/v1/settings/tts 返回格式**：

```json
{
  "success": true,
  "data": {
    "active_provider": "doubao",
    "providers": {
      "doubao": {
        "app_key": "xxx",
        "access_token": "yyy",
        "voice": "zh_female_xiaohe_uranus_bigtts",
        "cluster": "volcano_icl"
      },
      "qwen": {
        "api_key": "zzz"
      }
    },
    "resolved": {
      "app_key": "xxx",
      "access_token": "yyy",
      "voice": "zh_female_xiaohe_uranus_bigtts"
    }
  }
}
```

**GET /api/v1/settings/tts/voices 改造**：

```rust
pub async fn list_tts_voices(
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let provider = params.get("provider").map(String::as_str).unwrap_or("doubao");
    let voices = match provider {
        "doubao"  => univoice::tts::voices::doubao::list_voices(),
        "qwen"    => univoice::tts::voices::qwen::list_voices(),
        "glm"     => univoice::tts::voices::glm::list_voices(),
        "minimax" => univoice::tts::voices::minimax::list_voices(),
        "qwen_realtime" => univoice::tts::voices::qwen_realtime::list_voices(),
        _ => Vec::new(),
    };
    // ...
}
```

**POST /api/v1/settings/tts/verify**：

同 ASR verify 模式，按 provider 分流：

| 提供商 | 验证方式 |
|---|---|
| `doubao` | 使用提供的凭证调用 TTS 合成测试音频（已有逻辑，从 voice_settings.rs 提取复用） |
| `qwen` | HTTP GET 到 dashscope 模型列表 API，检查非 401/403 |
| `glm` | HTTP GET 到智谱 API |
| `openai` | HTTP GET 到 OpenAI 模型列表 API |
| `minimax` | HTTP GET 到 MiniMax API |
| `xfyun` | 返回"需 WebSocket HMAC 鉴权，保存后直接测试" |
| `gemini` | HTTP GET 到 Gemini API |

### 3.3 路由注册（web/mod.rs）

在已有 `voice_routes` 中新增 `POST /api/v1/settings/tts/verify` 路由。

### 3.4 前端改造

#### 3.4.1 新增 `data/tts-providers.ts`

定义各 TTS 提供商元信息（与 `asr-providers.ts` 结构一致）：

```typescript
export const TTS_PROVIDERS: ProviderInfo[] = [
  {
    id: 'doubao',
    name: '火山引擎',
    fields: [
      { key: 'app_key', label: 'App Key', type: 'password', placeholder: '未设置，可用环境变量 DOUBAO_APP_KEY' },
      { key: 'access_token', label: 'Access Token', type: 'password', placeholder: '未设置，可用环境变量 DOUBAO_ACCESS_TOKEN' },
      { key: 'cluster', label: 'Cluster', type: 'text', placeholder: '可选，如 volcano_icl' },
      { key: 'resource_id', label: 'Resource ID', type: 'text', placeholder: '可选，如 seed-tts-2.0' },
    ],
  },
  {
    id: 'qwen',
    name: '阿里通义千问',
    fields: [
      { key: 'api_key', label: 'API Key', type: 'password' },
      { key: 'model', label: '模型', type: 'text', placeholder: 'cosyvoice-v3-flash' },
    ],
  },
  // ...glm, minimax, openai, xfyun, gemini
];
```

#### 3.4.2 更新类型定义

```typescript
export interface TtsSettings {
  active_provider: string;
  providers: Record<string, Record<string, string>>;
  resolved?: {
    app_key: string | null;
    access_token: string | null;
    voice: string | null;
  };
}
```

#### 3.4.3 更新 API 客户端

```typescript
export async function getTtsSettings(): Promise<TtsSettings> { ... }
export async function updateTtsSettings(settings: {
  active_provider?: string;
  providers?: Record<string, Record<string, string>>;
}): Promise<TtsSettings> { ... }
export async function listTtsVoices(provider?: string): Promise<TtsVoice[]> { ... }
export async function verifyTtsCredentials(
  creds: Record<string, string>,
  provider: string,
): Promise<{ valid: boolean; message: string }> { ... }
```

#### 3.4.4 重构 TtsSettingsPanel

改造为与 `AsrSettingsPanel` 一致的 Tab 面板结构：

- Tabs 栏：每个提供商一个 Tab（由 `TTS_PROVIDERS` 驱动）
- 当前激活提供商显示 ✓ 标记
- 每 Tab 内：动态字段（PasswordInput）+ 音色选择器（VoiceSelector）
- 操作按钮：验证凭证 / 保存配置 / 设为首选
- 音色列表根据当前 Tab 提供商动态加载

### 3.5 使用侧改造（核心）

#### 3.5.1 TTS Provider 工厂函数

新增 `src/tts_factory.rs`：

```rust
/// 根据配置创建 TTS Provider 实例
pub fn create_tts_provider(config: &TtsConfig) -> Result<Box<dyn TtsProvider>, String> {
    let provider = config.active_provider.as_str();
    match provider {
        "doubao" => {
            let app_key = config.get_credential("app_key").ok_or("缺少 app_key")?;
            let access_token = config.get_credential("access_token").ok_or("缺少 access_token")?;
            let voice = config.get_credential("voice");
            let cluster = config.get_credential("cluster");
            let resource_id = config.get_credential("resource_id");
            
            let resource_id = resource_id.or_else(|| match cluster.as_deref() {
                Some("volcano_icl") => Some("seed-tts-1.0".into()),
                _ => Some("seed-tts-2.0".into()),
            });
            
            Ok(Box::new(DoubaoTts::new(DoubaoTtsOption {
                base: BaseTtsOption { voice: voice.map(Into::into), ..Default::default() },
                app_id: Some(app_key),
                access_token: Some(access_token),
                resource_id,
                ..Default::default()
            })))
        }
        "qwen" => {
            let api_key = config.get_credential("api_key").ok_or("缺少 api_key")?;
            let model = config.get_credential("model");
            let voice = config.get_credential("voice");
            Ok(Box::new(QwenTts::new(QwenTtsOption {
                base: BaseTtsOption { voice: voice.map(Into::into), ..Default::default() },
                api_key,
                model: model.unwrap_or_else(|| "cosyvoice-v3-flash".into()),
                ..Default::default()
            })))
        }
        "glm" => { /* 类似 */ }
        "minimax" => { /* 类似 */ }
        "openai" => { /* 类似 */ }
        "xfyun" => { /* 类似 */ }
        "gemini" => { /* 类似 */ }
        _ => Err(format!("不支持的 TTS 提供商: {}", provider)),
    }
}
```

也可以利用 univoice 已有的 `registry::create_tts()` 工厂，但需要将 `TtsConfig` 的 `providers` HashMap 转换为各 Provider 对应的 Option 结构体。

#### 3.5.2 策略代码改造

三个策略的改造原则：**策略不再直接持有凭证字符串，而是持有 `TtsConfig` 引用或 `Box<dyn TtsProvider>`**。

**方案 A（推荐）**：策略持有 `TtsConfig` + 惰性初始化 Provider

每个策略的 `from_config()` 方法已接受 `&TtsConfig`，在 `generate_response()` 时调用工厂函数创建 Provider。

```
// 改造前
let tts = DoubaoTts::new(DoubaoTtsOption { app_id, access_token, ... });

// 改造后
let tts = create_tts_provider(&self.tts_config)?;
let response = tts.synthesize(TtsRequest { text, ... }).await?;
```

**gateway::build_xiaozhi_strategy()**：

改为从配置（而非仅环境变量）读取凭证：

```rust
fn build_xiaozhi_strategy(config: &AppConfig) -> Option<Arc<dyn ResponseStrategy>> {
    // 读取 TTS 配置，尝试获取凭证
    let tts_config = &config.tts;
    let has_creds = tts_config.get_credential("app_key").is_some()
        || tts_config.get_credential("api_key").is_some();
    
    if !has_creds {
        tracing::info!("...不启动 xiaozhi");
        return None;
    }
    
    Some(Arc::new(AsrLlmTtsStrategy::from_config(
        &config.asr, tts_config, None, llm_agent,
    )))
}
```

### 3.6 模块注册（lib.rs）

新增 `mod tts_factory;` 声明。

## 4. 实施方案

### 阶段一：配置层 + API 层改造

**涉及文件**：
- `src/config/settings.rs` — `TtsConfig` 重构 + 旧格式迁移 + 凭证解析方法
- `src/web/api/voice_settings.rs` — GET/PUT 端点多提供商化 + verify 端点 + voices 端点改造
- `src/web/mod.rs` — 新增路由注册

**验收标准**：
1. 现有 `TtsConfig` 的扁平 TOML 配置可自动加载为多提供商格式
2. `GET /api/v1/settings/tts` 返回多提供商格式
3. `PUT /api/v1/settings/tts` 可完整替换 providers
4. `GET /api/v1/settings/tts/voices?provider=doubao` 返回火山引擎音色
5. `GET /api/v1/settings/tts/voices?provider=qwen` 返回 CosyVoice 音色
6. `POST /api/v1/settings/tts/verify` 对支持的提供商返回验证结果
7. 通过 `cargo test` 测试

### 阶段二：前端改造

**涉及文件**：
- `web-ui/src/data/tts-providers.ts` — 新增
- `web-ui/src/types/index.ts` — 更新 `TtsSettings` 类型
- `web-ui/src/api/voice.ts` — 更新 API 函数
- `web-ui/src/pages/Settings/VoiceSettings.tsx` — 重构 `TtsSettingsPanel`

**验收标准**：
1. TTS 配置面板使用 Tabs 展示所有提供商
2. 每个提供商显示其对应的字段（根据 `tts-providers.ts`）
3. 可切换激活提供商
4. 可验证凭证
5. 音色选择器根据当前 Tab 加载对应音色列表
6. 保存后配置持久化到 settings.toml
7. 通过 `pnpm check` 检查

### 阶段三：使用侧改造

**涉及文件**：
- `src/tts_factory.rs` — 新增工厂函数
- `src/xiaozhi_tts.rs` — `TtsStrategy` 改造
- `src/xiaozhi_asr_tts.rs` — `AsrTtsStrategy` 改造
- `src/xiaozhi_asr_llm_tts.rs` — `AsrLlmTtsStrategy` 改造
- `src/gateway/mod.rs` — `build_xiaozhi_strategy` 改造
- `src/cli.rs` — 策略构造调用适配
- `src/lib.rs` — 模块声明

**验收标准**：
1. 工厂函数可按 `active_provider` 创建对应 TTS Provider
2. 三个策略在 `generate_response()` 时使用工厂创建的 Provider
3. TTS 功能回归测试通过（需实际凭证）
4. 单元测试通过 `cargo test`

### 阶段四：集成测试与清理

**涉及内容**：
- 端到端测试（从前端配置 → 后端持久化 → 策略使用）
- 旧代码清理（移除 `cluster`、`resource_id` 等不再作为顶级字段的代码引用）
- 文档同步

**验收标准**：
1. 完整链路可用：Web UI 配置 → 切换提供商 → 策略使用新提供商
2. `cargo fmt --check && cargo clippy -- -D warnings && cargo test` 全部通过
3. `pnpm check` 全部通过

## 5. 关键风险与注意事项

1. **音色兼容性**：不同提供商的音色 ID 不互通，切换提供商后需重新选择音色。每个提供商的音色应独立存储在 `providers[provider_name].voice` 中（已在 providers HashMap 中）。

2. **cluster/resource_id 归并**：`cluster` 和 `resource_id` 是 doubao 特有的参数，迁移后纳入 `providers.doubao` 映射。策略代码中 `resolve_resource_id()` 逻辑（根据 cluster 推导 resource_id）保持不变，但数据来源改为从 providers 读取。

3. **向后兼容测试**：旧格式迁移的测试用例应覆盖所有可能的字段组合（仅 `provider`、`provider+app_key`、全部字段等）。

4. **xfen 验证限制**：讯飞使用 WebSocket HMAC 鉴权，无法通过简单的 HTTP 验证。沿用 ASR 的处理方式，返回提示信息。

5. **策略持有 TtsConfig 的生命周期**：TtsConfig 在策略构造时被引用，需确保其生命周期足够长（策略持有 cloned TtsConfig 或 Arc 共享）。
