# Bilibili 官方开放平台 backend

`mutsuki.bot.bilibili` 有两个互斥且显式的生产 backend：

- `web_cookie`：Cookie/WBI、动态轮询、链接解析、扫码管理和可选 Chromium 352 路径。
- `open_platform`：Bilibili 官方 OAuth2、v2 HMAC-SHA256 签名、授权账号直播状态与已发布稿件。

二者不会互相 fallback。`open_platform` 只复用现有 `mutsuki.bot.bilibili.poll/live@1` 与
`mutsuki.bot.bilibili.poll/video@1` 的 cursor、消息和 outbound binding 语义。官方接口
没有等价的任意 UID 动态查询，因此该 backend 不在 manifest/runner 中声明
`poll/dynamic` 或 `link/resolve`，也拒绝 management 和 `risk_control` 配置。

## 产品配置

```toml
backend = { type = "open_platform", client_id = "申请应用得到的 client_id", app_secret_key = "BILIBILI_OPEN_APP_SECRET", oauth_credential_key = "BILIBILI_OPEN_OAUTH", authorized_uid = 123456 }
live_interval_ms = 60000
dynamic_interval_ms = 120000
video_interval_ms = 300000
retry = { max_attempts = 3, initial_backoff_ms = 1000, max_backoff_ms = 30000 }
link_resolver = { enabled = false, cooldown_ms = 1000, account_to_binding = {} }
subscriptions = []
management = { enabled = false, allow_self_binding = false, admin_user_ids = [], self_binding_notifications = ["live", "video"], self_binding_outbound_binding = "" }
```

每条 subscription 的 `uid` 必须等于 `authorized_uid`，通知类型只能包含 `live` 和
`video`。选择 `live` 需要申请并由用户授权 `LIVE_ROOM_DATA`；选择 `video` 需要
`ARC_BASE`。scope 缺失、UID 不匹配、动态通知或 Web-only 配置都会结构化失败。

`security.secret_file` 必须可写，以便 refresh token 单次使用后原子替换 OAuth bundle。
Secret 文件只保存值，不进入产品配置：

```toml
[secrets]
BILIBILI_OPEN_APP_SECRET = "replace-locally"
BILIBILI_OPEN_OAUTH = '''{"access_token":"replace-locally","refresh_token":"replace-locally","expires_at":1893456000,"scopes":["LIVE_ROOM_DATA","ARC_BASE"]}'''
```

`expires_at` 是 UTC Unix 秒。bundle 用一个 Host secret key 保存 access token、refresh
token、过期时间和授权 scope，因此刷新不会留下跨 key 的半更新状态。普通 `Debug`、
RuntimeError 和请求 evidence 不包含 app secret、access token 或 refresh token。

## 官方契约映射

- OAuth code/token 与 refresh：`/x/account-oauth2/v1/token`、
  `/x/account-oauth2/v1/refresh_token`。
- 直播：`POST /arcopen/fn/live/room/detail`，scope `LIVE_ROOM_DATA`。
- 稿件：`GET /arcopen/fn/archive/viewlist?pn=1&ps=50&status=pubed`，scope `ARC_BASE`。
- v2 签名：对排序后的 `x-bili-*` headers 计算 HMAC-SHA256；access token 仅放在 header。

实现依据当前 [Bilibili 开放平台文档](https://open.bilibili.com/doc)。开放平台错误保持独立模型：

- OAuth 失效：`bilibili.open_platform.oauth_expired`，自动 refresh 后只重试一次。
- scope/账号/白名单：`bilibili.open_platform.permission_denied`。
- 签名、时间戳、nonce 或 MD5：`bilibili.open_platform.signature_rejected`。
- 其他官方业务错误：`bilibili.open_platform.api_failed`。

错误 evidence 只保留官方 `request_id`、非敏感 code 与 required scope，供工单排查。

## 验证层级

自动测试用 recording HTTP boundary 验证签名 headers、live/video 响应映射、过期 token
原子刷新、scope/签名错误分类、UID 限制以及 credential `Debug` 脱敏。真实账号 smoke
需要已入驻、已审核且完成用户授权的开放平台应用；未提供这些本地凭据时不得用 fake
结果宣称真实开放平台可用。
