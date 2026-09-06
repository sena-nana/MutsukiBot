use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use mutsuki_agent_service_host_integration::LOCAL_AGENT_CONFIG_PROVIDER_ID;
use mutsuki_bot_service_host_integration::{
    AgentConnectionConsoleBridge, BilibiliConsoleBridge, BotDatabaseConsoleBridge,
    BotFlowConsoleBridge, LocalAgentConsoleBridge, QqConsoleBridge, SandboxConsoleBridge,
};
use mutsuki_bot_web_console::{
    BotAgentConsoleServices, ConfigNavigationGroup, ConfigNavigationItem, ConsoleAssetDirs,
    ControlChangeBridge, ManagementChangeBridge, SecretKeyResolver, SecretMonitor,
    WebConsoleConfig, WebConsolePaths, WebConsoleSecrets, attach_control_changed_bridge,
    attach_management_changed_bridges, attach_revision_changed_bridge,
    build_console_host_with_agent,
};
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
use mutsuki_plugin_bot_agent::BOT_AGENT_BRIDGE_PLUGIN_ID;
use mutsuki_plugin_bot_bilibili::PLUGIN_ID as BILIBILI_PLUGIN_ID;
use mutsuki_plugin_bot_bilibili_workshop::PLUGIN_ID as WORKSHOP_PLUGIN_ID;
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_runtime::ServiceRuntime;
use mutsuki_web_host::{MutsukiWebHost, WebHost, WebHostResult};

use crate::{LocalConsoleConfig, PRODUCT_CONFIG_PROVIDER_ID};

