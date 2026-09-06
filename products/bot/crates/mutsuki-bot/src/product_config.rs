//! Product configuration and owner-provider assembly for the local Bot application.

use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AGENT_CONNECTIONS_PLUGIN_ID, AgentConnectionsConfig, LOCAL_AGENT_API_KEY,
    LOCAL_AGENT_API_KEY_FIELD, LOCAL_AGENT_CONFIG_PROVIDER_ID, LOCAL_AGENT_PLUGIN_ID,
    LocalAgentConfig, local_agent_config_descriptor, local_agent_config_value,
};
use mutsuki_bot_service_host_integration::{
    SANDBOX_SERVICE_ID, link_card_config_descriptor, link_card_config_value,
};
use mutsuki_config_service::{
    ConfigApplyMode, ConfigConstraints, ConfigContext, ConfigDescriptor, ConfigError, ConfigExpr,
    ConfigLifecycle, ConfigMutability, ConfigNode, ConfigPersistSink, ConfigPersistTransaction,
    ConfigPresentation, ConfigProviderId, ConfigProviderRegistry, ConfigRepository, ConfigScope,
    ConfigSecretMutation, ConfigService, ConfigValue, ConfigValueType, LocalizedText,
    MapKeyStrategy, MemoryConfigProvider, RestartPolicy, capability,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QQ_CLIENT_SECRET_FIELD, QQ_CLIENT_SECRET_KEY, QQ_RECEIVE_GUILD_FIELD,
    QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD, QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig,
    apply_receive_intents, qq_config_descriptor, qq_config_value, receive_guild,
    receive_private_and_group,
};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_BRIDGE_PLUGIN_ID, BotAgentConfig, bot_agent_config_schema,
};
use mutsuki_plugin_bot_bilibili::{
    BILIBILI_APP_SECRET_FIELD, BILIBILI_APP_SECRET_KEY, BILIBILI_COOKIE_FIELD, BILIBILI_COOKIE_KEY,
    BILIBILI_OAUTH_CREDENTIAL_FIELD, BILIBILI_OAUTH_CREDENTIAL_KEY, BilibiliBackendConfig,
    BilibiliConfig, PLUGIN_ID as BILIBILI_PLUGIN_ID, bilibili_config_descriptor,
    bilibili_config_value,
};
use mutsuki_plugin_bot_bilibili_workshop::PLUGIN_ID as WORKSHOP_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::BOT_FLOW_ROUTER_PLUGIN_ID;
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;
use mutsuki_service_config::{
    ConfiguredPluginSelection, HostSecretStore, PreparedHostSecretTransaction,
};

pub const PRODUCT_CONFIG_PROVIDER_ID: &str = "mutsuki.product";

/// Resource provider the product binds to every media-consuming plugin. The
/// binding is owned by this assembly; owner documents no longer carry it.
pub const MEDIA_RESOURCE_PROVIDER_ID: &str = mutsuki_plugin_resource_sqlite::PROVIDER_ID;

#[derive(Debug, thiserror::Error)]
pub enum ProductConfigError {
    #[error("product config provider registration failed: {0}")]
    Register(String),
}

struct HostSecretPersist {
    store: HostSecretStore,
    key_by_field: BTreeMap<String, String>,
}

struct PreparedHostSecretPersist(PreparedHostSecretTransaction);

impl ConfigPersistTransaction for PreparedHostSecretPersist {
    fn activate(&mut self) -> Result<(), ConfigError> {
        self.0.activate().map_err(persist_error)
    }

    fn commit_marker(&self) -> Option<&std::path::Path> {
        self.0.commit_marker()
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        self.0.commit().map_err(persist_error)
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        self.0.finish().map_err(persist_error)
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        self.0.rollback().map_err(persist_error)
    }
}

