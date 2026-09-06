---
name: bot-assembly
description: Assemble the first-party Mutsuki Bot product from external configuration through plugin selection, RuntimeProfile, RuntimeLoadPlan, ServiceRuntimeBuilder, EventSources, and secret references. Use for Bot startup, configuration, and product composition rather than owner capability implementation.
---

# Bot Assembly

将外部配置确定性地转换为可验证的 Bot 产品装配，产品入口只描述所需能力。

## 配置

- 产品固定使用可执行文件旁 `.mutsuki-bot` 单实例目录；Host identity、目录、secret 文件、插件发现和 SQLite repository 选择由产品内建。
- 生产入口不得接受配置路径、profile、namespace 或 `MUTSUKI_BOOTSTRAP`；旧 bootstrap 和完整产品 TOML 不读取、不迁移。
- Mutsuki Bot 产品显式选择 SQLite repository plugin、document namespace 和路径；Mutsuki 框架不得内置该选择。
- 产品插件选择、WebExtension 选择和每个 owner 配置由 `ConfigService` 保存到独立 provider document。
- 内置平台插件（B 站、B 站工房、米画师）与 QQ/Local Agent/Bot Agent 一样由产品 owner provider 驱动：
  启动时注册带 `enabled` 开关的字段级 schema（B 站凭据经 Host secret key 引用注入），默认禁用，
  从控制台插件页配置并热加载；`runtime_plugins` 拒绝这些 owner id。
- 主配置只保存 secret key；实际值由 Host 从显式引用且被忽略的专用 secret 文件或环境变量注入。
- 零插件配置允许启动为空闲 Runtime；未知字段和显式选择后缺失的 capability、plugin、deployment 或 secret 必须结构化失败。

## 装配

1. 打开产品固定的 SQLite 配置仓库，空仓库以 CAS 写一次工作区种子；已有文档不覆盖。
2. 将配置文档解析为 capability、plugin、deployment、binding 和 Host 资源需求；Bot 匹配与顺序只来自 active Flow provider snapshot。
3. 只聚合 owner 公开 factory catalog；产品不得注册自有业务 manifest 或 Runner。
4. 启动前生成并校验 RuntimeProfile/RuntimeLoadPlan；registry freeze 后不得越权注册。
5. 通过 `ServiceRuntimeBuilder` 或当前等价 API 启动真实 `ServiceRuntime`，不创建 BotHost。

QQBot、Agent 和 Provider 只是配置选择与验收场景，不得产生绕过 Bot protocol 或上游公开边界的专用路径。测试合法配置的 load plan，并验证缺失项 fail loud、health 只报告真实组件。
