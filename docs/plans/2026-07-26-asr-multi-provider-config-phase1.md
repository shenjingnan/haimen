# ASR 多服务商配置 — 第一阶段（后端重构）

## 改动总结

将 `AsrConfig` 从扁平硬编码结构重构为支持多服务商的容器结构，实现配置迁移、API 更新和前端类型适配。

## 改动文件

### 已修改

| 文件 | 改动 |
|------|------|
| `src/config/settings.rs` | 重构 `AsrConfig` 为 `active_provider` + `providers`；添加自定义 `Deserialize` 支持旧格式自动迁移；添加 `get_credential`/`get_provider_credential` 方法 |
| `src/web/api/voice_settings.rs` | 更新 GET/PUT/verify 端点适配新结构；添加 `parse_providers` 工具函数；verify 端点支持按 provider 验证 |
| `web-ui/src/types/index.ts` | 更新 `AsrSettings` 接口定义 |
| `web-ui/src/api/voice.ts` | 更新 API 调用参数为新结构 |

### 无需修改

| 文件 | 原因 |
|------|------|
| `src/xiaozhi_asr_tts.rs` | 通过 `resolved_app_key()`/`resolved_access_token()` 获取凭证，接口兼容 |
| `src/xiaozhi_asr_llm_tts.rs` | 同上 |
| `src/cli.rs` | 传递 `&settings.asr` 引用，接口兼容 |

## 向后兼容

旧 TOML 格式：
```toml
[asr]
provider = "doubao"
app_key = "xxx"
access_token = "xxx"
```

自动迁移为新格式：
```toml
[asr]
active_provider = "doubao"

[asr.providers.doubao]
app_key = "xxx"
access_key = "xxx"
```

## 验证结果

- `cargo fmt --check` — ✅
- `cargo clippy -- -D warnings` — ✅
- `cargo test` — 321 tests passed ✅
- 集成测试 — 6 tests passed ✅
- 前端 Biome check — 仅预存 lint 错误 ✅

## 下一步（第二阶段）

Web UI 前端重构：Tab 切换 + 动态字段渲染（等待产品设计确认后实施）
