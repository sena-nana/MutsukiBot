use mutsuki_agent_client::{AgentClient, AgentClientBackend};
use mutsuki_agent_contracts::{
    AgentWireError, AgentWireRequestEnvelope, AgentWireResponseEnvelope,
};
use mutsuki_agent_service_host_integration::AgentConnectionRegistry;
use mutsuki_bot_conversation::ConversationService;
use mutsuki_bot_flow::{
    BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider, BotFlowRegistry, BotNodeCatalog,
};
use mutsuki_bot_management::{BilibiliCredentialSecretState, BilibiliManagementApi};
use mutsuki_bot_protocol::{BotFlowDocument, ConversationPolicy};
use mutsuki_bot_sandbox::{SANDBOX_SERVICE_ID, SandboxService};
use mutsuki_bot_sdk::BotSubmissionGate;
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_config_service::{
    ConfigApplyMode, ConfigApplyRequest, ConfigConstraints, ConfigContext, ConfigDescriptor,
    ConfigDocumentKey, ConfigKey, ConfigMutability, ConfigNode, ConfigPresentation,
    ConfigProviderId, ConfigProviderRegistration, ConfigScope, ConfigService, ConfigValue,
    ConfigValueType, LocalizedText, MemoryConfigProvider, RestartPolicy, capability,
};
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_BRIDGE_PLUGIN_ID, BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_CONFIG_SERVICE_ID,
    BotAgentBridge, BotAgentConfig, BotAgentConfigHandle, agent_bridge_runner,
    bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_command::{
    BOT_COMMAND_PLUGIN_ID, BotCommandNodeRunner, bot_command_manifest,
};
use mutsuki_plugin_bot_conversation_context::{
    ConversationContextRunner, ConversationContextStore, bot_conversation_context_manifest,
};
use mutsuki_plugin_bot_delivery::{bot_reply_delivery_manifest_for, reply_delivery_runner_for};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_REGISTRY_SERVICE_ID, BOT_FLOW_ROUTER_PLUGIN_ID, BotFlowMatchRunner,
    flow_ingress_runner, flow_node_runner,
};
use mutsuki_plugin_bot_persona::{PersonaRunner, PersonaStore, bot_persona_manifest};
use mutsuki_plugin_bot_reply::{BotReplyRunner, bot_reply_manifest};
use mutsuki_runtime_contracts::{
    ContractSurfaceKind, PluginManifest, RuntimeLoadPlan, SurfaceRequirement,
};
use mutsuki_runtime_sdk::{
    HostEffect, HostEffectFuture, HostEffectKind, LoadedPlugin, PluginBuilder,
    RuntimeBootstrapperEffect, RuntimeBootstrapperService,
};
use mutsuki_service_config::HostSecretStore;
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, LoadPlanObserver, ServiceRuntimeBuilder,
    ServiceRuntimeResult,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    BILIBILI_MANAGEMENT_SERVICE_ID, BilibiliPollingCredentials, BilibiliPollingEventSource,
    BotReplyDeliveryRecoveryEventSource, QqBotPluginBundle,
};
use mutsuki_plugin_bot_bilibili::{
    BilibiliBackendConfig, BilibiliConfig, BilibiliConfigStore, BilibiliCredentialStore,
    BilibiliManagementService, BilibiliRunner, BilibiliSecretPresence,
    PLUGIN_ID as BILIBILI_PLUGIN_ID, ReqwestBilibiliOpenPlatformTransport,
    ReqwestBilibiliTransport, RuntimeBilibiliQrRenderer, SharedBilibiliConfig,
    SharedBilibiliCredential, SqliteBilibiliRepository, bilibili_config_descriptor,
    bilibili_config_value,
};
use mutsuki_plugin_bot_bilibili_workshop::{
    PLUGIN_ID as WORKSHOP_PLUGIN_ID, ReqwestWorkshopTransport, WorkshopRunner,
};
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;

/// Media resource provider id for assemblies that do not inject their own
/// binding (tests, benchmarks and examples). Products pass the persistent
/// SQLite provider explicitly.
pub const DEFAULT_MEDIA_PROVIDER_ID: &str = mutsuki_plugin_resource_sqlite::PROVIDER_ID;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowRouterConfig {}

struct ConfigProviderEffect(ConfigProviderRegistration);

impl HostEffect for ConfigProviderEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        Box::pin(async move {
            let _ = self.0.dispose();
            Ok(())
        })
    }
}

pub struct BotFlowRouterConfiguredPlugin {
    config: Arc<ConfigService>,
    registry: Option<Arc<BotFlowRegistry>>,
    seed: Option<BotFlowDocument>,
}

impl BotFlowRouterConfiguredPlugin {
    #[must_use]
    pub fn new(config: Arc<ConfigService>) -> Self {
        Self {
            config,
            registry: None,
            seed: None,
        }
    }

    #[must_use]
    pub fn with_registry(config: Arc<ConfigService>, registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            config,
            registry: Some(registry),
            seed: None,
        }
    }
}

struct LegacyBotEventRouterConfiguredPlugin;

impl ConfiguredPluginFactory for LegacyBotEventRouterConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        "mutsuki.bot.router.event"
    }

    fn prepare(
        &self,
        _config: &Value,
        _builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        Err("legacy Bot event subscriptions are unsupported; configure mutsuki.bot.router.flow and apply a graph".into())
    }
}

struct BotFlowLoadPlanObserver {
    registry: Arc<BotFlowRegistry>,
    config: Arc<ConfigService>,
    seed: Option<BotFlowDocument>,
}

impl LoadPlanObserver for BotFlowLoadPlanObserver {
    fn validate(&self, plan: &RuntimeLoadPlan) -> Result<(), String> {
        self.registry
            .validate_load_plan(plan)
            .map_err(|error| error.to_string())
    }