impl ConfigPersistSink for HostSecretPersist {
    fn prepare(
        &self,
        _persisted_value: &ConfigValue,
        secret_mutations: BTreeMap<String, ConfigSecretMutation>,
    ) -> Result<Box<dyn ConfigPersistTransaction>, ConfigError> {
        let updates = secret_mutations
            .into_iter()
            .filter_map(|(field, mutation)| {
                self.key_by_field.get(&field).cloned().map(|key| {
                    let value = match mutation {
                        ConfigSecretMutation::Set(value) => Some(value),
                        ConfigSecretMutation::Clear => None,
                    };
                    (key, value)
                })
            })
            .collect();
        self.store
            .prepare_mutations(updates)
            .and_then(|mut transaction| {
                transaction.enable_coordinated_commit()?;
                Ok(transaction)
            })
            .map(|transaction| {
                Box::new(PreparedHostSecretPersist(transaction))
                    as Box<dyn ConfigPersistTransaction>
            })
            .map_err(persist_error)
    }
}

pub struct ProductConfigOptions {
    pub repository: Arc<dyn ConfigRepository>,
    pub lifecycle: Option<Arc<dyn ConfigLifecycle>>,
}

impl ProductConfigOptions {
    #[must_use]
    pub fn new(repository: Arc<dyn ConfigRepository>) -> Self {
        Self {
            repository,
            lifecycle: None,
        }
    }

    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn ConfigLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }
}

pub fn product_config_service(
    repository: Arc<dyn ConfigRepository>,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    product_config_service_with_options(ProductConfigOptions::new(repository))
}

pub fn product_config_service_with_options(
    options: ProductConfigOptions,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            product_descriptor(),
            product_seed_defaults(),
            ConfigApplyMode::HotReload,
        )))
        .map_err(register_error)?;
    let service = ConfigService::new(registry, options.repository)
        .map_err(|error| ProductConfigError::Register(error.to_string()))?;
    if let Some(lifecycle) = options.lifecycle {
        service.set_lifecycle(lifecycle);
    }
    Ok(Arc::new(service))
}

pub fn product_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(PRODUCT_CONFIG_PROVIDER_ID),
        schema_version: 3,
        value_version: 3,
        title: LocalizedText::new("工作区"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: "product".into(),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("工作区"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children: vec![
                bool_node(
                    "workspace_enabled",
                    "启用",
                    Some("关闭后，助手连接和流程路由会一并停用。"),
                    RestartPolicy::ApplicationRestart,
                ),
                hidden(ConfigNode {
                    key: "runtime_plugins".into(),
                    value_type: ConfigValueType::Map {
                        key_strategy: MapKeyStrategy::FreeString,
                        value: Box::new(ConfigValueType::Object),
                    },
                    title: LocalizedText::new("运行插件"),
                    description: None,
                    default_value: None,
                    constraints: ConfigConstraints::default(),
                    presentation: ConfigPresentation::default(),
                    visibility: None,
                    enabled_if: None,
                    mutability: ConfigMutability::ReadWrite,
                    restart_policy: RestartPolicy::PluginReload,
                    children: Vec::new(),
                }),
            ],
        },
        groups: Vec::new(),
    }
}

fn bool_node(
    key: &str,
    title: &str,
    description: Option<&str>,
    restart_policy: RestartPolicy,
) -> ConfigNode {
    ConfigNode {
        key: key.into(),
        value_type: ConfigValueType::Bool,
        title: LocalizedText::new(title),
        description: description.map(LocalizedText::new),
        default_value: None,
        constraints: ConfigConstraints::default(),
        presentation: ConfigPresentation::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy,
        children: Vec::new(),
    }
}

fn hidden(mut node: ConfigNode) -> ConfigNode {
    node.visibility = Some(ConfigExpr::Literal {
        value: ConfigValue::Bool(false),
    });
    node
}

pub fn product_seed_defaults() -> ConfigValue {
    ConfigValue::Object(BTreeMap::from([
        ("workspace_enabled".into(), ConfigValue::Bool(true)),
        (
            "runtime_plugins".into(),
            ConfigValue::Object(BTreeMap::new()),
        ),
    ]))
}

