# Mutsuki Bot 产品工作规范

本目录是 **配置驱动的第一方 Mutsuki Bot 产品运行入口**，也是 Bot 产品需求进入 monorepo
后的能力边界核查入口。它只负责外部配置契约、catalog 聚合、产品装配、进程入口和跨 package
验收，不拥有 Core、Host、Bot、Agent 或平台能力的实现。

## 阅读顺序

1. 当前及关联 issue，确认目标、依赖和验收场景。
2. 读取 `../../AGENTS.md` 与 `../../plans/{roadmap,architecture,engineering,contracts}.md`。
3. 候选依赖仓库的 `AGENTS.md`、公开 API、manifest 和测试。
4. 本文件路由的相关技能，再检查当前实现、远端 commit 和 lockfile。

Issue 是需求线索，不是当前 API 的事实源。存在 `.codegraph/` 时，定位代码先用 CodeGraph。

## 技能路由

- `skills/capability-boundaries/SKILL.md`：判断能力归属和跨仓库顺序。
- `skills/bot-assembly/SKILL.md`：配置契约、LoadPlan 和 ServiceRuntime 装配。
- `skills/integration-testing/SKILL.md`：mock、fake server、真实 smoke、health 和 shutdown。

职责不明先读 capability-boundaries；涉及运行装配同时读 bot-assembly。

## 职责边界

| package 目录 | 职责 |
| --- | --- |
| `crates/*` | 领域中立 contracts、Task/Runner、资源、LoadPlan、Link 和 Rust Host/SDK 基础面 |
| `plugins/std` | 领域中立标准协议，以及 config/db/fs/http/observe/resource/workflow 插件 |
| `kits/python-runner` | Runner Link 的 Python contract mirror、Runner backend、transport 和测试工具 |
| `hosts/service` | 服务生命周期、配置/secret、插件加载、EventSource、控制面和 health |
| `plugins/bot` | `mutsuki.bot.*` 协议、Bot SDK、标准 Runner、平台 Adapter/Gateway 和显式 Host integration crate。库面 crate 持有 trait/service；可加载 plugin 面使用 `mutsuki-plugin-*` 名。`mutsuki-bot-state-db` 实现 conversation/persona/delivery/interaction/sandbox 库面 store，不依赖 plugin crate |
| `kits/agent` | Agent 协议、SDK、模型、工具和记忆能力 |
| `hosts/cli` | ServiceHost 公开控制 API 的 CLI/TUI 客户端 |
| `hosts/tauri` | 内嵌 Core 的桌面 Host、Tauri/WebView bridge 和前端 SDK |
| `hosts/web` | Web 运行宿主：HTTP/WS、静态资源、RPC/Event bridge、WebExtension 加载与 Recovery Shell |
| `hosts/distributed` | 分布式控制面、调度、恢复和资源预算 |
| 外部业务仓库 | 自己领域的协议、插件、Provider、Runner 或 sidecar |
| 本目录 | 第一方 Bot 外部配置、catalog 聚合、ServiceRuntime 启动、薄产品脚手架和跨 package 产品验收 |

## Hard Rules

1. 能力缺失时在 owner package 补齐并验证，再更新产品装配；禁止复制实现、生产 fallback 或兼容 shim。
2. 产品使用根 Workspace path 和唯一根 `Cargo.lock`；禁止仓库内 Mutsuki Git 依赖、仓库外 `path` 或本地 `[patch]`。
3. 配置只声明 capability、插件和部署选择。产品不按平台、Agent、Provider 或 backend 硬编码替代路径。
4. 产品只支持可执行文件旁 `.mutsuki-bot` 单实例目录；Host 边界与 SQLite repository 由产品内建，产品和 owner 配置进入 `ConfigRepository`，只保存 secret key 引用。
5. 产品不得拥有业务 Runner、命令、回复或 Agent 流程；这些能力由 owner package 实现，并遵守 batch-first、`TaskHandle` 和通用协议契约。编译期依赖 owner 插件的配置 schema 是允许的，不等于硬编码 backend 替代路径。`mutsuki-bot-runtime-reference` 只做域拓扑 bench，不是生产入口。
6. RuntimeProfile/RuntimeLoadPlan 是装配权威；registry freeze 后不得动态越权注册。
7. 缺失 capability、配置、secret、artifact 或 revision 必须结构化失败，禁止假成功和吞错。
8. 生产入口不接受配置路径、profile 或 namespace；产品显式选择固定 SQLite 配置仓库，但框架不假设路径或存储实现。空仓库只写一次版本化种子，直接启用 Agent Connections、Flow Router 与对应管理页，并把 `qq.business.full` 全量参考图作为 Flow 种子（已有 Flow 记录永不覆盖；节点目录不完整时种子不应用并保持空图）；QQ、Local Agent、Bot Agent、内置平台插件（B 站、B 站工房、米画师）和业务 Flow 的实际生效仍由保存后的 owner 配置显式启用，`runtime_plugins` 不得配置这些 owner 插件。Mock 仅限测试。
9. 产品自动装配持久化媒体资源 Provider `mutsuki.std.resource.sqlite`（数据库文件位于实例 `data/resources.sqlite`），并向 QQ、B 站、B 站工房与米画师工厂注入唯一 Provider 绑定；owner 配置文档不再携带 `media_provider_id` 字段，旧文档中的残留字段被忽略。
10. `sena-nana/MutsukiBotTemplate` 已退出源码和分发职责；不得恢复独立实现、Issue 或发布同步。
11. `create-bot` 只生成固定统一 revision、调用公开产品 API 的外部薄壳；不得复制第一方产品、
    Host、Bot、Agent 或插件实现，不得写入 Secret，也不得覆盖已有目录。

## Git 与验证

- 工作前后检查 `git status --short`；owner package 与产品装配在同一 revision 原子提交。
- Rust 或依赖改动运行 `cargo fmt --check`、`cargo check`、`cargo test`。
- 装配或依赖改动再运行 `cargo metadata --locked`，并在无兄弟仓库的主仓 clone 验证产品。
- 最终说明列出实际命令和结果；测试断言行为，不只匹配日志或文案。