#[derive(Debug, thiserror::Error)]
pub enum WebConsoleError {
    #[error("{code}: {message}")]
    Config { code: &'static str, message: String },
    #[error(transparent)]
    WebHost(#[from] mutsuki_web_host::WebHostError),
}

/// Keeps the embedded Web Console alive for the ServiceRuntime lifetime.
pub struct WebConsoleGuard {
    host: MutsukiWebHost,
    _config_watch: mutsuki_config_service::ConfigWatchSubscription,
    _control_changes: ControlChangeBridge,
    _management_changes: ManagementChangeBridge,
    _assets: ConsoleAssetDirs,
}

impl WebConsoleGuard {
    pub async fn start(
        config: LocalConsoleConfig,
        product_root: &Path,
        service: &ServiceConfig,
        runtime: &ServiceRuntime,
        config_service: Arc<mutsuki_config_service::ConfigService>,
    ) -> Result<Option<Self>, WebConsoleError> {
        if !config.enabled {
            return Ok(None);
        }
        let workspace_enabled = config.extensions.iter().any(|extension| {
            matches!(
                extension.as_str(),
                "qq" | "agent" | "bot-flow-editor" | "sandbox"
            )
        });
        let mut config_provider_ids = vec![PRODUCT_CONFIG_PROVIDER_ID.into()];
        if workspace_enabled {
            config_provider_ids.extend([
                QQBOT_ADAPTER_PLUGIN_ID.into(),
                LOCAL_AGENT_CONFIG_PROVIDER_ID.into(),
                BOT_AGENT_BRIDGE_PLUGIN_ID.into(),
                BILIBILI_PLUGIN_ID.into(),
                WORKSHOP_PLUGIN_ID.into(),
                MIHUASHI_PLUGIN_ID.into(),
            ]);
        }
        let config = WebConsoleConfig {
            enabled: config.enabled,
            listen: config.listen,
            auth_token_key: config.auth_token_key,
            extensions: config.extensions,
            config_provider_ids,
            primary_config_provider_id: Some(PRODUCT_CONFIG_PROVIDER_ID.into()),
            config_navigation_groups: config_navigation_groups(),
            release_set: config.release_set,
        };
        let secrets = resolve_secrets(service, &config)?;
        let secret_monitor = build_secret_monitor(service, &config);
        let bilibili = BilibiliConsoleBridge::get(runtime);
        let qq = QqConsoleBridge::get(runtime);
        let sandbox = SandboxConsoleBridge::get(runtime);
        let (host, assets) = build_console_host_with_agent(
            &config,
            &secrets,
            runtime.control_handler(),
            runtime.control_token(),
            Some(config_service.clone()),
            secret_monitor,
            &WebConsolePaths::resolve(product_root, &config),
            bilibili.clone(),
            qq.clone(),
            sandbox.clone(),
            BotDatabaseConsoleBridge::get(runtime),
            BotAgentConsoleServices {
                connections: AgentConnectionConsoleBridge::get(runtime),
                sessions: LocalAgentConsoleBridge::get(runtime),
                flow: BotFlowConsoleBridge::get(runtime),
            },
        )?;
        let mut host = host;
        host.start().await?;
        let config_watch =
            attach_revision_changed_bridge(&host, &config_service).ok_or_else(|| {
                WebConsoleError::Config {
                    code: "web.console.bridge_unavailable",
                    message: "started Web Console has no event bridge".into(),
                }
            })?;
        let control_changes =
            attach_control_changed_bridge(&host, runtime.subscribe_control_changes()).ok_or_else(
                || WebConsoleError::Config {
                    code: "web.console.bridge_unavailable",
                    message: "started Web Console has no event bridge".into(),
                },
            )?;
        let management_changes = attach_management_changed_bridges(
            &host,
            qq.as_ref(),
            bilibili.as_ref(),
            sandbox.as_ref(),
        )
        .ok_or_else(|| WebConsoleError::Config {
            code: "web.console.bridge_unavailable",
            message: "started Web Console has no event bridge".into(),
        })?;
        Ok(Some(Self {
            host,
            _config_watch: config_watch,
            _control_changes: control_changes,
            _management_changes: management_changes,
            _assets: assets,
        }))
    }

    pub fn listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.host.listen_addr()
    }

    pub async fn stop(mut self) -> WebHostResult<()> {
        self.host.stop().await
    }
}

fn resolve_secrets(
    service: &ServiceConfig,
    config: &WebConsoleConfig,
) -> Result<WebConsoleSecrets, WebConsoleError> {
    let key = config
        .auth_token_key
        .as_deref()
        .ok_or_else(|| WebConsoleError::Config {
            code: "web.console.auth_token_key_required",
            message: "enabled web console requires web.console.auth_token_key".into(),
        })?;
    let store = service.host_secret_store();
    let auth_token = store.resolve(key).ok_or_else(|| WebConsoleError::Config {
        code: "web.console.auth_token_missing",
        message: format!("secret key `{key}` is not configured"),
    })?;
    if auth_token.is_empty() {
        return Err(WebConsoleError::Config {
            code: "web.console.auth_token_empty",
            message: format!("secret key `{key}` must not be empty"),
        });
    }
    Ok(WebConsoleSecrets { auth_token })
}

struct HostSecretResolver {
    store: mutsuki_service_config::HostSecretStore,
}

impl SecretKeyResolver for HostSecretResolver {
    fn resolve(&self, key: &str) -> Option<String> {
        self.store.resolve(key)
    }
}

fn build_secret_monitor(
    service: &ServiceConfig,
    config: &WebConsoleConfig,
) -> Option<SecretMonitor> {
    let mut keys = BTreeSet::new();
    if let Some(key) = &config.auth_token_key {
        keys.insert(key.clone());
    }
    if keys.is_empty() {
        return None;
    }
    let store = service.host_secret_store();
    Some(SecretMonitor::new(
        keys.into_iter().collect(),
        Arc::new(HostSecretResolver { store }),
    ))
}

fn config_navigation_groups() -> Vec<ConfigNavigationGroup> {
    // Formal display names for every plugin the product may load. The 接入
    // entries keep owner config providers, so they render even before their
    // plugin is enabled; web-extension pages appear once their plugin loads.
    let item = |provider_id: &str, label: &str| ConfigNavigationItem {
        provider_id: provider_id.into(),
        label: Some(label.into()),
    };
    vec![
        ConfigNavigationGroup {
            label: None,
            items: vec![item(PRODUCT_CONFIG_PROVIDER_ID, "工作区")],
        },
        ConfigNavigationGroup {
            label: Some("接入".into()),
            items: vec![
                item(QQBOT_ADAPTER_PLUGIN_ID, "QQ 登录"),
                item("mutsuki.bot.mihuashi", "米画师"),
                item("mutsuki.bot.bilibili", "B 站"),
                item("mutsuki.bot.bilibili.workshop", "B 站工房"),
            ],
        },
        ConfigNavigationGroup {
            label: Some("助手".into()),
            items: vec![
                item(LOCAL_AGENT_CONFIG_PROVIDER_ID, "模型"),
                item("mutsuki.agent.connections", "助手连接"),
                item(BOT_AGENT_BRIDGE_PLUGIN_ID, "回复"),
                item("mutsuki.plugin.bot.persona", "人设"),
                item("mutsuki.plugin.bot.conversation.context", "会话上下文"),
                item("mutsuki.plugin.bot.interaction", "互动"),
                item("mutsuki.plugin.bot.media", "媒体"),
                item("mutsuki.bot.command", "指令"),
                item("mutsuki.bot.router.flow", "流程路由"),
                item("mutsuki.bot.flow", "流程编排"),
            ],
        },
        ConfigNavigationGroup {
            label: Some("投递".into()),
            items: vec![
                item("mutsuki.plugin.bot.reply", "自动回复"),
                item("mutsuki.plugin.bot.delivery", "消息投递"),
                item("mutsuki.plugin.bot.delivery.reply", "回复投递"),
                item("mutsuki.plugin.bot.delivery.scheduled", "定时投递"),
            ],
        },
        ConfigNavigationGroup {
            label: Some("服务".into()),
            items: vec![
                item(crate::SERVICE_CONFIG_PROVIDER_ID, "服务运行时"),
                item("mutsuki.std.image.render", "图像渲染"),
            ],
        },
        ConfigNavigationGroup {
            label: Some("扩展".into()),
            items: vec![
                item("overview", "总览"),
                item("qq-bot", "QQ 机器人"),
                item("bot-agent", "连接管理"),
                item("bilibili", "B 站管理"),
                item("bot-flow-editor", "流程编辑器"),
                item("control", "运行控制"),
                item("database", "数据库"),
                item("sandbox", "沙盒"),
                item("secret", "密钥"),
                item("upgrade", "升级"),
            ],
        },
    ]
}