/// Builds an owner provider whose secret fields resolve through the Host
/// secret store under the given `(document field, store key)` pairs.
fn secret_backed_provider(
    descriptor: ConfigDescriptor,
    value: ConfigValue,
    secrets: &HostSecretStore,
    secret_keys: &[(&str, &str)],
) -> MemoryConfigProvider {
    let mut provider = MemoryConfigProvider::new(descriptor, value, ConfigApplyMode::HotReload)
        .with_persist(Arc::new(HostSecretPersist {
            store: secrets.clone(),
            key_by_field: secret_keys
                .iter()
                .map(|(field, key)| ((*field).to_owned(), (*key).to_owned()))
                .collect(),
        }));
    for (field, key) in secret_keys {
        if secrets.resolve(key).is_some() {
            provider = provider.with_initial_secret(*field, "configured".into());
        }
    }
    provider
}

/// Registers the product-facing owner providers after Host secrets have been loaded.
pub(crate) async fn register_configured_product_providers(
    service: &Arc<ConfigService>,
    secrets: HostSecretStore,
) -> Result<(), ProductConfigError> {
    let qq_config = QqBotConfig::default();
    let qq_provider = secret_backed_provider(
        qq_config_descriptor(QQBOT_ADAPTER_PLUGIN_ID),
        qq_config_value(false, &qq_config),
        &secrets,
        &[(QQ_CLIENT_SECRET_FIELD, QQ_CLIENT_SECRET_KEY)],
    );
    service
        .registry()
        .register(Arc::new(qq_provider))
        .map_err(register_error)?;

    let local_config = LocalAgentConfig::default();
    let local_provider = secret_backed_provider(
        local_agent_config_descriptor(),
        local_agent_config_value(false, &local_config),
        &secrets,
        &[(LOCAL_AGENT_API_KEY_FIELD, LOCAL_AGENT_API_KEY)],
    );
    service
        .registry()
        .register(Arc::new(local_provider))
        .map_err(register_error)?;

    let bridge_config = BotAgentConfig::default();
    service
        .registry()
        .register(Arc::new(MemoryConfigProvider::new(
            bot_agent_config_schema(),
            ConfigValue::from_json(
                &serde_json::to_value(bridge_config).expect("Bot Agent config serializes"),
            ),
            ConfigApplyMode::HotReload,
        )))
        .map_err(register_error)?;

    let bilibili_provider = secret_backed_provider(
        bilibili_config_descriptor(),
        bilibili_config_value(false, &BilibiliConfig::default()),
        &secrets,
        &[
            (BILIBILI_COOKIE_FIELD, BILIBILI_COOKIE_KEY),
            (BILIBILI_APP_SECRET_FIELD, BILIBILI_APP_SECRET_KEY),
            (
                BILIBILI_OAUTH_CREDENTIAL_FIELD,
                BILIBILI_OAUTH_CREDENTIAL_KEY,
            ),
        ],
    );
    service
        .registry()
        .register(Arc::new(bilibili_provider))
        .map_err(register_error)?;
    for (plugin_id, title) in [
        (WORKSHOP_PLUGIN_ID, "B 站工房"),
        (MIHUASHI_PLUGIN_ID, "米画师"),
    ] {
        service
            .registry()
            .register(Arc::new(MemoryConfigProvider::new(
                link_card_config_descriptor(plugin_id, title),
                link_card_config_value(false),
                ConfigApplyMode::HotReload,
            )))
            .map_err(register_error)?;
    }

    for provider_id in [
        QQBOT_ADAPTER_PLUGIN_ID,
        LOCAL_AGENT_CONFIG_PROVIDER_ID,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        BILIBILI_PLUGIN_ID,
        WORKSHOP_PLUGIN_ID,
        MIHUASHI_PLUGIN_ID,
    ] {
        service
            .restore(provider_id, provider_context(provider_id))
            .await
            .map_err(register_error)?;
    }
    Ok(())
}

/// Resolves the owner documents that are authoritative for configurable product plugins.
pub(crate) async fn configured_product_owner_selections(
    service: &Arc<ConfigService>,
) -> Result<Vec<ConfiguredPluginSelection>, ProductConfigError> {
    let mut selections = Vec::new();
    for provider_id in [
        QQBOT_ADAPTER_PLUGIN_ID,
        LOCAL_AGENT_CONFIG_PROVIDER_ID,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        BILIBILI_PLUGIN_ID,
        WORKSHOP_PLUGIN_ID,
        MIHUASHI_PLUGIN_ID,
    ] {
        let snapshot = service
            .read(
                provider_id,
                provider_context(provider_id),
                &[capability::VALUE_READ.into()],
            )
            .await
            .map_err(register_error)?;
        let restored = configured_plugin_selection_from_value(provider_id, &snapshot.value, None)
            .map_err(register_error)?;
        selections.push(restored);
    }
    Ok(selections)
}