    fn activate(&self, plan: &RuntimeLoadPlan) {
        self.registry
            .activate_load_plan(plan)
            .expect("validated Bot Flow LoadPlan catalog must activate");
        let key = ConfigDocumentKey::new(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global());
        let config = self.config.clone();
        if self.config.repository().read(&key).ok().flatten().is_some() {
            tokio::spawn(async move {
                if let Err(error) = config
                    .restore(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global())
                    .await
                {
                    tracing::error!(
                        error = %error,
                        "stored Bot Flow could not be restored after startup; routing stays on an empty graph"
                    );
                }
            });
            return;
        }
        let Some(seed) = self.seed.clone() else {
            return;
        };
        tokio::spawn(async move {
            let candidate = ConfigValue::from_json(&serde_json::json!({ "flow": seed }));
            if let Err(error) = config
                .create_if_absent(
                    BOT_FLOW_CONFIG_PROVIDER_ID,
                    candidate,
                    ConfigContext::global(),
                )
                .await
            {
                tracing::error!(
                    error = %error,
                    "Bot Flow seed was not applied; routing stays on an empty graph"
                );
            }
        });
    }
}

impl ConfiguredPluginFactory for BotFlowRouterConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_FLOW_ROUTER_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config = if config.is_null() {
            Value::Object(Default::default())
        } else {
            config.clone()
        };
        let _config: FlowRouterConfig =
            serde_json::from_value(config).map_err(|error| error.to_string())?;
        let registry = self
            .registry
            .clone()
            .unwrap_or_else(|| Arc::new(BotFlowRegistry::new(BotNodeCatalog::default())));
        let provider = Arc::new(BotFlowConfigProvider::new(registry.clone()));
        let mut manifest =
            mutsuki_plugin_bot_event_router::flow_router_manifest_for_catalog(&registry.catalog());
        manifest
            .provides
            .services
            .push(BOT_FLOW_REGISTRY_SERVICE_ID.into());
        manifest.provides.capabilities.push("bot.flow".into());
        let loaded_manifest = manifest.clone();
        let ingress_registry = registry.clone();
        let node_registry = registry.clone();
        let service_registry = registry.clone();
        let ingress_stats_registry = registry.clone();
        let config_service = self.config.clone();
        Ok(builder
            .register_builtin_loaded_plugin_factory(manifest, move || {
                let registration = config_service
                    .register_provider_staged(provider.clone())
                    .map_err(|error| error.to_string())?;
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService::new(
                        BOT_FLOW_REGISTRY_SERVICE_ID,
                        service_registry.clone(),
                        "bot.flow",
                    )],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: vec![RuntimeBootstrapperEffect {
                        kind: HostEffectKind::HostLocal,
                        effect: Box::new(ConfigProviderEffect(registration)),
                    }],
                })
            })
            .register_builtin_runner(move || flow_ingress_runner(ingress_registry.clone()))
            .register_health_probe("mutsuki.bot.flow.ingress", move || {
                let stats = ingress_stats_registry.ingress_stats();
                serde_json::json!({
                    "status": "ok",
                    "accepted_total": stats.accepted_total(),
                    "dropped_total": stats.dropped_total(),
                })
            })
            .register_builtin_runner(move || Box::new(BotFlowMatchRunner::default()))
            .register_runtime_client_runner(move |client| {
                flow_node_runner(client, node_registry.clone())
            })
            .register_load_plan_observer(
                BOT_FLOW_REGISTRY_SERVICE_ID,
                Arc::new(BotFlowLoadPlanObserver {
                    registry,
                    config: self.config.clone(),
                    seed: self.seed.clone(),
                }),
            ))
    }
}

pub struct BotCommandConfiguredPlugin;

impl ConfiguredPluginFactory for BotCommandConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_COMMAND_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config = if config.is_null() {
            Value::Object(Default::default())
        } else {
            config.clone()
        };
        let _config: FlowRouterConfig =
            serde_json::from_value(config).map_err(|error| error.to_string())?;
        Ok(builder
            .register_builtin_plugin(bot_command_manifest(1))
            .register_builtin_runner(move || Box::new(BotCommandNodeRunner::new(1))))
    }
}

const BOT_AGENT_REPLY_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.agent.reply-delivery.runner";

struct ConfigSelectedAgentBackend {
    connections: AgentConnectionRegistry,
    config: BotAgentConfigHandle,
}

impl AgentClientBackend for ConfigSelectedAgentBackend {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let connection_id = self
            .config
            .snapshot()
            .selected_connection_id()
            .map_err(|error| AgentWireError {
                code: "bot.agent.connection.invalid".into(),
                message: error.to_string(),
                retryable: false,
            })?
            .ok_or_else(|| AgentWireError {
                code: "bot.agent.disabled".into(),
                message: "Bot Agent is disabled".into(),
                retryable: false,
            })?;
        self.connections
            .client_backend(&connection_id)
            .request(request)
    }
}

pub struct BotAgentConfiguredPlugin {
    connections: AgentConnectionRegistry,
}

impl BotAgentConfiguredPlugin {
    #[must_use]
    pub fn new(connections: AgentConnectionRegistry) -> Self {
        Self { connections }
    }
}

