# Changelog

## [0.2.2](https://github.com/shenjingnan/haimen/compare/v0.2.1...v0.2.2) - 2026-07-19

### Fixed

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