pub(crate) fn configured_product_selections(
    product: &serde_json::Value,
    owner_selections: Vec<ConfiguredPluginSelection>,
) -> Result<Vec<ConfiguredPluginSelection>, ConfigError> {
    let workspace_enabled = product
        .get("workspace_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut selections = runtime_plugin_selections(product)?;
    selections.extend([
        ConfiguredPluginSelection {
            id: AGENT_CONNECTIONS_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::to_value(AgentConnectionsConfig::default())
                .expect("Agent connection defaults serialize"),
        },
        ConfiguredPluginSelection {
            id: BOT_FLOW_ROUTER_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::json!({}),
        },
        ConfiguredPluginSelection {
            id: SANDBOX_SERVICE_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::json!({}),
        },
    ]);
    selections.extend(owner_selections);
    // The local engine rides on the workspace-provided Agent connections and
    // flow router, so it cannot outlive `workspace_enabled` even when its own
    // owner document says enabled.
    if !workspace_enabled {
        for selection in &mut selections {
            if selection.id == LOCAL_AGENT_PLUGIN_ID {
                selection.enabled = false;
            }
        }
    }
    Ok(selections)
}

pub(crate) fn runtime_plugin_selections(
    product: &serde_json::Value,
) -> Result<Vec<ConfiguredPluginSelection>, ConfigError> {
    product
        .get("runtime_plugins")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .map(|(id, value)| {
            if is_product_owner_plugin(id) {
                return Err(ConfigError::ApplyRejected {
                    reason: format!("`runtime_plugins` 不得配置 owner 插件 `{id}`"),
                });
            }
            Ok(ConfiguredPluginSelection {
                id: id.clone(),
                enabled: value
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                config: value
                    .get("config")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            })
        })
        .collect()
}

pub(crate) fn is_product_owner_plugin(id: &str) -> bool {
    matches!(
        id,
        AGENT_CONNECTIONS_PLUGIN_ID
            | BOT_FLOW_ROUTER_PLUGIN_ID
            | SANDBOX_SERVICE_ID
            | QQBOT_ADAPTER_PLUGIN_ID
            | LOCAL_AGENT_PLUGIN_ID
            | BOT_AGENT_BRIDGE_PLUGIN_ID
            | BILIBILI_PLUGIN_ID
            | WORKSHOP_PLUGIN_ID
            | MIHUASHI_PLUGIN_ID
    )
}

pub(crate) fn configured_plugin_selection_from_value(
    provider_id: &str,
    value: &ConfigValue,
    base: Option<&ConfiguredPluginSelection>,
) -> Result<ConfiguredPluginSelection, ConfigError> {
    let json = value.to_json();
    let object = json.as_object().ok_or_else(|| ConfigError::ApplyRejected {
        reason: "配置必须是对象".into(),
    })?;
    match provider_id {
        AGENT_CONNECTIONS_PLUGIN_ID => {
            let config: AgentConnectionsConfig =
                serde_json::from_value(json).map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            config
                .validate()
                .map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled: base.is_some_and(|selection| selection.enabled),
                config: serde_json::to_value(config).expect("Agent connections serialize"),
            })
        }
        QQBOT_ADAPTER_PLUGIN_ID => {
            let mut config: QqBotConfig = object
                .get("runtime_config")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .or_else(|| {
                    base.and_then(|selection| serde_json::from_value(selection.config.clone()).ok())
                })
                .unwrap_or_default();
            config.account_id = if config.account_id.trim().is_empty() {
                "local".into()
            } else {
                config.account_id
            };
            config.app_id = optional_string_field(object, "app_id");
            config.client_secret_key = QQ_CLIENT_SECRET_KEY.into();
            if let (Ok(index), Ok(count)) = (
                unsigned_field(object, "shard_index"),
                unsigned_field(object, "shard_count"),
            ) {
                config.shard = [index, count];
            }
            config.gateway_intents = qq_gateway_intents(object, config.gateway_intents)?;
            let enabled = bool_field(object, "enabled")?;
            if enabled {
                config
                    .validate()
                    .map_err(|error| ConfigError::ApplyRejected {
                        reason: error.to_string(),
                    })?;
            }
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("QQ config serializes"),
            })
        }
        LOCAL_AGENT_CONFIG_PROVIDER_ID => {
            let mut config: LocalAgentConfig = base
                .and_then(|selection| serde_json::from_value(selection.config.clone()).ok())
                .unwrap_or_default();
            config.endpoint = string_field(object, "endpoint")?;
            config.model = string_field(object, "model")?;
            config.api_key_key = LOCAL_AGENT_API_KEY.into();
            config.assistant_instruction = string_field(object, "assistant_instruction")?;
            let enabled = bool_field(object, "enabled")?;
            if enabled {
                config
                    .validate()
                    .map_err(|reason| ConfigError::ApplyRejected { reason })?;
            }
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("Local Agent config serializes"),
            })
        }
        BOT_AGENT_BRIDGE_PLUGIN_ID => {
            let config: BotAgentConfig =
                serde_json::from_value(json).map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            config
                .validate()
                .map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            let enabled = config.enabled;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("Bot Agent config serializes"),
            })
        }
        BILIBILI_PLUGIN_ID => {
            let enabled = bool_field(object, "enabled")?;
            let mut config: BilibiliConfig = object
                .get("runtime_config")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .or_else(|| {
                    base.and_then(|selection| serde_json::from_value(selection.config.clone()).ok())
                })
                .unwrap_or_default();
            // The product assembly owns the media resource provider binding;
            // owner documents no longer carry a provider id.
            config.media_provider_id = MEDIA_RESOURCE_PROVIDER_ID.into();
            if optional_string_field(object, "backend") == "open_platform" {
                config.backend = BilibiliBackendConfig::OpenPlatform {
                    client_id: optional_string_field(object, "client_id"),
                    app_secret_key: BILIBILI_APP_SECRET_KEY.into(),
                    oauth_credential_key: BILIBILI_OAUTH_CREDENTIAL_KEY.into(),
                    authorized_uid: object
                        .get("authorized_uid")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default(),
                };
                // The open platform backend cannot serve cookie management,
                // web link resolution or Chromium risk control; drop the stale
                // hidden values so validation matches the visible contract.
                config.management.enabled = false;
                config.risk_control = None;
                config.link_resolver.enabled = false;
            } else {
                config.backend = BilibiliBackendConfig::WebCookie {
                    cookie_secret_key: BILIBILI_COOKIE_KEY.into(),
                };
            }
            config.management.enabled = optional_bool_field(object, "management_enabled")
                .unwrap_or_default()
                && matches!(config.backend, BilibiliBackendConfig::WebCookie { .. });
            if let Some(admin_user_ids) = object
                .get("management_admin_user_ids")
                .and_then(serde_json::Value::as_array)
            {
                config.management.admin_user_ids = admin_user_ids
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
            if let Some(binding) = object.get("management_self_binding_outbound_binding") {
                config.management.self_binding_outbound_binding = binding
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| config.management.self_binding_outbound_binding.clone());
            }
            if enabled {
                config
                    .validate()
                    .map_err(|reason| ConfigError::ApplyRejected { reason })?;
            }
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("Bilibili config serializes"),
            })
        }
        WORKSHOP_PLUGIN_ID | MIHUASHI_PLUGIN_ID => {
            let enabled = bool_field(object, "enabled")?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::json!({ "enabled": enabled }),
            })
        }
        _ => Err(ConfigError::ApplyRejected {
            reason: format!("未知产品配置 `{provider_id}`"),
        }),
    }
}