impl ConfiguredPluginFactory for BotAgentConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_AGENT_BRIDGE_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: BotAgentConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate().map_err(|error| error.to_string())?;
        let config_handle =
            BotAgentConfigHandle::new(config.clone()).map_err(|error| error.to_string())?;
        if !config.enabled {
            return Ok(register_bot_agent_services(
                builder,
                PluginBuilder::new(BOT_AGENT_BRIDGE_PLUGIN_ID)
                    .build()
                    .manifest,
                config_handle,
            ));
        }
        let state_dir = builder.data_dir().join("bot");
        std::fs::create_dir_all(&state_dir).map_err(|error| {
            format!(
                "failed to create Bot Agent state directory {}: {error}",
                state_dir.display()
            )
        })?;
        let repository = Arc::new(
            BotStateDbRepository::open(state_dir.join("state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let conversation_context: Arc<dyn ConversationContextStore> = repository.clone();
        let persona_store: Arc<dyn PersonaStore> = repository.clone();
        let connection_id = config
            .selected_connection_id()
            .map_err(|error| error.to_string())?
            .expect("enabled Bot Agent config has a connection id");
        let backend = ConfigSelectedAgentBackend {
            connections: self.connections.clone(),
            config: config_handle.clone(),
        };
        let client = AgentClient::new(backend);
        let conversations = ConversationService::new(
            repository.clone(),
            execution_product_policy(&config).map_err(|error| error.to_string())?,
        );
        let bridge =
            BotAgentBridge::new_with_config(conversations, Box::new(client), config_handle.clone());

        let mut manifest = merge_manifests(
            bot_agent_bridge_manifest(),
            bot_reply_delivery_manifest_for(
                BOT_AGENT_BRIDGE_PLUGIN_ID,
                BOT_AGENT_REPLY_DELIVERY_RUNNER_ID,
            ),
        );
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::Capability,
            connection_id.capability(),
        ));
        let builder = register_bot_agent_services(builder, manifest, config_handle.clone());
        Ok(builder
            .register_event_source(Box::new(BotReplyDeliveryRecoveryEventSource::for_plugin(
                Duration::from_millis(250),
                BOT_AGENT_BRIDGE_PLUGIN_ID,
            )))
            .register_builtin_plugin(bot_conversation_context_manifest())
            .register_builtin_plugin(bot_reply_manifest())
            .register_builtin_plugin(bot_persona_manifest())
            .register_builtin_runner({
                let store = conversation_context.clone();
                move || Box::new(ConversationContextRunner::new(store.clone()))
            })
            .register_builtin_runner(|| Box::new(BotReplyRunner::default()))
            .register_builtin_runner({
                let store = persona_store.clone();
                move || Box::new(PersonaRunner::new(store.clone()))
            })
            .register_dynamic_runner_limit(BOT_AGENT_BRIDGE_RUNNER_ID, {
                let config = config_handle.clone();
                move || {
                    let settings = config.snapshot();
                    (Some(settings.max_concurrency), Some(settings.timeout_ms))
                }
            })
            .register_runtime_client_runner(move |client| {
                agent_bridge_runner(client, bridge.clone())
            })
            .register_runtime_client_runner(move |client| {
                reply_delivery_runner_for(
                    client,
                    repository.clone(),
                    BOT_AGENT_BRIDGE_PLUGIN_ID,
                    BOT_AGENT_REPLY_DELIVERY_RUNNER_ID,
                )
            }))
    }
}

fn register_bot_agent_services(
    builder: ServiceRuntimeBuilder,
    mut manifest: PluginManifest,
    config: BotAgentConfigHandle,
) -> ServiceRuntimeBuilder {
    manifest
        .provides
        .services
        .push(BOT_AGENT_CONFIG_SERVICE_ID.into());
    manifest
        .provides
        .capabilities
        .push("bot.agent.config".into());
    let loaded_manifest = manifest.clone();
    let config = Arc::new(config);
    builder.register_builtin_loaded_plugin_factory(manifest, move || {
        Ok::<LoadedPlugin, String>(LoadedPlugin {
            manifest: loaded_manifest.clone(),
            runners: Vec::new(),
            async_handlers: Vec::new(),
            host_services: vec![RuntimeBootstrapperService::new(
                BOT_AGENT_CONFIG_SERVICE_ID,
                config.clone(),
                "bot.agent.config",
            )],
            resource_providers: Vec::new(),
            async_resource_providers: Vec::new(),
            host_effects: Vec::new(),
        })
    })
}

fn merge_manifests(mut left: PluginManifest, right: PluginManifest) -> PluginManifest {
    debug_assert_eq!(left.plugin_id, right.plugin_id);
    left.provides
        .capabilities
        .extend(right.provides.capabilities);
    left.provides.runners.extend(right.provides.runners);
    left.provides.protocols.extend(right.provides.protocols);
    left.provides
        .protocol_classes
        .extend(right.provides.protocol_classes);
    left.provides
        .handler_bindings
        .extend(right.provides.handler_bindings);
    left.provides.extensions.extend(right.provides.extensions);
    left
}

fn execution_product_policy(config: &BotAgentConfig) -> Result<ConversationPolicy, String> {
    Ok(ConversationPolicy {
        revision: 0,
        session_scope: config.session_scope().map_err(|error| error.to_string())?,
        business_profile_binding_id: None,
        agent_runtime_profile_id: (!config.default_profile_id.trim().is_empty())
            .then(|| config.default_profile_id.clone()),
        stt_enabled: config.stt_enabled,
        tts_enabled: config.tts_enabled,
        speech_reply_policy: config
            .speech_reply_policy()
            .map_err(|error| error.to_string())?,
        stt_selector_id: (!config.stt_selector_id.trim().is_empty())
            .then(|| config.stt_selector_id.clone()),
        tts_selector_id: (!config.tts_selector_id.trim().is_empty())
            .then(|| config.tts_selector_id.clone()),
        active_delivery_enabled: false,
    })
}

type SharedSandboxSlot = Arc<OnceLock<Arc<SandboxService>>>;

struct SandboxConfiguredPlugin {
    slot: SharedSandboxSlot,
}

