//! Product-facing Bilibili configuration contract.
//!
//! The owner document persists the full [`BilibiliConfig`] under a hidden
//! `runtime_config` node so deployment-specific tuning keeps one authority,
//! while the visible fields expose only what a product operator needs to
//! enable the plugin and bind credentials through the Host secret store.

use mutsuki_config_service::{
    ConfigConstraints, ConfigDescriptor, ConfigExpr, ConfigKey, ConfigMutability, ConfigNode,
    ConfigPresentation, ConfigProviderId, ConfigScope, ConfigValue, ConfigValueType, LocalizedText,
    RestartPolicy, SecretState,
};

use crate::{BilibiliBackendConfig, BilibiliConfig, LinkResolverConfig, PLUGIN_ID, RetryConfig};

/// Provider document field holding the Web cookie credential value.
pub const BILIBILI_COOKIE_FIELD: &str = "cookie";
/// Host secret store key the Web cookie credential resolves through.
pub const BILIBILI_COOKIE_KEY: &str = "BILIBILI_COOKIE";

/// Deployment defaults for the product-owned document. Poll intervals stay
/// conservative because the polling sources are shared public endpoints.
impl Default for BilibiliConfig {
    fn default() -> Self {
        Self {
            backend: BilibiliBackendConfig::WebCookie {
                cookie_secret_key: BILIBILI_COOKIE_KEY.into(),
            },
            live_interval_ms: 30_000,
            dynamic_interval_ms: 60_000,
            video_interval_ms: 60_000,
            retry: RetryConfig {
                max_attempts: 3,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 30_000,
            },
            subscriptions: Vec::new(),
            link_resolver: LinkResolverConfig {
                enabled: false,
                cooldown_ms: 60_000,
                account_to_binding: std::collections::BTreeMap::new(),
            },
            media_provider_id: String::new(),
            risk_control: None,
            management: Default::default(),
        }
    }
}

/// Product-facing Bilibili configuration. Subscriptions, poll intervals and
/// retry tuning remain hidden but are persisted by the owner document so
/// deployment-specific values have one authority.
#[must_use]
pub fn bilibili_config_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(PLUGIN_ID),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("B 站"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("bilibili"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("B 站"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::PluginReload,
            children: vec![
                bool_node("enabled", "启用", Some("关闭后不会加载 B 站插件。")),
                secret_node(BILIBILI_COOKIE_FIELD, "登录 Cookie"),
                bool_node(
                    "management_enabled",
                    "Cookie 管理",
                    Some(
                        "启用后可通过管理页扫码登录并续期 Cookie，需要 Host security.secret_file。",
                    ),
                ),
                when_management(array_node(
                    "management_admin_user_ids",
                    "管理员用户 ID",
                    "允许使用 B 站管理指令的用户 ID。",
                )),
                when_management(string_node_with(
                    "management_self_binding_outbound_binding",
                    "自绑消息绑定",
                    "管理指令自绑通知使用的投递绑定名称。",
                )),
                hidden_object_node("runtime_config", "Bilibili Runtime Config"),
            ],
        },
        groups: Vec::new(),
    }
}

/// Projects a full [`BilibiliConfig`] into the product-owned document shape.
#[must_use]
pub fn bilibili_config_value(enabled: bool, config: &BilibiliConfig) -> ConfigValue {
    ConfigValue::Object(
        [
            ("enabled".into(), ConfigValue::Bool(enabled)),
            (
                "media_provider_id".into(),
                ConfigValue::String(config.media_provider_id.clone()),
            ),
            (
                BILIBILI_COOKIE_FIELD.into(),
                ConfigValue::Secret(SecretState::Keep),
            ),
            (
                "management_enabled".into(),
                ConfigValue::Bool(config.management.enabled),
            ),
            (
                "management_admin_user_ids".into(),
                ConfigValue::Array(
                    config
                        .management
                        .admin_user_ids
                        .iter()
                        .cloned()
                        .map(ConfigValue::String)
                        .collect(),
                ),
            ),
            (
                "management_self_binding_outbound_binding".into(),
                ConfigValue::String(config.management.self_binding_outbound_binding.clone()),
            ),
            (
                "runtime_config".into(),
                ConfigValue::from_json(
                    &serde_json::to_value(config).expect("Bilibili config serializes"),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

fn when_management(mut node: ConfigNode) -> ConfigNode {
    node.enabled_if = Some(ConfigExpr::Field {
        key: ConfigKey::new("management_enabled"),
    });
    node
}

fn hidden_object_node(key: &str, title: &str) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Object,
        ConfigConstraints::default(),
    );
    node.visibility = Some(ConfigExpr::Literal {
        value: ConfigValue::Bool(false),
    });
    node
}

fn bool_node(key: &str, title: &str, description: Option<&str>) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Bool,
        ConfigConstraints::default(),
    );
    node.description = description.map(LocalizedText::new);
    node
}

fn string_node(key: &str, title: &str) -> ConfigNode {
    field_node(
        key,
        title,
        ConfigValueType::String { multiline: false },
        ConfigConstraints::default(),
    )
}

fn string_node_with(key: &str, title: &str, description: &str) -> ConfigNode {
    let mut node = string_node(key, title);
    node.description = Some(LocalizedText::new(description));
    node
}

fn array_node(key: &str, title: &str, description: &str) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Array {
            item: Box::new(ConfigValueType::String { multiline: false }),
        },
        ConfigConstraints::default(),
    );
    node.description = Some(LocalizedText::new(description));
    node
}

fn secret_node(key: &str, title: &str) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Secret,
        ConfigConstraints {
            required: false,
            ..ConfigConstraints::default()
        },
    );
    node.presentation.secret = true;
    node
}

fn field_node(
    key: &str,
    title: &str,
    value_type: ConfigValueType,
    constraints: ConfigConstraints,
) -> ConfigNode {
    ConfigNode {
        key: ConfigKey::new(key),
        value_type,
        title: LocalizedText::new(title),
        description: None,
        default_value: None,
        constraints,
        presentation: ConfigPresentation::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy: RestartPolicy::PluginReload,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_exposes_enable_surface_over_hidden_runtime_config() {
        let descriptor = bilibili_config_descriptor();
        let keys = descriptor
            .root
            .children
            .iter()
            .map(|node| node.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "enabled",
                BILIBILI_COOKIE_FIELD,
                "management_enabled",
                "management_admin_user_ids",
                "management_self_binding_outbound_binding",
                "runtime_config",
            ]
        );
        assert_eq!(descriptor.provider_id.as_str(), PLUGIN_ID);
    }

    #[test]
    fn config_value_projects_management_switches() {
        let mut config = BilibiliConfig::default();
        config.media_provider_id = "memory".into();
        config.management.enabled = true;
        config.management.admin_user_ids = vec!["admin".into()];
        let value = bilibili_config_value(true, &config).to_json();
        assert_eq!(value["enabled"], true);
        assert_eq!(value["media_provider_id"], "memory");
        assert_eq!(value["management_enabled"], true);
        assert_eq!(value["management_admin_user_ids"][0], "admin");
        assert_eq!(
            value["runtime_config"]["backend"]["cookie_secret_key"],
            BILIBILI_COOKIE_KEY
        );
    }

    #[test]
    fn default_document_round_trips_through_runtime_config() {
        let value = bilibili_config_value(false, &BilibiliConfig::default());
        let restored: BilibiliConfig =
            serde_json::from_value(value.to_json()["runtime_config"].clone()).unwrap();
        assert_eq!(restored, BilibiliConfig::default());
    }
}