fn qq_gateway_intents(
    object: &serde_json::Map<String, serde_json::Value>,
    current: u64,
) -> Result<u64, ConfigError> {
    let private = optional_bool_field(object, QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD);
    let guild = optional_bool_field(object, QQ_RECEIVE_GUILD_FIELD);
    if private.is_some() || guild.is_some() {
        return Ok(apply_receive_intents(
            current,
            private.unwrap_or_else(|| receive_private_and_group(current)),
            guild.unwrap_or_else(|| receive_guild(current)),
        ));
    }
    if object.contains_key("gateway_intents") {
        return unsigned_field(object, "gateway_intents");
    }
    Ok(current)
}

fn optional_bool_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<bool> {
    object.get(field).and_then(serde_json::Value::as_bool)
}

fn optional_string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> String {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn bool_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<bool, ConfigError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("`{field}` 必须是布尔值"),
        })
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, ConfigError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("`{field}` 必须是字符串"),
        })
}

fn unsigned_field<T>(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<T, ConfigError>
where
    T: TryFrom<u64>,
{
    object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| T::try_from(value).ok())
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("`{field}` 超出允许范围"),
        })
}

fn persist_error(error: mutsuki_service_config::ConfigError) -> ConfigError {
    ConfigError::ApplyRejected {
        reason: error.to_string(),
    }
}