impl ConfiguredPluginFactory for SandboxConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        SANDBOX_SERVICE_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config = if config.is_null() {
            Value::Object(Default::default())
        } else {
            config.clone()
        };
        let _config: FlowRouterConfig =
            serde_json::from_value(config).map_err(|error| error.to_string())?;
        let state_dir = builder.data_dir().join("bot");
        std::fs::create_dir_all(&state_dir).map_err(|error| {
            format!(
                "failed to create Bot sandbox state directory {}: {error}",
                state_dir.display()
            )
        })?;
        let repository = Arc::new(
            BotStateDbRepository::open(state_dir.join("state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let sandbox = Arc::new(
            SandboxService::with_history("local", repository).map_err(|error| error.to_string())?,
        );
        self.slot
            .set(sandbox.clone())
            .map_err(|_| "sandbox service already prepared".to_string())?;
        let mut manifest = PluginBuilder::new(SANDBOX_SERVICE_ID).build().manifest;
        manifest.provides.services.push(SANDBOX_SERVICE_ID.into());
        manifest.provides.capabilities.push("bot.sandbox".into());
        let loaded_manifest = manifest.clone();
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService::new(
                        SANDBOX_SERVICE_ID,
                        sandbox.clone(),
                        "bot.sandbox",
                    )],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: Vec::new(),
                })
            }),
        )
    }
}

pub struct QqBotConfiguredPlugin {
    slot: SharedSandboxSlot,
    media_provider_id: String,
}

impl ConfiguredPluginFactory for QqBotConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        QQBOT_ADAPTER_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let mut config: QqBotConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        // The product assembly owns the media resource provider binding; inbound
        // media validation reads it back from the adapter config.
        config.media_provider_id = Some(self.media_provider_id.clone());
        let mut bundle =
            QqBotPluginBundle::new(config).map_err(|error| error.redacted_message())?;
        bundle = bundle.with_resource_media_provider(self.media_provider_id.clone());
        if builder
            .configured_plugin_selection(SANDBOX_SERVICE_ID)
            .is_some()
        {
            let sandbox = self.slot.get().cloned().ok_or_else(|| {
                "sandbox plugin must be prepared before the QQ adapter".to_string()
            })?;
            bundle = bundle.with_workspace_sandbox(sandbox);
        }
        bundle
            .install(builder)
            .map_err(|error| error.redacted_message())
    }
}

pub struct BilibiliConfiguredPlugin {
    config_service: Option<Arc<ConfigService>>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
    media_provider_id: String,
}

impl BilibiliConfiguredPlugin {
    fn new(config_service: Option<Arc<ConfigService>>, media_provider_id: String) -> Self {
        Self {
            config_service,
            flow_registry: None,
            media_provider_id,
        }
    }

    /// Shares the Flow registry so the push poll path and the management
    /// status observe the same active graph the router activates.
    fn with_flow_registry(mut self, registry: Arc<BotFlowRegistry>) -> Self {
        self.flow_registry = Some(registry);
        self
    }
}

fn block_on_config<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle)
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) =>
        {
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Ok(_) => std::thread::spawn(move || futures_executor::block_on(future))
            .join()
            .expect("Bilibili config worker"),
        Err(_) => futures_executor::block_on(future),
    }
}

struct HostBilibiliCredentialStore {
    host: HostSecretStore,
    shared: SharedBilibiliCredential,
}

impl BilibiliCredentialStore for HostBilibiliCredentialStore {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
        self.host
            .rotate(key, credential.clone())
            .map_err(|error| error.to_string())?;
        self.shared.set(credential);
        Ok(())
    }
}

struct ConfigServiceBilibiliConfigStore(Arc<ConfigService>);

