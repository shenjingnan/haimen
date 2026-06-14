# Web UI 设计方案：React SPA 嵌入 Rust 二进制

> 日期: 2026-06-14
> 状态: 设计确认

## 概述

为 haimen CLI 工具的嵌入式 Web 服务器设计完整的 **AI 网关管理控制台**，支持复杂 UI 交互和多种 UI 组件。

## 技术选型

| 层面 | 选择 | 原因 |
|------|------|------|
| 前端框架 | React + TypeScript | 生态丰富 |
| 构建工具 | Vite | 快速、React 生态标配 |
| 组件库 | Shadcn/ui + Tailwind CSS | 轻量、可控、产物小，适合嵌入二进制 |
| 数据获取 | TanStack Query (React Query) | 自动缓存/重取/错误处理 |
| 路由 | React Router v7 | React 生态标准 |
| 表单 | react-hook-form + zod | 类型安全表单校验 |
| 后端 | axum | 已有依赖，新增 API 路由 |
| 静态资源嵌入 | rust-embed | 将前端产物嵌入 Rust 二进制 |
| 静态文件服务 | tower-http | 配合 rust-embed 或开发模式代理 |

## 项目结构

```
haimen/
├── Cargo.toml                    # 新增 tower-http, rust-embed 依赖
├── src/
│   ├── web/
│   │   ├── mod.rs                # 重构：拆分路由、静态文件、API
│   │   ├── api/                  # API 路由模块
│   │   │   ├── mod.rs
│   │   │   ├── dashboard.rs
│   │   │   ├── messages.rs
│   │   │   ├── config.rs
│   │   │   ├── auth.rs
│   │   │   ├── gateway.rs
│   │   │   └── system.rs
│   │   └── static.rs             # rust-embed 静态文件服务
│   └── ...
├── web-ui/                       # React 前端项目
│   ├── src/
│   │   ├── components/
│   │   │   ├── layout/           # Sidebar, Topbar, Layout
│   │   │   ├── ui/               # Shadcn/ui 组件
│   │   │   └── shared/           # 共享组件
│   │   ├── pages/
│   │   │   ├── Dashboard.tsx
│   │   │   ├── Messages.tsx
│   │   │   ├── MessageDetail.tsx
│   │   │   ├── Config.tsx
│   │   │   ├── Auth.tsx
│   │   │   ├── Gateway.tsx
│   │   │   └── GatewayLogs.tsx
│   │   ├── hooks/
│   │   ├── api/
│   │   ├── types/
│   │   ├── App.tsx
│   │   ├── main.tsx
│   │   └── index.css
│   ├── dist/                     # 构建产物（嵌入 Rust 二进制）
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── tailwind.config.ts
│   ├── components.json
│   └── package.json
└── ...
```

## 页面和路由

| 路径 | 页面 | 说明 |
|------|------|------|
| `/` | Dashboard | 仪表盘，网关概览 |
| `/messages` | MessageList | 飞书消息列表 |
| `/messages/:id` | MessageDetail | 消息详情 |
| `/config` | Configuration | 系统配置管理 |
| `/auth` | AuthManagement | 飞书认证管理 |
| `/gateway` | GatewayMonitor | 网关运行状态监控 |
| `/gateway/logs` | GatewayLogs | 网关日志查看 |

## API 设计

所有 API 统一 `/api/v1/` 前缀：

```
GET    /api/v1/dashboard/stats     → 网关统计概览
GET    /api/v1/messages            → 消息列表（分页筛选）
GET    /api/v1/messages/:id        → 消息详情
DELETE /api/v1/messages/:id        → 删除消息
GET    /api/v1/config              → 获取配置
PUT    /api/v1/config              → 更新配置
GET    /api/v1/auth/status         → 认证状态
POST   /api/v1/auth/login          → 触发认证
POST   /api/v1/auth/logout         → 登出
GET    /api/v1/gateway/status      → 网关运行状态
GET    /api/v1/gateway/logs        → 网关日志（SSE 流式）
GET    /api/v1/gateway/stats       → 网关统计数据
GET    /api/v1/system/info         → 系统信息（版本、运行时间）
```

## 开发工作流

### 开发模式
```bash
# 终端 1：前端 Vite dev server → localhost:5173
cd web-ui && pnpm dev

# 终端 2：Rust 后端 → localhost:9527
cargo run -- serve

# Vite 配置 proxy /api/* → localhost:9527
```

### 生产构建
```bash
cd web-ui && pnpm build       # → web-ui/dist/
cargo build --release          # 嵌入 dist/ 内容
```

## Rust 端核心实现

### 新增依赖
```toml
tower-http = { version = "0.5", features = ["cors", "fs"] }
rust-embed = "8"
```

### 静态文件服务
```rust
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web-ui/dist"]
struct Assets;

// 静态文件服务 + SPA fallback
// - /api/* 路由交给 API 处理
// - 其他路由优先查找嵌入文件，找不到则返回 index.html
```

## 分阶段实施

| 阶段 | 内容 | 关键文件 |
|------|------|----------|
| Phase 1 | 前端项目初始化 + Rust 静态文件服务 + SPA 骨架 | `web-ui/`, `src/web/static.rs`, `src/web/mod.rs` |
| Phase 2 | API 路由拆分 + 仪表盘页面 | `src/web/api/*`, `Dashboard.tsx` |
| Phase 3 | 消息、配置、认证功能页面 | `pages/*.tsx`, `api/*.rs` |
| Phase 4 | 网关监控 + SSE 实时日志 | `Gateway.tsx`, `GatewayLogs.tsx` |
| Phase 5 | 测试 + 优化 + 文档 | 全局 |

## 验证方法

- `cargo build --release` 成功
- `cargo clippy -- -D warnings` 无警告
- `cargo fmt --check` 格式正确
- `haimen serve` 启动后浏览器可访问所有页面
- API 端点返回正确 JSON
