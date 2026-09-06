# MutsukiBotPlugins

## Bilibili / Workshop / Mihuashi

Mutsuki-native 迁移提供以下 builtin Rust 协议：

- `mutsuki.bot.bilibili.poll/live@1`
- `mutsuki.bot.bilibili.poll/dynamic@1`
- `mutsuki.bot.bilibili.poll/video@1`
- `mutsuki.bot.bilibili.card/render@1`
- `mutsuki.bot.bilibili.link/resolve@1`
- `mutsuki.bot.bilibili.workshop.link/resolve@1`
- `mutsuki.bot.mihuashi.link/resolve@1`

`mutsuki-bot-link-parser` 是共享库而非 Host 插件，负责卡片 JSON 展开、URL 提取去重
与冷却辅助。Text/Markdown/`ark`/`ark_data`/`msg_elements` 走同一套抽链。
Flow 用 `mutsuki.bot.match.link` 按域名白名单匹配，matched 口写入 `bot.link.url`。
Bilibili `web_cookie` 贡献 `mutsuki.bot.bilibili.resolve`，米画师贡献
`mutsuki.bot.mihuashi.resolve`。示例图 `qq_link_resolve_flow()` 为 Source → link match →
resolve → `qq.send`。B 站小程序自动解析需要 `web_cookie` 和全量群消息
（`GROUP_MESSAGE_CREATE`）；AT-only 收不到未 @ 的分享。
ConfigService 只接受一个激活 flow 文档；`qq_full_business_flow()`（flow_id
`qq.business.full`）把 AI 对话、链接解析与 Bilibili 推送子图合并为一张全量参考图。
第一方产品在配置仓库没有 flow 记录时于启动种子化该图，已有记录永不覆盖；
示例 `configs/flow-full.example.json` 供 Agent flow 工具应用或对照参考。
Bilibili 状态固定写入 ServiceHost
`data_dir/bilibili/state.sqlite3`；首次轮询只建立 cursor，不补发历史。轮询检测到新条目后不再
直连发送：runner 提交 `mutsuki.bot.event.bilibili` v1 触发事件（载荷 `BilibiliNotification`，
target 取自订阅），推送卡片渲染与投递由 Flow 子图
`mutsuki.bot.bilibili.notification` → `mutsuki.bot.bilibili.card` → 平台 send 节点完成
（参考图 `bilibili_push_flow()`，一条链按 `BilibiliNotification.kind` 覆盖直播/动态/视频）。
活动图中没有匹配 Source 时事件按 ingress 语义静默丢弃，
升级后需在 Flow 编辑器或 Agent flow 工具中重建推送子图。B 站只支持
`backend.type = "web_cookie"`：Web backend 的 Cookie 只通过
`backend.cookie_secret_key` 进入共享 credential boundary，WBI 请求使用运行时获取的
mixin key 和注入式签名函数。

图片通过显式 `media_provider_id` 创建 `ResourceRef`，单资源上限 8 MiB。QQ adapter
从 Host registry 打开最新版 descriptor、读取并校验摘要、分块上传，随后按 segment
顺序发送 image/text。米画师 runner 使用 `TaskAwaitRunnerAdapter` 调用
`mutsuki.browser.snapshot`，不拥有 Chromium 生命周期。

账号与订阅管理以 Bilibili Processor 节点进入同一个 batch-first runner；命令路径由图中的
Command Match 节点配置。扫码登录与凭据轮换始终可用（聊天内仍需管理员），不依赖 management
开关；启用 management 后另提供：签名验证码自助绑定、订阅列表/暂停/恢复/删除，以及不推进
cursor 的最新动态预览。二维码在 runner 内生成 PNG `ResourceRef`，Cookie 不进入消息、Task
payload、manifest、日志或 trace；Web 控制台不提供手动 Cookie 输入，配置描述符隐藏 `cookie`
字段，凭据只能经扫码轮换。

管理操作只通过 ServiceHost 的原子 secret/config persistence handle 落盘：扫码成功轮换
`backend.cookie_secret_key` 指向的本地 secret，订阅变更替换产品配置中 Bilibili owner 的 opaque
config。插件 SQLite 只保存 cursor、cooldown 和未完成的 QR/绑定 challenge，不是订阅关系
权威。management 默认关闭；启用时必须从真实产品配置文件启动并配置 Host
`security.secret_file`。

