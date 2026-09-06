# Bilibili account and subscription management

The Bilibili configured plugin can opt into chat management through its owner-defined
`management` config. `mutsuki-bot-management::BilibiliManagementApi` is the only public
management surface shared by chat and Web Console. The Bilibili plugin provides the concrete
implementation, while ServiceHost integration owns construction, Host secret/config injection,
and registration of the API object.

```text
Chat:
  Bot Flow: Source -> Command Match -> Bilibili management Processor
    -> BilibiliManagementApi -> plugin implementation -> Host secret / configured-plugin store

Web Console:
  overview /bilibili page -> bilibili.* RPC (runtime.read|write)
    -> BilibiliWebExtension -> same BilibiliManagementApi
```

The management contract contains only product fields:

- `enabled` publishes the management behavior node; its command path exists only in a Command
  Match node in the published graph.
- `admin_user_ids` authorizes QR login and administrator subscription changes **in chat**.
- `allow_self_binding` enables signature-challenge ownership verification.
- `self_binding_notifications` and `self_binding_outbound_binding` define the subscription created
  after successful verification.
- every subscription has a stable `subscription_id`, `uid`, notification kinds, target, outbound
  binding, `paused`, and optional `owner_user_id`.

Push delivery is a Flow concern: the polling runner submits a `mutsuki.bot.event.bilibili`
trigger event per fresh item and never sends a message itself. The active graph must wire
`mutsuki.bot.bilibili.notification` → `mutsuki.bot.bilibili.card` → a platform send node
(reference graph `bilibili_push_flow()`, one chain for live/dynamic/video kinds); with no
matching Source the event is dropped.
A subscription's `outbound_binding` remains required configuration and Web `subscribe` still
asks for it, but it is carried as event context only — the graph's send node binding decides
delivery. The `/bili preview` command path keeps its direct command-route reply and still never
advances the poll cursor.

Enabling management requires the service to be loaded from a real product config file and requires
Host `security.secret_file`. The product config stores only
`backend = { type = "web_cookie", cookie_secret_key = "..." }`; the ignored secret
file stores the value. Environment-backed secrets are intentionally read-only and cannot be
rotated by QR login.

Full Web Console and chat management require `backend.type = web_cookie` and
`management.enabled = true`.

## Commands

- `/bili login` and `/bili login-status`: administrator QR login. A PNG `ResourceRef` is sent to
  chat; confirmation atomically rotates the Host secret and updates the live credential reader.
- `/bili bind <uid>` and `/bili verify`: self-binding through a short signature challenge. Only a
  verified binding is written into the owner config.
- `/bili unbind`: removes the caller's verified self-binding from owner config.
- `/bili pause [subscription-or-uid]` and `/bili resume [...]`: persist the operational state in
  owner config. The polling EventSource reads the shared current config before every scheduling
  pass.
- `/bili preview [subscription-or-uid]`: fetches and sends the newest dynamic without changing the
  durable poll cursor.
- `/bili list`: lists subscriptions visible to the caller.
- `/bili subscribe <id> <uid> [live,dynamic,video]` and `/bili unsubscribe <id>`: administrator
  management for the current conversation.

## Web Console

When the Bilibili management service is registered, the embedded console injects the `bilibili`
WebExtension and the overview shows **B站推送**. Static enablement stays in the product file
and `security.secret_file`; it is not a Config-page form.

When the product assembly shares the Flow registry (`configured_bot_plugin_catalog_with_agent_and_flow`),
the `status` response carries `push_wired`: whether the `mutsuki.bot.bilibili.notification`
Source chain is wired into the active Bot Flow graph (`null` when no registry is shared).
While it is `false`, polling skips the upstream Bilibili API call entirely — pushes are frozen
and observable, and the first wired poll after a freeze baselines the cursor instead of
replaying the frozen window as a backlog.

Auth:

- Console holders of `WEB_CONSOLE_AUTH_TOKEN` with `runtime.read` / `runtime.write` act as
  administrators on the Web surface (chat `admin_user_ids` is not re-checked).
- Self-binding RPCs require an explicit `operator_user_id` so `owner_user_id` stays chat-compatible.
- Web `subscribe` requires explicit `target` and `outbound_binding` (chat still uses the current
  conversation target and `self_binding_outbound_binding`).
- Web `preview` returns card JSON only; it does not submit an outbound Bot message.
- Web `login.start` returns only `qr_png_base64`.
- Web `credential.clear` and `subscriptions.unsubscribe` require `confirmed: true`.
- Cookie secret values never enter RPC responses, logs, or frontend markup. QR confirmation
  returns a base64 PNG only.

RPC surface (`bilibili` namespace):

- read: `status`, `login.poll`, `subscriptions.list`, `subscriptions.preview`
- write: `login.start`, `credential.clear`, `subscriptions.subscribe`,
  `subscriptions.unsubscribe`, `subscriptions.set_paused`, `binding.start`, `binding.verify`,
  `binding.unbind`

Missing actor identity, authorization, challenge, Host store, secret backend, subscription, or
credential fails with a structured Bilibili runtime error. Cookie values never enter command
payloads, replies, manifests, traces, or ordinary logs.

## Validation levels

- Unit/batch tests use a fake Bilibili transport and real SQLite state to cover secret rotation,
  partial batch failure, signature verification, config persistence, pause, and cursor-free
  preview.
- ServiceHost tests cover atomic shared secret rotation, environment override rejection, and
  owner-only configured-plugin replacement.
- A real-account smoke must use an ignored local product config and secret file. It should verify
  QR confirmation, a signed self-binding, pause/resume, preview, normal polling, and clean Host
  shutdown. Unit or fake coverage must not be reported as a real-account smoke.