impl BilibiliConfigStore for ConfigServiceBilibiliConfigStore {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
        let service = self.0.clone();
        let config = config.clone();
        block_on_config(async move {
            let snapshot = service
                .read(
                    BILIBILI_PLUGIN_ID,
                    ConfigContext::global(),
                    &[capability::VALUE_READ.into()],
                )
                .await?;
            let stored = snapshot.value.to_json();
            // The product-owned document wraps the runtime config with the
            // enable switch and projected UI fields; legacy documents hold the
            // bare runtime config.
            let candidate = match stored.get("enabled").and_then(Value::as_bool) {
                Some(enabled) => bilibili_config_value(enabled, &config),
                None => ConfigValue::from_json(
                    &serde_json::to_value(&config).expect("Bilibili config serializes"),
                ),
            };
            service
                .apply(
                    BILIBILI_PLUGIN_ID,
                    ConfigApplyRequest {
                        candidate,
                        expected_revision: snapshot.revision,
                        dry_run: false,
                    },
                    ConfigContext::global(),
                    &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
                )
                .await
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
    }
}

struct HostSecretPresence(HostSecretStore);

impl BilibiliSecretPresence for HostSecretPresence {
    fn inspect(&self, key: &str) -> BilibiliCredentialSecretState {
        match self.0.resolve(key) {
            None => BilibiliCredentialSecretState::Absent,
            Some(value) if value.trim().is_empty() => BilibiliCredentialSecretState::Invalid,
            Some(_) => BilibiliCredentialSecretState::Present,
        }
    }
}

impl ConfiguredPluginFactory for BilibiliConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BILIBILI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let mut config: BilibiliConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        // The product assembly owns the media resource provider binding; the
        // owner document no longer carries a provider id.
        config.media_provider_id = self.media_provider_id.clone();
        // The product pre-registers the owner config provider before plugins
        // install; only fall back to a plugin-owned provider when no product
        // owns this provider id yet.
        let config_provider = self.config_service.as_ref().and_then(|service| {
            service
                .registry()
                .get(BILIBILI_PLUGIN_ID)
                .is_err()
                .then(|| {
                    Arc::new(MemoryConfigProvider::new(
                        bilibili_config_descriptor(),
                        ConfigValue::from_json(
                            &serde_json::to_value(&config).expect("Bilibili config serializes"),
                        ),
                        ConfigApplyMode::HotReload,
                    ))
                })
                .map(|provider| (service.clone(), provider))
        });
        if let Some((service, provider)) = &config_provider {
            let service = service.clone();
            let provider = provider.clone();
            let seed = ConfigValue::from_json(
                &serde_json::to_value(&config).expect("Bilibili config serializes"),
            );
            let snapshot = block_on_config(async move {
                service
                    .prepare_provider_candidate(provider, Some(seed), ConfigContext::global())
                    .await
            })
            .map_err(|error| error.to_string())?;
            config = serde_json::from_value(snapshot.value.to_json())
                .map_err(|error| error.to_string())?;
        }
        config.validate()?;
        let host_secret_store = builder.host_secret_store();
        if matches!(config.backend, BilibiliBackendConfig::OpenPlatform { .. })
            && !host_secret_store.rotation_available()
        {
            return Err(
                "Bilibili Open Platform requires a Host security.secret_file for OAuth refresh"
                    .into(),
            );
        }
        if config.management.enabled {
            if !matches!(config.backend, BilibiliBackendConfig::WebCookie { .. }) {
                return Err("Bilibili management requires backend.type = web_cookie".into());
            }
            if !host_secret_store.rotation_available() {
                return Err("Bilibili management requires a Host security.secret_file".into());
            }
        }
        let config_service = if config.management.enabled {
            Some(
                self.config_service
                    .clone()
                    .ok_or_else(|| "Bilibili management requires ConfigService".to_string())?,
            )
        } else {
            None
        };
        let repository = Arc::new(
            SqliteBilibiliRepository::open(builder.data_dir().join("bilibili/state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let web_credential = SharedBilibiliCredential::default();
        let app_secret = SharedBilibiliCredential::default();
        let oauth_credential = SharedBilibiliCredential::default();
        let shared_config = SharedBilibiliConfig::new(config);
        let runner_config = shared_config.clone();
        let runner_repository = repository.clone();
        let runner_web_credential = web_credential.clone();
        let runner_app_secret = app_secret.clone();
        let runner_oauth_credential = oauth_credential.clone();
        let source_credentials = match &shared_config.snapshot().backend {
            BilibiliBackendConfig::WebCookie { cookie_secret_key } => {
                BilibiliPollingCredentials::WebCookie {
                    secret_key: cookie_secret_key.clone(),
                    credential: web_credential.clone(),
                    required: !shared_config.snapshot().management.enabled,
                }
            }
            BilibiliBackendConfig::OpenPlatform {
                app_secret_key,
                oauth_credential_key,
                ..
            } => BilibiliPollingCredentials::OpenPlatform {
                app_secret_key: app_secret_key.clone(),
                app_secret: app_secret.clone(),
                oauth_credential_key: oauth_credential_key.clone(),
                oauth_credential: oauth_credential.clone(),
            },
        };
        let source = BilibiliPollingEventSource::new(shared_config.clone(), source_credentials);
        let manifest_config = runner_config.snapshot();
        let mut manifest = mutsuki_plugin_bot_bilibili::manifest_for_config(&manifest_config);
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            runner_config.snapshot().media_provider_id,
        ));
        BotSubmissionGate::ensure_manifest_business_surface(&manifest)
            .map_err(|error| error.to_string())?;

        let management_service = if let Some(config_service) = config_service {
            let mut service = BilibiliManagementService::new(
                runner_config.clone(),
                web_credential.clone(),
                Box::new(ReqwestBilibiliTransport::new(
                    web_credential.clone(),
                    Duration::from_secs(15),
                )),
                repository.clone(),
                Arc::new(HostBilibiliCredentialStore {
                    host: host_secret_store.clone(),
                    shared: web_credential.clone(),
                }),
                Arc::new(ConfigServiceBilibiliConfigStore(config_service)),
                Arc::new(HostSecretPresence(host_secret_store.clone())),
            );
            if let Some(registry) = self.flow_registry.clone() {
                service = service.with_flow_registry(registry);
            }
            Some(Arc::new(service))
        } else {
            None
        };

        if management_service.is_some() {
            manifest
                .provides
                .services
                .push(BILIBILI_MANAGEMENT_SERVICE_ID.into());
            manifest
                .provides
                .capabilities
                .push("bot.bilibili.management".into());
        }
        let loaded_manifest = manifest.clone();
        let management_api = management_service
            .clone()
            .map(|service| service as Arc<dyn BilibiliManagementApi>);
        let runner_flow_registry = self.flow_registry.clone();
        let builder = builder.register_builtin_loaded_plugin_factory(manifest, move || {
            let host_services = management_api
                .as_ref()
                .map(|management_api| {
                    RuntimeBootstrapperService::new(
                        BILIBILI_MANAGEMENT_SERVICE_ID,
                        Arc::new(management_api.clone()),
                        "bot.bilibili.management",
                    )
                })
                .into_iter()
                .collect();
            let host_effects = config_provider
                .as_ref()
                .map(|(service, provider)| {
                    service
                        .register_provider_staged(provider.clone())
                        .map(|registration| RuntimeBootstrapperEffect {
                            kind: HostEffectKind::HostLocal,
                            effect: Box::new(ConfigProviderEffect(registration)),
                        })
                        .map_err(|error| error.to_string())
                })
                .transpose()?
                .into_iter()
                .collect();
            Ok::<LoadedPlugin, String>(LoadedPlugin {
                manifest: loaded_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services,
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
                host_effects,
            })
        });

        Ok(builder
            .register_fallible_runtime_services_runner(move |client, resources| {
                let snapshot = runner_config.snapshot();
                let transport: Box<dyn mutsuki_plugin_bot_bilibili::BilibiliTransport> =
                    match &snapshot.backend {
                        BilibiliBackendConfig::WebCookie { .. } => {
                            Box::new(ReqwestBilibiliTransport::new(
                                runner_web_credential.clone(),
                                Duration::from_secs(15),
                            ))
                        }
                        BilibiliBackendConfig::OpenPlatform {
                            client_id,
                            oauth_credential_key,
                            authorized_uid,
                            ..
                        } => Box::new(ReqwestBilibiliOpenPlatformTransport::new(
                            client_id,
                            *authorized_uid,
                            runner_app_secret.clone(),
                            runner_oauth_credential.clone(),
                            oauth_credential_key,
                            Arc::new(HostBilibiliCredentialStore {
                                host: host_secret_store.clone(),
                                shared: runner_oauth_credential.clone(),
                            }),
                            Duration::from_secs(15),
                        )),
                    };
                let mut runner = BilibiliRunner::new_for_backend(
                    transport,
                    runner_repository.clone(),
                    resources.clone(),
                    snapshot.media_provider_id.clone(),
                    snapshot.backend.kind(),
                );
                if snapshot.management.enabled {
                    let management = management_service.clone().ok_or_else(|| {
                        mutsuki_plugin_bot_bilibili::BilibiliError::ManagementUnavailable(
                            "Bilibili management service is unavailable".into(),
                        )
                    })?;
                    management.bind_qr_renderer(Arc::new(RuntimeBilibiliQrRenderer::new(
                        client.clone(),
                        resources,
                    )));
                    runner = runner.with_management(runner_config.clone(), management);
                }
                if let Some(registry) = runner_flow_registry.clone() {
                    runner = runner.with_flow_registry(registry);
                }
                Ok::<
                    Box<dyn mutsuki_runtime_core::Runner>,
                    mutsuki_plugin_bot_bilibili::BilibiliError,
                >(runner.into_runtime_runner(client, snapshot.risk_control.clone()))
            })
            .register_event_source(Box::new(source)))
    }
}