Bilibili 动态 API 的 352 风控回退默认关闭。产品必须同时显式配置
`risk_control.backend = "chromium"` 和 `mutsuki.std.io.browser.chromium`；Bilibili Runner
仅通过通用 `mutsuki.browser.snapshot` 子任务获取 DOM，不拥有 Chromium 生命周期。
Chromium factory 在启动阶段校验 executable，provider 与 Bilibili owner 配置分别限制
domain、timeout、DOM 和读取响应大小。未配置 backend、浏览器任务失败、重定向越域或
响应超限都会结构化失败；成功回退写入 `mutsuki.bot.bilibili.risk_control/status@1`
degradation event。配置与验证层级见 `docs/bilibili-risk-control.md`；账号与订阅管理见
`docs/bilibili-management.md`。

MutsukiBotPlugins is the batch-first Bot domain plugin collection for Mutsuki. It is not a Host and it is not a Core extension.

The repository owns Bot protocol objects, Bot authoring helpers, Bot event routing, Bot command parsing, and platform adapter plugins such as QQBot. Runtime scheduling, runner lifecycle, host startup, Python runner execution, plugin marketplace behavior, and product-specific business bots stay outside this repository.

Complete crate table and Host-assembly boundaries: `docs/architecture.md`.

## MVP Crates

- `mutsuki-config-service` / `mutsuki-config-derive`: Schema-first ConfigDescriptor + `#[derive(MutsukiConfig)]`
- `mutsuki-plugin-config-web`: 默认 Web 配置插件（Lilia Workspace 壳 + `@mutsuki/ui` styles）
- `mutsuki-plugin-bot-control-web`: ServiceHost ControlMethod 的 `control.*` Web RPC 代理（`runtime.read` / `runtime.write` 门禁；含 task 调试与 lifecycle drain/shutdown）
- `mutsuki-plugin-bot-overview-web`: Web 概览（`overview.summary`：经 control-web 聚合状态/结构/计数/uptime）
- `mutsuki-plugin-bot-database-web`: Web 数据库查看（读取当前 Bot 实际接入的 `BotStateDb`：表列表、列结构、分页行）
- `mutsuki-bot-web-console`: 嵌入式 Bot 管理台装配（WebHost + control/overview/config/upgrade extensions）。产品路径仅 Embedded；不提供 Standalone / 分进程 Console 装配。
- `examples/config-demo`: Discord-like 最小可用配置闭环

WebHost 依赖：Bot package 通过根 Workspace path 使用 `mutsuki-web-host` / `mutsuki-web-protocol`，
并与 `products/bot` 在同一 revision 原子验证。`web_host` 不是独立产品部署能力，仅作为 Embedded
Console 的库依赖。

- `mutsuki-bot-protocol`: common event/message and typed Bot Flow contracts.
- `mutsuki-bot-flow`: Bot-owned graph catalog validation, immutable versions and revision CAS.
- `mutsuki-bot-sdk`: author-facing helpers that lower to Mutsuki task protocols.
- `mutsuki-plugin-bot-event-router`: `mutsuki.bot.flow/ingress@1` DAG executor and match nodes.
- `mutsuki-plugin-bot-command`: graph-configured command Match node.
- `mutsuki-plugin-bot-agent`: explicit QQ-to-AgentKit bridge with durable conversation/session
  handling and production configured factory `mutsuki.plugin.bot.agent`. Its config stores only
  `connection_id`; AgentKit supplies the selected `agent_connection:<id>`.
- `mutsuki-plugin-bot-agent-web`: authenticated Agent connection management only; event matching
  is edited in the Flow page.
- `mutsuki-bot-sandbox`: QQ conversation sandbox with `BotStateDb` history. Simulate mode is a
  Koishi-style closed loop through Bot Flow; live mode projects real inbound events.
  Conversations, users and messages hydrate from `bot_sandbox_*` tables on startup;
  other plugins query the same tables through `BotStateDbRepository`.
- `mutsuki-plugin-bot-sandbox-web`: WebExtension for the shared simulate/live Stapxs-style QQ conversation client.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform adapter for gateway events and message/media OpenAPI tasks.
- `mutsuki-bot-service-host-integration`: configured native factories, QQ EventSource bundle, and sandbox outbound intercept.
- `mutsuki-bot-testkit`: reusable fake QQ HTTP/WebSocket boundary for downstream product E2E.
- `examples/bot-echo`: platform-neutral example business plugin that depends only on Bot protocols and SDK helpers.

## Plugin Discovery

The substantive native plugin crates generate current `PluginManifest` values from their runner
descriptors through the Mutsuki SDK `PluginBuilder`:

- `mutsuki-plugin-bot-event-router`: provides `flow/ingress@1` DAG execution plus graph-owned
  match nodes (including rate-limit).
- `mutsuki-plugin-bot-command`: provides a typed command Match node.
- `mutsuki-plugin-bot-agent`: provides submit/cancel/reset/fork/status/regenerate nodes; its
  AgentClient and product state are injected by an explicit product bundle.
- `mutsuki-plugin-bot-adapter-qqbot`: provides standard Bot message/media tasks and QQBot-specific account, gateway status, and raw call tasks.

The generated manifest is the only host-loadable source of truth. This repository does not keep
the legacy `[plugin]` / `[[provides]]` authoring format alongside it.

`mutsuki-plugin-bot-command` also builds as a Core ABI v2 `cdylib`. Builtin and ABI deployments
publish an equivalent `mutsuki.bot.flow.nodes@1` catalog and exact callable binding.

`mutsuki-bot-protocol` and `mutsuki-bot-sdk` are library crates and are not host-loadable plugins.

## Runtime Relationship

```text
MutsukiServiceHost / MutsukiCliHost / MutsukiTauriHost
  -> MutsukiCore
  -> MutsukiBotPlugins
```

Do not introduce `BotHost`. A standalone Bot service should run through `MutsukiServiceHost`.

All native runners implement the current MutsukiCore `Runner::run_batch` contract. A single task is represented as a one-entry `WorkBatch`; there is no separate scalar `step` execution path. Row payload tasks are mapped back to their matching `BatchEntry`, and each entry produces its own `EntryCompletion` inside a `CompletionBatch`.

## QQBot Production Bundle

`mutsuki-bot-service-host-integration::QqBotPluginBundle` assembles the QQBot manifest,
batch runners and the ServiceHost-managed Gateway EventSource. The adapter crate itself has no
ServiceHost dependency. The production HTTP transport uses
`reqwest` with the Rustls TLS backend; Gateway WebSocket uses
`tokio-tungstenite` with Rustls webpki roots. Product code installs the bundle
into `ServiceRuntimeBuilder`; it does not create a Bot-specific Host.
The configured factory is text-only. Media upload is declared only when product code explicitly
adds a real media provider; no unavailable production fallback is registered.
At source startup and reconnect, the adapter validates the configured account
through `/users/@me`, obtains `/gateway/bot`, and lets Gateway reject invalid or
disallowed intent/shard configurations as permanent structured failures.

See `docs/qqbot-adapter.md` and `examples/service-host-example` for configured ServiceHost
assembly, fake-server E2E and real-account smoke boundaries. `configured_bot_plugin_catalog()`
exports owner-defined config factories without moving QQ fields into ServiceHost.
Products that opt into Agent use `configured_bot_plugin_catalog_with_agent()` with the same shared
`AgentConnectionRegistry` passed to AgentKit's configured catalog. The active Bot Flow configuration is the
only place that decides whether a QQ event reaches Command, Agent or another behavior. The Agent
bridge owns connection/profile, session scope, media settings, concurrency, timeout and durable
session/delivery fencing; selecting the plugin alone does not route an event.
Issue #141's criterion-by-criterion functional and performance evidence is recorded in
`docs/issue141-acceptance.md`.

`examples/qqbot-echo` is only the deterministic product assembly. Its Echo
business runner lives in the separate `examples/bot-echo` crate and has no
QQBot, HTTP, WebSocket, or ServiceHost dependency.

## Boundary Rule

Business bot plugins should depend on `mutsuki.bot.*` protocols. They should not call QQBot APIs directly. QQBot-specific escape hatches must use `mutsuki.bot.qqbot.*` protocols and remain adapter-specific.

## Performance model

`mutsuki-bot-benchmarks` and `scripts/run-performance-model.py` implement the versioned Bot owner
workload. The current v2 suite measures command hit/miss, three-node Flow chains, explicit fan-out,
conversation/session binding,
active-delivery idempotency, and interaction state transitions to the existing deterministic
fixtures and loopback HTTP/WebSocket cases.

Run a local reference report with:

```text
python scripts/run-performance-model.py \
  --mode reference \
  --process-runs 3 \
  --output artifacts/performance/issue140-reference.json
```

The raw samples, unified report, anomaly analysis, workload boundary, and revision-lock procedure
are documented in `docs/performance-model-issue140.md`. The end-to-end functional boundary and
truthful QQ capability matrix are documented in `docs/qq-ai-pipeline-issue140.md`.