fn provider_context(provider_id: &str) -> ConfigContext {
    if provider_id == BOT_AGENT_BRIDGE_PLUGIN_ID {
        ConfigContext::plugin_instance("default")
    } else {
        ConfigContext::global()
    }
}

fn register_error(error: ConfigError) -> ProductConfigError {
    ProductConfigError::Register(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_plugin_bot_adapter_qqbot::{
        DEFAULT_QQBOT_INTENTS, QQ_INTENT_GROUP_AND_C2C, QQ_INTENT_PUBLIC_GUILD,
    };

    fn select(value: ConfigValue) -> ConfiguredPluginSelection {
        configured_plugin_selection_from_value(QQBOT_ADAPTER_PLUGIN_ID, &value, None).unwrap()
    }

    #[test]
    fn disabled_login_allows_empty_app_id() {
        let selected = select(qq_config_value(false, &QqBotConfig::default()));
        assert!(!selected.enabled);
        let config: QqBotConfig = serde_json::from_value(selected.config).unwrap();
        assert!(config.app_id.is_empty());
    }

    #[test]
    fn enabled_login_requires_app_id() {
        let error = configured_plugin_selection_from_value(
            QQBOT_ADAPTER_PLUGIN_ID,
            &qq_config_value(true, &QqBotConfig::default()),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("app_id"));
    }

    #[test]
    fn receive_switches_update_only_known_intent_bits() {
        let extra = 1 << 1;
        let mut config = QqBotConfig::new("local", "app");
        config.gateway_intents = DEFAULT_QQBOT_INTENTS | extra;
        let ConfigValue::Object(mut object) = qq_config_value(true, &config) else {
            panic!("QQ login document must be an object");
        };
        object.insert(QQ_RECEIVE_GUILD_FIELD.into(), ConfigValue::Bool(false));
        let selected = select(ConfigValue::Object(object));
        let restored: QqBotConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(
            restored.gateway_intents & QQ_INTENT_PUBLIC_GUILD,
            0,
            "guild receive switch must clear only the guild bit"
        );
        assert_eq!(restored.gateway_intents & extra, extra);
        assert_eq!(
            restored.gateway_intents & QQ_INTENT_GROUP_AND_C2C,
            QQ_INTENT_GROUP_AND_C2C
        );
    }

    #[test]
    fn legacy_gateway_intents_are_still_accepted() {
        let mut config = QqBotConfig::new("local", "app");
        config.gateway_intents = QQ_INTENT_GROUP_AND_C2C;
        let value = ConfigValue::Object(
            [
                ("enabled".into(), ConfigValue::Bool(true)),
                ("app_id".into(), ConfigValue::String("app".into())),
                (
                    "gateway_intents".into(),
                    ConfigValue::Integer(i64::try_from(config.gateway_intents).unwrap()),
                ),
                (
                    "runtime_config".into(),
                    ConfigValue::from_json(&serde_json::to_value(&config).unwrap()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let selected = select(value);
        let restored: QqBotConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(restored.gateway_intents, QQ_INTENT_GROUP_AND_C2C);
    }

    fn select_plugin(
        provider_id: &str,
        value: ConfigValue,
    ) -> Result<ConfiguredPluginSelection, ConfigError> {
        configured_plugin_selection_from_value(provider_id, &value, None)
    }

    #[test]
    fn bilibili_owner_document_stays_disabled_until_configured() {
        let selected = select_plugin(
            BILIBILI_PLUGIN_ID,
            bilibili_config_value(false, &BilibiliConfig::default()),
        )
        .unwrap();
        assert!(!selected.enabled);
        let config: BilibiliConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(
            config.backend,
            BilibiliBackendConfig::WebCookie {
                cookie_secret_key: BILIBILI_COOKIE_KEY.into(),
            }
        );
    }

    #[test]
    fn enabled_bilibili_binds_product_media_provider_and_secret_keys() {
        let mut config = BilibiliConfig::default();
        config.management.enabled = true;
        config.management.admin_user_ids = vec!["admin".into()];
        config.management.self_binding_outbound_binding = "qq-main".into();
        let selected =
            select_plugin(BILIBILI_PLUGIN_ID, bilibili_config_value(true, &config)).unwrap();
        assert!(selected.enabled);
        let restored: BilibiliConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(restored.media_provider_id, MEDIA_RESOURCE_PROVIDER_ID);
        assert!(restored.management.enabled);
        assert_eq!(restored.management.admin_user_ids, vec!["admin".to_owned()]);
        assert_eq!(restored.management.self_binding_outbound_binding, "qq-main");
        assert_eq!(
            restored.backend.cookie_secret_key(),
            Some(BILIBILI_COOKIE_KEY)
        );
    }

    #[test]
    fn open_platform_switch_forces_cookie_only_surfaces_off() {
        let mut config = BilibiliConfig::default();
        config.management.enabled = true;
        config.link_resolver.enabled = true;
        let value = bilibili_config_value(true, &config).to_json();
        let value = serde_json::json!({
            "enabled": true,
            "backend": "open_platform",
            "client_id": "client",
            "app_secret": "configured",
            "oauth_credential": "configured",
            "authorized_uid": 42,
            "management_enabled": true,
            "runtime_config": value["runtime_config"],
        });
        let selected = select_plugin(BILIBILI_PLUGIN_ID, ConfigValue::from_json(&value)).unwrap();
        let restored: BilibiliConfig = serde_json::from_value(selected.config).unwrap();
        match restored.backend {
            BilibiliBackendConfig::OpenPlatform {
                client_id,
                app_secret_key,
                oauth_credential_key,
                authorized_uid,
            } => {
                assert_eq!(client_id, "client");
                assert_eq!(app_secret_key, BILIBILI_APP_SECRET_KEY);
                assert_eq!(oauth_credential_key, BILIBILI_OAUTH_CREDENTIAL_KEY);
                assert_eq!(authorized_uid, 42);
            }
            other => panic!("expected open platform backend, got {other:?}"),
        }
        assert!(!restored.management.enabled);
        assert!(!restored.link_resolver.enabled);
        assert!(restored.risk_control.is_none());
    }

    #[test]
    fn link_card_owner_documents_only_carry_the_enable_switch() {
        for provider_id in [WORKSHOP_PLUGIN_ID, MIHUASHI_PLUGIN_ID] {
            let disabled = select_plugin(provider_id, link_card_config_value(false)).unwrap();
            assert!(!disabled.enabled);

            let enabled = select_plugin(provider_id, link_card_config_value(true)).unwrap();
            assert!(enabled.enabled);
            assert_eq!(enabled.config, serde_json::json!({ "enabled": true }));
        }
    }

    #[test]
    fn builtin_platform_plugins_are_rejected_from_runtime_plugins() {
        for plugin_id in [BILIBILI_PLUGIN_ID, WORKSHOP_PLUGIN_ID, MIHUASHI_PLUGIN_ID] {
            let product = serde_json::json!({
                "runtime_plugins": {
                    plugin_id: { "enabled": true, "config": {} },
                },
            });
            let error = runtime_plugin_selections(&product).unwrap_err();
            assert!(error.to_string().contains("不得配置 owner 插件"));
        }
    }
}