/// Owner-document shape kept for backward compatibility: legacy documents may
/// still carry a `media_provider_id` field, which the product assembly now
/// ignores in favor of its own provider binding.
#[derive(Clone, Debug, Deserialize)]
struct LinkCardPluginConfig {}

pub struct WorkshopConfiguredPlugin {
    media_provider_id: String,
}

impl ConfiguredPluginFactory for WorkshopConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        WORKSHOP_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let _config: LinkCardPluginConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        let mut manifest = mutsuki_plugin_bot_bilibili_workshop::manifest();
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            self.media_provider_id.clone(),
        ));
        BotSubmissionGate::ensure_manifest_business_surface(&manifest)
            .map_err(|error| error.to_string())?;
        let media_provider_id = self.media_provider_id.clone();
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                let transport = ReqwestWorkshopTransport::new();
                Ok::<Box<dyn mutsuki_runtime_core::Runner>, String>(Box::new(WorkshopRunner::new(
                    Box::new(transport),
                    resources,
                    media_provider_id.clone(),
                )))
            }))
    }
}

pub struct MihuashiConfiguredPlugin {
    media_provider_id: String,
}

impl ConfiguredPluginFactory for MihuashiConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        MIHUASHI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let _config: LinkCardPluginConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        let mut manifest = mutsuki_plugin_bot_mihuashi::manifest();
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            self.media_provider_id.clone(),
        ));
        manifest.requires.push(SurfaceRequirement::task_protocol(
            "mutsuki.browser.snapshot",
        ));
        BotSubmissionGate::ensure_manifest_business_surface(&manifest)
            .map_err(|error| error.to_string())?;
        let media_provider_id = self.media_provider_id.clone();
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_runtime_services_runner(move |client, resources| {
                mutsuki_plugin_bot_mihuashi::runner(client, resources, media_provider_id.clone())
            }))
    }
}

/// Product-facing link-card plugin configuration shared by the Workshop and
/// Mihuashi factories. The owner document only exposes the enable switch; the
/// media resource provider binding is owned by the product assembly.
pub fn link_card_config_descriptor(plugin_id: &str, title: &str) -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(plugin_id),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new(title),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("link_card"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new(title),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::PluginReload,
            children: vec![bool_node("enabled", "启用", Some("关闭后不会加载该插件。"))],
        },
        groups: Vec::new(),
    }
}

/// Projects the link-card owner document shape.
pub fn link_card_config_value(enabled: bool) -> ConfigValue {
    ConfigValue::Object(
        [("enabled".into(), ConfigValue::Bool(enabled))]
            .into_iter()
            .collect(),
    )
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

/// Catalog of production Bot plugins that can be selected by ServiceHost configuration.
/// Media upload is intentionally absent until a product registers an explicit provider-backed
/// QQ factory of its own.
///
/// Conversation-context, reply and persona runners are registered by
/// `BotAgentConfiguredPlugin` against the shared `BotStateDb` file. They are not
/// independently selectable factories (that path used a process-local Memory store).
pub fn configured_bot_plugin_catalog(
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    configured_bot_plugin_catalog_inner(None, None, media_provider_id)
}

fn configured_bot_plugin_catalog_inner(
    config: Option<Arc<ConfigService>>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    let slot: SharedSandboxSlot = Arc::new(OnceLock::new());
    catalog.register(LegacyBotEventRouterConfiguredPlugin)?;
    catalog.register(BotCommandConfiguredPlugin)?;
    catalog.register(SandboxConfiguredPlugin { slot: slot.clone() })?;
    catalog.register(QqBotConfiguredPlugin {
        slot,
        media_provider_id: media_provider_id.clone(),
    })?;
    let bilibili = match flow_registry.clone() {
        Some(registry) => BilibiliConfiguredPlugin::new(config, media_provider_id.clone())
            .with_flow_registry(registry),
        None => BilibiliConfiguredPlugin::new(config, media_provider_id.clone()),
    };
    catalog.register(bilibili)?;
    catalog.register(WorkshopConfiguredPlugin {
        media_provider_id: media_provider_id.clone(),
    })?;
    catalog.register(MihuashiConfiguredPlugin { media_provider_id })?;
    Ok(catalog)
}

/// Adds the Flow Router only when product bootstrap supplies ConfigService.
pub fn configured_bot_plugin_catalog_with_config(
    config: Arc<ConfigService>,
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let flow_registry = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));
    let mut catalog = configured_bot_plugin_catalog_inner(
        Some(config.clone()),
        Some(flow_registry.clone()),
        media_provider_id,
    )?;
    catalog.register(BotFlowRouterConfiguredPlugin::with_registry(
        config,
        flow_registry,
    ))?;
    Ok(catalog)
}

