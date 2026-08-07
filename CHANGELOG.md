# Changelog

## Unreleased

## [0.12.0](https://github.com/shenjingnan/haimen/compare/v0.11.2...v0.12.0) - 2026-08-07

### Added

- *(agent)* 新增 Hermes Agent 后端 ([#169](https://github.com/shenjingnan/haimen/pull/169))
- *(agent)* Web 端切换 Agent 实时生效，无需重启 ([#168](https://github.com/shenjingnan/haimen/pull/168))
- *(agent)* 新增 OpenClaw Agent 后端 ([#167](https://github.com/shenjingnan/haimen/pull/167))

### Fixed

- *(codex)* 放开沙箱并修复 resume 会话 JSON 输出 ([#164](https://github.com/shenjingnan/haimen/pull/164))

### Other

- *(agents)* 拆分 claude-code/codex 为独立 workspace crate ([#166](https://github.com/shenjingnan/haimen/pull/166))

### Added

- *(agent)* Web 端切换 Agent 实时生效，无需重启——IM 网关 / xiaozhi 语音 / GitHub Webhook 三路径共享同一 Agent 句柄，切换后下一条消息即用新 Agent；因会话语义为切换即重置，切换后进行中的会话自动开启新对话

## [0.11.2](https://github.com/shenjingnan/haimen/compare/v0.11.1...v0.11.2) - 2026-08-06

### Added

- *(xiaozhi)* 连续音频管道——断档静音填充消除播放 underrun ([#162](https://github.com/shenjingnan/haimen/pull/162))

## [0.11.1](https://github.com/shenjingnan/haimen/compare/v0.11.0...v0.11.1) - 2026-08-06

### Other

- 补充 Windows PowerShell 一键安装说明 ([#160](https://github.com/shenjingnan/haimen/pull/160))
- 同步 README 中的 CLI 命令与配置示例 ([#158](https://github.com/shenjingnan/haimen/pull/158))

## [0.11.0](https://github.com/shenjingnan/haimen/compare/v0.10.0...v0.11.0) - 2026-08-05

### Added

- *(xiaozhi)* Agent 处理期间 TTS 中间状态播报与工具级播报 ([#157](https://github.com/shenjingnan/haimen/pull/157))
- *(xiaozhi)* 录音无语音超时后播报告别语并结束对话 ([#155](https://github.com/shenjingnan/haimen/pull/155))
- *(xiaozhi)* 硬件唤醒时服务端主动播报 TTS 问候 ([#153](https://github.com/shenjingnan/haimen/pull/153))
- *(agent)* 新增 Agent 调用日志（完整内容轨迹 + CLI + Web 展示） ([#152](https://github.com/shenjingnan/haimen/pull/152))

### Fixed

- *(xiaozhi)* 流式管线提前退出路径补齐 agent 调用日志 ([#156](https://github.com/shenjingnan/haimen/pull/156))
- *(xiaozhi)* 修复流式回放帧发送节奏，消除语音跳字与语速压缩 ([#154](https://github.com/shenjingnan/haimen/pull/154))
- *(xiaozhi)* 修复 LLM 大段 markdown 回复时 TTS 零音频 ([#145](https://github.com/shenjingnan/haimen/pull/145))

### Other

- *(deps)* bump webbrowser from 1.2.1 to 1.2.2 ([#151](https://github.com/shenjingnan/haimen/pull/151))
- *(deps)* bump toml from 1.1.3+spec-1.1.0 to 1.1.4+spec-1.1.0 ([#150](https://github.com/shenjingnan/haimen/pull/150))
- *(deps)* bump clap_complete from 4.6.7 to 4.6.8 ([#149](https://github.com/shenjingnan/haimen/pull/149))

## [0.10.0](https://github.com/shenjingnan/haimen/compare/v0.9.1...v0.10.0) - 2026-08-02

### Other

- *(deps)* 升级 univoice 0.1.4 → 0.1.6 并迁移 doubao 鉴权到 api_key ([#143](https://github.com/shenjingnan/haimen/pull/143))

## [0.9.1](https://github.com/shenjingnan/haimen/compare/v0.9.0...v0.9.1) - 2026-08-01

### Fixed

- 修复 Windows 发布构建 libopus __imp_floor 链接错误 ([#141](https://github.com/shenjingnan/haimen/pull/141))

## [0.9.0](https://github.com/shenjingnan/haimen/compare/v0.8.0...v0.9.0) - 2026-08-01

### Added

- 支持 Windows 发布与 npm shim 子进程启动 ([#140](https://github.com/shenjingnan/haimen/pull/140))
- 小智设备屏幕文本下发 + 豆包 TTS 模型选择 ([#138](https://github.com/shenjingnan/haimen/pull/138))
- *(web)* 新增消息渠道页展示飞书可用状态并提供配置 ([#137](https://github.com/shenjingnan/haimen/pull/137))

### Other

- *(agent)* Agent 插件化抽象层，注册表分发替代硬编码 ([#135](https://github.com/shenjingnan/haimen/pull/135))

## [0.8.0](https://github.com/shenjingnan/haimen/compare/v0.7.2...v0.8.0) - 2026-08-01

### Added

- *(gateway)* Agent 子进程支持指定工作目录，默认使用 ~/.haimen/workspace ([#132](https://github.com/shenjingnan/haimen/pull/132))

### Fixed

- *(release)* 修复发布二进制 Web 控制台白屏（前端产物未嵌入） ([#134](https://github.com/shenjingnan/haimen/pull/134))

## [0.7.2](https://github.com/shenjingnan/haimen/compare/v0.7.1...v0.7.2) - 2026-07-27

### Fixed

- *(install)* 移除 copy-installers.yml，使用 cargo-dist 默认安装脚本名称 ([#130](https://github.com/shenjingnan/haimen/pull/130))

## [0.7.1](https://github.com/shenjingnan/haimen/compare/v0.7.0...v0.7.1) - 2026-07-27

### Added

- *(asr)* 支持多 ASR 提供商动态切换（Qwen/GLM/MiMo/Xfyun） ([#127](https://github.com/shenjingnan/haimen/pull/127))

### Other

- *(asr)* 移除未使用的智谱AI和小米MiMo ASR 提供商 ([#128](https://github.com/shenjingnan/haimen/pull/128))

## [0.7.0](https://github.com/shenjingnan/haimen/compare/v0.6.1...v0.7.0) - 2026-07-27

### Added

- *(tts)* 新增 MiniMax TTS 提供商支持 ([#124](https://github.com/shenjingnan/haimen/pull/124))
- *(web)* ASR 配置支持运行时热加载，TTS 失败时播放内置提示音 ([#123](https://github.com/shenjingnan/haimen/pull/123))
- *(web)* TTS 配置保存后即时生效，支持运行时热加载 ([#122](https://github.com/shenjingnan/haimen/pull/122))
- *(start)* 新增 --log-level 和 --open-browser 参数，优化日志与浏览器行为 ([#121](https://github.com/shenjingnan/haimen/pull/121))

### Fixed

- *(tts)* StreamingOpusEncoder 流式编码 + 即时下发，改善长文本延迟 ([#119](https://github.com/shenjingnan/haimen/pull/119))

## [0.6.1](https://github.com/shenjingnan/haimen/compare/v0.6.0...v0.6.1) - 2026-07-26

### Fixed

- *(ci)* 修复安装脚本下载模式匹配名称 ([#114](https://github.com/shenjingnan/haimen/pull/114))

## [0.6.0](https://github.com/shenjingnan/haimen/compare/v0.5.1...v0.6.0) - 2026-07-26

### Added

- *(tts)* 新增固定文本模式，支持跳过 LLM 直接播报预设文本 ([#109](https://github.com/shenjingnan/haimen/pull/109))

### Fixed

- *(voice)* ASR 流式管线增加文本稳定超时 VAD 机制 ([#111](https://github.com/shenjingnan/haimen/pull/111))

## [0.5.1](https://github.com/shenjingnan/haimen/compare/v0.5.0...v0.5.1) - 2026-07-26

### Fixed

- *(docs)* 修复安装脚本 URL 和 Markdown 表格格式 ([#107](https://github.com/shenjingnan/haimen/pull/107))
- *(docs)* 修复安装脚本 URL 并新增 Gitee 国内镜像安装方式 ([#106](https://github.com/shenjingnan/haimen/pull/106))

### Other

- *(readme)* 更新 README 安装方式、CLI 命令和配置示例 ([#104](https://github.com/shenjingnan/haimen/pull/104))

## [0.5.0](https://github.com/shenjingnan/haimen/compare/v0.4.0...v0.5.0) - 2026-07-26

### Added

- *(config)* Agent 多服务商配置——后端重构及 Web 管理界面 ([#102](https://github.com/shenjingnan/haimen/pull/102))
- *(logging)* LLM 回复内容输出到日志 ([#101](https://github.com/shenjingnan/haimen/pull/101))
- *(config)* TTS 多服务商配置——后端重构及 Web 管理界面 ([#100](https://github.com/shenjingnan/haimen/pull/100))
- *(deps)* 升级 univoice 依赖至本地 v0.11.0 ([#99](https://github.com/shenjingnan/haimen/pull/99))
- *(config)* ASR 多服务商配置——后端重构及 Web 管理界面 ([#98](https://github.com/shenjingnan/haimen/pull/98))
- *(config)* 语音配置管理——ASR/TTS 配置化及 Web 管理界面 ([#96](https://github.com/shenjingnan/haimen/pull/96))

### Fixed

- *(xiaozhi-asr)* 修复流式 ASR VAD 延迟及多轮会话 VAD 残留问题 ([#97](https://github.com/shenjingnan/haimen/pull/97))

## [0.4.0](https://github.com/shenjingnan/haimen/compare/v0.3.0...v0.4.0) - 2026-07-20

### Added

- *(build)* cargo build 时自动构建 Web UI 前端并嵌入二进制 ([#88](https://github.com/shenjingnan/haimen/pull/88))
- *(start)* haimen start 自动启动 HTTP 服务器（Web 控制台 + xiaozhi + GitHub Webhook） ([#86](https://github.com/shenjingnan/haimen/pull/86))

## [0.3.0](https://github.com/shenjingnan/haimen/compare/v0.2.2...v0.3.0) - 2026-07-20

### Added

- *(serve)* 将 serve 默认模式改为 LLM 模式，Echo 模式需 --xiaozhi-echo 参数 ([#84](https://github.com/shenjingnan/haimen/pull/84))

### Fixed

- 将 serve 命令默认监听地址从 127.0.0.1 改为 0.0.0.0 ([#75](https://github.com/shenjingnan/haimen/pull/75))

### Other

- *(deps)* bump thiserror from 2.0.18 to 2.0.19 ([#78](https://github.com/shenjingnan/haimen/pull/78))
- *(deps)* bump univoice from 0.1.0 to 0.1.2 ([#82](https://github.com/shenjingnan/haimen/pull/82))
- *(deps)* bump tokio from 1.52.3 to 1.53.0 ([#83](https://github.com/shenjingnan/haimen/pull/83))
- *(deps)* bump anyhow from 1.0.103 to 1.0.104 ([#81](https://github.com/shenjingnan/haimen/pull/81))
- *(deps)* bump clap from 4.6.1 to 4.6.2 ([#80](https://github.com/shenjingnan/haimen/pull/80))
- *(deps)* bump serde_json from 1.0.150 to 1.0.151 ([#79](https://github.com/shenjingnan/haimen/pull/79))
- *(deps)* bump async-trait from 0.1.89 to 0.1.91 ([#77](https://github.com/shenjingnan/haimen/pull/77))
- *(deps)* bump toml from 1.1.2+spec-1.1.0 to 1.1.3+spec-1.1.0 ([#76](https://github.com/shenjingnan/haimen/pull/76))

## [0.2.2](https://github.com/shenjingnan/haimen/compare/v0.2.1...v0.2.2) - 2026-07-19

### Fixed

- 移除 +crt-static 以匹配 libopus_sys 的动态 CRT 使用方式
- release-plz 使用 PAT_TOKEN 创建 PR 以自动触发 CI workflow
- 使用 CFLAGS_x86_64_pc_windows_msvc=/MT 替代 CMAKE_MSVC_RUNTIME_LIBRARY 修复 Windows MSVC __imp_floor 链接问题
- 修复 Windows MSVC 构建时 libopus_sys 链接 __imp_floor 未解析的问题（完整方案） ([#73](https://github.com/shenjingnan/haimen/pull/73))
- 修复 Windows MSVC 构建时 libopus_sys 链接 __imp_floor 未解析的问题 ([#71](https://github.com/shenjingnan/haimen/pull/71))

## [0.2.1](https://github.com/shenjingnan/haimen/compare/v0.2.0...v0.2.1) - 2026-07-19

### Fixed

- 修复 gitignore 冲突导致 release-plz release-pr 失败 ([#69](https://github.com/shenjingnan/haimen/pull/69))

## [0.2.0](https://github.com/shenjingnan/haimen/compare/v0.1.0...v0.2.0) - 2026-06-22

### Added

- *(gateway)* 重构网关为多连接器并行架构 ([#45](https://github.com/shenjingnan/haimen/pull/45))
- *(xiaozhi)* 添加 crates/haimen-xiaozhi 实现硬件音频回环 ([#43](https://github.com/shenjingnan/haimen/pull/43))
- *(workspace)* 重构为 Cargo workspace 多 crate 架构 ([#41](https://github.com/shenjingnan/haimen/pull/41))
- *(connectors)* 实现钉钉 DingTalk 通道集成 ([#40](https://github.com/shenjingnan/haimen/pull/40))
- *(gateway)* 重构网关架构，引入 Connector/Agent 抽象层 ([#39](https://github.com/shenjingnan/haimen/pull/39))
- *(gateway)* 实现 Claude Code 多轮会话机制，新增会话管理与内置命令 ([#38](https://github.com/shenjingnan/haimen/pull/38))
- *(skill)* 为 commit-push-pr 技能添加 Attribution 配置信息 ([#34](https://github.com/shenjingnan/haimen/pull/34))
- *(cli)* 新增 upgrade/uninstall 命令，完善安装流程 ([#32](https://github.com/shenjingnan/haimen/pull/32))
- *(web)* 集成 React 前端，嵌入 Ant Design UI 与静态文件服务 ([#31](https://github.com/shenjingnan/haimen/pull/31))
- *(web)* 添加内嵌 Web 服务器能力，新增 haimen serve 子命令 ([#30](https://github.com/shenjingnan/haimen/pull/30))

### Other

- *(readme)* 更新 README 添加 logo 和项目徽章 ([#46](https://github.com/shenjingnan/haimen/pull/46))
- *(deps)* bump reqwest from 0.12.28 to 0.13.4 ([#37](https://github.com/shenjingnan/haimen/pull/37))
- *(deps)* bump axum from 0.7.9 to 0.8.9 ([#35](https://github.com/shenjingnan/haimen/pull/35))
- *(deps)* bump toml from 0.8.23 to 1.1.2+spec-1.1.0 ([#24](https://github.com/shenjingnan/haimen/pull/24))
- *(web)* 清理 web-ui 目录，添加 .vite 到 gitignore 并移除未使用的 shadcn/ui 组件 ([#33](https://github.com/shenjingnan/haimen/pull/33))

## [0.1.0] - 2026-06-05

### Added

- 项目初始化
- CLI 骨架（clap + tokio）
- 配置管理（TOML 配置读写）
- 双层日志系统（tracing）
- 日期时间工具模块
- CI/CD 配置（GitHub Actions）
- 代码质量工具（fmt, clippy, typos, tarpaulin, codecov）
- Shell 补全生成