/// Production Bot catalog with configurable Agent nodes wired to a shared Agent owner
/// registry. The base catalog intentionally remains Agent-free for products that do not opt in.
/// `seed_flow` is applied by LoadPlan activation into stores that never recorded a flow
/// document; existing records, including a graph the user cleared, are never overwritten.
pub fn configured_bot_plugin_catalog_with_agent_and_flow(
    config: Arc<ConfigService>,
    connections: AgentConnectionRegistry,
    flow_registry: Arc<BotFlowRegistry>,
    seed_flow: Option<BotFlowDocument>,
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = configured_bot_plugin_catalog_inner(
        Some(config.clone()),
        Some(flow_registry.clone()),
        media_provider_id,
    )?;
    catalog.register(BotFlowRouterConfiguredPlugin {
        config,
        registry: Some(flow_registry),
        seed: seed_flow,
    })?;
    catalog.register(BotAgentConfiguredPlugin::new(connections))?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_config_service::{
        ConfigCompareAndSetRequest, ConfigContext, ConfigDocumentKey, ConfigProviderRegistry,
        ConfigRepository, ConfigRevision, ConfigService, ConfigValue, InMemoryConfigRepository,
    };
    use mutsuki_plugin_bot_adapter_qqbot::qqbot_adapter_manifest;
    use mutsuki_plugin_bot_event_router::flow_router_manifest;
    use mutsuki_plugin_bot_interaction::bot_interaction_manifest;
    use mutsuki_runtime_contracts::RuntimeLoadPlan;
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use super::*;

    fn reference_catalog_manifests() -> Vec<PluginManifest> {
        vec![
            qqbot_adapter_manifest(1, false),
            flow_router_manifest(),
            bot_command_manifest(1),
            bot_conversation_context_manifest(),
            bot_agent_bridge_manifest(),
            bot_reply_manifest(),
            mutsuki_plugin_bot_delivery::bot_reply_delivery_manifest(),
            bot_persona_manifest(),
            bot_interaction_manifest(),
            mutsuki_plugin_bot_bilibili::manifest(),
            mutsuki_plugin_bot_mihuashi::manifest(),
        ]
    }

    fn empty_load_plan(manifests: Vec<PluginManifest>) -> RuntimeLoadPlan {
        RuntimeLoadPlan {
            lock_version: 1,
            core_api_version: "1".into(),
            profile_id: "test".into(),
            profile_hash: "hash".into(),
            registry_generation: 1,
            plugins: manifests,
            load_order: Vec::new(),
            runner_bindings: BTreeMap::new(),
            plugin_deployments: BTreeMap::new(),
            observability: Default::default(),
            capability_graph: Default::default(),
            contract_surfaces: Vec::new(),
        }
    }

    #[tokio::test]
    async fn flow_observer_seeds_reference_graph_when_store_has_no_record() {
        let manifests = reference_catalog_manifests();
        let registry = Arc::new(BotFlowRegistry::new(
            BotNodeCatalog::from_manifests(&manifests).expect("reference catalogs merge"),
        ));
        let config = Arc::new(
            ConfigService::new(
                Arc::new(ConfigProviderRegistry::default()),
                Arc::new(InMemoryConfigRepository::default()),
            )
            .expect("config service"),
        );
        config
            .registry()
            .register(Arc::new(BotFlowConfigProvider::new(registry.clone())))
            .expect("flow provider registers");
        let observer = BotFlowLoadPlanObserver {
            registry: registry.clone(),
            config: config.clone(),
            seed: Some(crate::orchestrated_flow::qq_full_business_flow()),
        };

        observer.activate(&empty_load_plan(manifests));

        let key = ConfigDocumentKey::new(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global());
        let mut seeded = false;
        for _ in 0..100 {
            let recorded = config
                .repository()
                .read(&key)
                .expect("repository readable")
                .is_some_and(|snapshot| {
                    snapshot.value.to_json()["flow"]["flow_id"] == "qq.business.full"
                });
            if recorded {
                seeded = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(seeded, "observer must seed the absent flow record");
        assert_eq!(registry.active().flow.flow_id, "qq.business.full");
    }

    fn seed_stored_flow(repo: &InMemoryConfigRepository, value: ConfigValue) {
        let mut write = repo
            .prepare_compare_and_set(ConfigCompareAndSetRequest {
                key: ConfigDocumentKey::new(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global()),
                expected_revision: ConfigRevision::ABSENT,
                value,
                schema_version: 1,
                value_version: 2,
            })
            .unwrap();
        write.commit().unwrap();
        write.finish().unwrap();
    }

    #[tokio::test]
    async fn configured_qq_plugin_fails_preflight_without_host_secret() {
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
        service.plugins.configured = vec![ConfiguredPluginSelection {
            id: QQBOT_ADAPTER_PLUGIN_ID.into(),
            enabled: true,
            config: json!({
                "account_id": "configured",
                "app_id": "APP_ID",
                "client_secret_key": "MISSING_CONFIGURED_QQ_SECRET"
            }),
        }];

        let error = match ServiceRuntimeBuilder::new(service)
            .with_configured_plugin_catalog(
                configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap(),
            )
            .start()
            .await
        {
            Ok(runtime) => {
                runtime.shutdown().await;
                panic!("configured QQBot unexpectedly started")
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("MISSING_CONFIGURED_QQ_SECRET"));
    }

    #[test]
    fn graph_only_plugins_accept_omitted_empty_config() {
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.service.data_dir = root.path().join("data");
        let config = Arc::new(
            ConfigService::new(
                Arc::new(mutsuki_config_service::ConfigProviderRegistry::default()),
                Arc::new(mutsuki_config_service::InMemoryConfigRepository::default()),
            )
            .unwrap(),
        );

        BotFlowRouterConfiguredPlugin::new(config)
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service.clone()))
            .expect("Flow Router should accept a graph-only selection without a config table");
        BotCommandConfiguredPlugin
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service))
            .expect(
                "Command node plugin should accept a graph-only selection without a config table",
            );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stored_incompatible_flow_does_not_block_start() {
        let repo = InMemoryConfigRepository::default();
        seed_stored_flow(
            &repo,
            ConfigValue::from_json(&json!({
                "flow": {
                    "nodes": [{
                        "node_id": "missing",
                        "node_type_id": "does.not.exist",
                        "node_type_version": 1
                    }]
                }
            })),
        );
        let config = Arc::new(
            ConfigService::new(
                Arc::new(mutsuki_config_service::ConfigProviderRegistry::default()),
                Arc::new(repo),
            )
            .unwrap(),
        );
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
        service.service.home_dir = root.path().into();
        service.service.data_dir = root.path().join("data");
        service.service.log_dir = root.path().join("logs");
        service.service.run_dir = root.path().join("run");
        std::fs::create_dir_all(&service.service.data_dir).unwrap();
        std::fs::create_dir_all(&service.service.log_dir).unwrap();
        std::fs::create_dir_all(&service.service.run_dir).unwrap();
        service.plugins.configured = vec![ConfiguredPluginSelection {
            id: BOT_FLOW_ROUTER_PLUGIN_ID.into(),
            enabled: true,
            config: Value::Null,
        }];
        let mut catalog = ConfiguredPluginCatalog::new();
        catalog
            .register(BotFlowRouterConfiguredPlugin::new(config))
            .unwrap();
        let runtime = ServiceRuntimeBuilder::new(service)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .expect("stored Flow must not block start");
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let snapshot = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap()
            .active();
        assert_eq!(snapshot.revision, 0);
        assert!(snapshot.flow.nodes.is_empty());
        runtime.shutdown().await;
    }

    #[test]
    fn configured_bilibili_management_requires_host_persistence_boundaries() {
        let config = json!({
            "backend": {"type": "web_cookie", "cookie_secret_key": "BILIBILI_COOKIE"},
            "live_interval_ms": 1000,
            "dynamic_interval_ms": 1000,
            "video_interval_ms": 1000,
            "retry": {"max_attempts": 3, "initial_backoff_ms": 10, "max_backoff_ms": 100},
            "subscriptions": [],
            "link_resolver": {"enabled": false, "cooldown_ms": 1000, "account_to_binding": {}},
            "media_provider_id": "memory",
            "management": {
                "enabled": true,
                "allow_self_binding": true,
                "admin_user_ids": ["admin"],
                "self_binding_notifications": ["dynamic"],
                "self_binding_outbound_binding": "qq-main"
            }
        });
        let error = match BilibiliConfiguredPlugin::new(None, DEFAULT_MEDIA_PROVIDER_ID.to_string())
            .prepare(
                &config,
                ServiceRuntimeBuilder::new(ServiceConfig::default()),
            ) {
            Ok(_) => panic!("Bilibili management unexpectedly accepted missing Host stores"),
            Err(error) => error,
        };
        assert!(error.contains("security.secret_file"));
    }

    #[test]
    fn configured_bilibili_open_platform_requires_rotatable_oauth_store() {
        let config = json!({
            "backend": {
                "type": "open_platform",
                "client_id": "client",
                "app_secret_key": "BILIBILI_OPEN_APP_SECRET",
                "oauth_credential_key": "BILIBILI_OPEN_OAUTH",
                "authorized_uid": 42
            },
            "live_interval_ms": 1000,
            "dynamic_interval_ms": 1000,
            "video_interval_ms": 1000,
            "retry": {"max_attempts": 3, "initial_backoff_ms": 10, "max_backoff_ms": 100},
            "subscriptions": [],
            "link_resolver": {"enabled": false, "cooldown_ms": 1000, "account_to_binding": {}},
            "media_provider_id": "memory",
            "management": {
                "enabled": false,
                "allow_self_binding": false,
                "admin_user_ids": [],
                "self_binding_notifications": ["live", "video"],
                "self_binding_outbound_binding": ""
            }
        });
        let error = BilibiliConfiguredPlugin::new(None, DEFAULT_MEDIA_PROVIDER_ID.to_string())
            .prepare(
                &config,
                ServiceRuntimeBuilder::new(ServiceConfig::default()),
            )
            .err()
            .expect("Open Platform unexpectedly accepted a non-rotatable secret store");
        assert!(error.contains("OAuth refresh"));
    }

    #[test]
    fn legacy_orchestration_configuration_is_rejected_at_the_owner_boundary() {
        let builder = ServiceRuntimeBuilder::new(ServiceConfig::default());
        let error = LegacyBotEventRouterConfiguredPlugin
            .prepare(&json!({"subscriptions": []}), builder)
            .err()
            .expect("legacy event router must be rejected");
        assert!(error.contains("apply a graph"));

        let error = BotCommandConfiguredPlugin
            .prepare(
                &json!({"prefixes": ["/"], "commands": []}),
                ServiceRuntimeBuilder::new(ServiceConfig::default()),
            )
            .err()
            .expect("legacy command config must be rejected");
        assert!(error.contains("unknown field"));

        assert!(
            serde_json::from_value::<mutsuki_plugin_bot_bilibili::BilibiliManagementConfig>(
                json!({
                    "enabled": false,
                    "allow_self_binding": false,
                    "command": "bili",
                    "admin_user_ids": [],
                    "self_binding_notifications": [],
                    "self_binding_outbound_binding": ""
                }),
            )
            .is_err()
        );
    }
}
