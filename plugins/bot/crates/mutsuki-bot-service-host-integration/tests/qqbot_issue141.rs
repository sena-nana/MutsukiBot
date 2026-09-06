// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_bot_flow::{BotFlowRegistry, BotNodeCatalog};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BotEvent, BotEventKind, BotFlowDocument,
    BotFlowEdge, BotFlowEdgeKind, BotFlowNode, BotFlowNodePosition, BotFlowSnapshot,
    BotFlowSourceSelector, BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment,
    BotNodeDescriptor, BotNodeInvocation, BotNodePortDescriptor, BotNodePortDirection,
    BotNodeResult, BotNodeRole, BotTarget, MessageSegment,
};
use mutsuki_bot_service_host_integration::{
    DEFAULT_MEDIA_PROVIDER_ID, configured_bot_plugin_catalog,
};
use mutsuki_bot_testkit::{FakeQqGatewayScript, FakeQqServer};
use mutsuki_plugin_bot_adapter_qqbot::tasks::{
    QQ_NODE_BOT_CONNECTED, QQ_NODE_BOT_DISCONNECTED, QQ_NODE_MEMBER_JOINED, QQ_NODE_MEMBER_LEFT,
    QQ_NODE_MESSAGE_CREATED, QQ_NODE_MESSAGE_DELETED, QQ_NODE_MESSAGE_UPDATED,
    QQ_NODE_REACTION_ADDED, QQ_NODE_REACTION_REMOVED, qqbot_adapter_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_REGISTRY_SERVICE_ID, BotFlowMatchRunner, flow_ingress_runner, flow_node_runner,
    flow_router_manifest,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, RunnerDescriptor, RunnerResult, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::{Value, json};
use tokio::sync::Notify;

const CAPTURE_PLUGIN_ID: &str = "test.qqbot.issue141.capture";
const CAPTURE_RUNNER_ID: &str = "test.qqbot.issue141.capture.runner";
const CAPTURE_PROTOCOL_ID: &str = "test.qqbot.issue141/capture@1";

struct CaptureRunner {
    descriptor: RunnerDescriptor,
    events: Arc<Mutex<Vec<BotEvent>>>,
    notify: Arc<Notify>,
}

impl Runner for CaptureRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation = task
                .payload
                .decode_shared::<BotNodeInvocation>()
                .expect("capture task carries a node invocation")
                .as_ref()
                .clone();
            let event: BotEvent = serde_json::from_value(invocation.input.payload.value).unwrap();
            self.events.lock().unwrap().push(event);
            self.notify.notify_waiters();
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(
                serde_json::to_value(BotNodeResult {
                    outputs: Vec::new(),
                    metadata: Default::default(),
                })
                .unwrap(),
            );
            Ok(result)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_gateway_delivers_private_group_channel_and_distinct_delete_once() {
    let group = gateway_event(
        3,
        "event-group",
        "GROUP_AT_MESSAGE_CREATE",
        json!({
            "id": "message-group",
            "group_openid": "group",
            "content": "<@bot> group",
            "mentions": [{"id": "bot", "is_you": true}],
            "author": {"member_openid": "group-user"}
        }),
    );
    let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
        initial_events: vec![
            gateway_event(
                2,
                "event-private",
                "C2C_MESSAGE_CREATE",
                json!({
                    "id": "message-private",
                    "content": "private",
                    "author": {"user_openid": "private-user"}
                }),
            ),
            group.clone(),
            gateway_event(
                4,
                "event-channel",
                "AT_MESSAGE_CREATE",
                json!({
                    "id": "message-channel",
                    "guild_id": "guild",
                    "channel_id": "channel",
                    "content": "channel",
                    "mentions": [{"id": "bot", "is_you": true}],
                    "message_reference": {"message_id": "quoted-message"},
                    "author": {"id": "channel-user"}
                }),
            ),
        ],
        resumed_events: vec![
            gateway_event(
                5,
                "event-group-replayed",
                "GROUP_AT_MESSAGE_CREATE",
                group["d"].clone(),
            ),
            gateway_event(
                6,
                "event-channel-delete",
                "PUBLIC_MESSAGE_DELETE",
                json!({
                    "id": "message-channel",
                    "guild_id": "guild",
                    "channel_id": "channel",
                    "author": {"id": "channel-user"}
                }),
            ),
        ],
        ..FakeQqGatewayScript::default()
    })
    .await;
    let secret_key = format!("QQBOT_ISSUE141_SECRET_{}", fake.websocket_addr().port());
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("local.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let qq = fake.config("issue141", "TEST_APP_ID", &secret_key);
    let config_path = root.path().join("local.toml");
    std::fs::write(&config_path, product_toml(root.path(), &qq)).unwrap();
    let config = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(Notify::new());
    let descriptor = RunnerDescriptorBuilder::new(CAPTURE_RUNNER_ID, CAPTURE_PLUGIN_ID)
        .accepted_protocol(CAPTURE_PROTOCOL_ID)
        .execution_class(ExecutionClass::Io)
        .build();
    let manifest = PluginBuilder::new(CAPTURE_PLUGIN_ID)
        .runner_descriptor(descriptor.clone())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(CAPTURE_PROTOCOL_ID)
                .input_schema(json!({
                    "type": "object",
                    "required": ["flow_id", "node_id", "input"]
                }))
                .output_schema(json!({
                    "type": "object",
                    "required": ["outputs", "metadata"]
                }))
                .error_schema(json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            CAPTURE_RUNNER_ID,
            "capture",
        )
        .extension(
            BotNodeCatalogFragment {
                nodes: vec![BotNodeDescriptor {
                    node_type_id: CAPTURE_PLUGIN_ID.into(),
                    version: 1,
                    title: "Capture".into(),
                    category: "Test".into(),
                    role: BotNodeRole::Sink,
                    binding: Some(BotNodeBinding {
                        binding_id: format!("binding:{CAPTURE_PROTOCOL_ID}"),
                        protocol_id: CAPTURE_PROTOCOL_ID.into(),
                        runner_hint: Some(CAPTURE_RUNNER_ID.into()),
                    }),
                    ports: vec![
                        capture_port("event", mutsuki_bot_protocol::BOT_FLOW_MESSAGE_EVENT_TYPE),
                        capture_port(
                            "deleted",
                            mutsuki_bot_protocol::BOT_FLOW_MESSAGE_DELETED_EVENT_TYPE,
                        ),
                        capture_port(
                            "reaction",
                            mutsuki_bot_protocol::BOT_FLOW_REACTION_EVENT_TYPE,
                        ),
                        capture_port("member", mutsuki_bot_protocol::BOT_FLOW_MEMBER_EVENT_TYPE),
                        capture_port(
                            "lifecycle",
                            mutsuki_bot_protocol::BOT_FLOW_LIFECYCLE_EVENT_TYPE,
                        ),
                    ],
                    config_schema: json!({"type": "object", "additionalProperties": false}),
                }],
            }
            .into_plugin_extension()
            .unwrap(),
        )
        .build()
        .manifest;
    let flow_registry = capture_flow_registry(&manifest);
    let mut flow_manifest = flow_router_manifest();
    flow_manifest
        .provides
        .services
        .push(BOT_FLOW_REGISTRY_SERVICE_ID.into());
    flow_manifest.provides.capabilities.push("bot.flow".into());
    let loaded_flow_manifest = flow_manifest.clone();
    let ingress_registry = flow_registry.clone();
    let node_registry = flow_registry.clone();
    let service_registry = flow_registry.clone();
    let runner_descriptor = descriptor.clone();
    let runner_events = events.clone();
    let runner_notify = notify.clone();
    let mut catalog =
        mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap();
    catalog
        .merge(configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap())
        .unwrap();
    let runtime = ServiceRuntimeBuilder::new(config)
        .with_configured_plugin_catalog(catalog)
        .register_builtin_loaded_plugin_factory(flow_manifest, move || {
            Ok::<mutsuki_runtime_sdk::LoadedPlugin, String>(mutsuki_runtime_sdk::LoadedPlugin {
                manifest: loaded_flow_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services: vec![mutsuki_runtime_sdk::RuntimeBootstrapperService::new(
                    BOT_FLOW_REGISTRY_SERVICE_ID,
                    service_registry.clone(),
                    "bot.flow",
                )],
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
                host_effects: Vec::new(),
            })
        })
        .register_builtin_runner(move || flow_ingress_runner(ingress_registry.clone()))
        .register_builtin_runner(|| Box::new(BotFlowMatchRunner::default()))
        .register_runtime_client_runner(move |client| {
            flow_node_runner(client, node_registry.clone())
        })
        .register_builtin_plugin(manifest)
        .register_builtin_runner(move || {
            Box::new(CaptureRunner {
                descriptor: runner_descriptor.clone(),
                events: runner_events.clone(),
                notify: runner_notify.clone(),
            })
        })
        .start()
        .await
        .unwrap();

    let capture_result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ready = {
                let captured = events.lock().unwrap();
                [
                    "event-private",
                    "event-group",
                    "event-channel",
                    "event-channel-delete",
                ]
                .into_iter()
                .all(|event_id| captured.iter().any(|event| event.event_id == event_id))
            };
            if ready {
                break;
            }
            notify.notified().await;
        }
    })
    .await;
    assert!(
        capture_result.is_ok(),
        "private, group, channel and delete events should reach the business runner; captured={:?}; tasks={:#?}",
        events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        runtime.task_snapshots()
    );

    let captured = events.lock().unwrap().clone();
    assert!(
        captured
            .iter()
            .all(|event| event.event_id != "event-group-replayed"),
        "the replayed group message must be suppressed"
    );
    let private = event_by_id(&captured, "event-private");
    assert_eq!(
        private.target,
        BotTarget::User {
            user_id: "private-user".into()
        }
    );
    assert_eq!(private.actor.as_ref().unwrap().user_id, "private-user");
    let group = event_by_id(&captured, "event-group");
    assert_eq!(
        group.target,
        BotTarget::Group {
            group_id: "group".into()
        }
    );
    assert_eq!(group.actor.as_ref().unwrap().user_id, "group-user");
    assert_eq!(group.ext["qqbot.mentioned_bot"], Value::Bool(true));
    let channel = event_by_id(&captured, "event-channel");
    assert_eq!(
        channel.target,
        BotTarget::GuildChannel {
            guild_id: "guild".into(),
            channel_id: "channel".into()
        }
    );
    assert_eq!(channel.actor.as_ref().unwrap().user_id, "channel-user");
    assert_eq!(channel.ext["qqbot.sequence"], Value::from(4));
    assert_eq!(
        channel.message.as_ref().unwrap().reply_to.as_deref(),
        Some("quoted-message")
    );
    assert!(channel.message.as_ref().unwrap().segments.iter().any(
        |segment| matches!(segment, MessageSegment::Quote { message_id } if message_id == "quoted-message")
    ));
    assert_eq!(
        event_by_id(&captured, "event-channel-delete").kind,
        BotEventKind::MessageDeleted
    );

    runtime.shutdown().await;
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
}

fn event_by_id<'a>(events: &'a [BotEvent], event_id: &str) -> &'a BotEvent {
    events
        .iter()
        .find(|event| event.event_id == event_id)
        .unwrap_or_else(|| panic!("missing event {event_id}"))
}

fn gateway_event(sequence: u64, event_id: &str, event_type: &str, data: Value) -> Value {
    json!({
        "op": 0,
        "s": sequence,
        "t": event_type,
        "id": event_id,
        "d": data
    })
}

fn product_toml(
    root: &std::path::Path,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    format!(
        r#"[service]
profile = "issue141"
instance_id = "issue141"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "tcp-debug"
name = "issue141"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[[plugins.configured]]
id = "mutsuki.std.resource.sqlite"

[plugins.configured.config]
database_path = "{}"

[[plugins.configured]]
id = "mutsuki.bot.adapter.qqbot"
[plugins.configured.config]
account_id = "{}"
app_id = "{}"
client_secret_key = "{}"
token_url = "{}"
openapi_base_url = "{}"
allow_insecure_transport = true
gateway_hello_timeout_ms = 1000
gateway_ack_timeout_ms = 500
retry_base_delay_ms = 0
retry_max_delay_ms = 0
reconnect_initial_delay_ms = 10
reconnect_max_delay_ms = 20
reconnect_jitter_ms = 0

[[plugins.configured]]
id = "{CAPTURE_PLUGIN_ID}"

[[plugins.configured]]
id = "mutsuki.bot.router.flow"

[security]
secret_file = "local.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        root.join("resources.sqlite")
            .to_string_lossy()
            .replace('\\', "/"),
        qq.account_id,
        qq.app_id,
        qq.client_secret_key,
        qq.token_url,
        qq.openapi_base_url,
    )
}

fn capture_port(port_id: &str, event_type: &str) -> BotNodePortDescriptor {
    BotNodePortDescriptor {
        port_id: port_id.into(),
        title: port_id.into(),
        direction: BotNodePortDirection::Input,
        event_type: BotFlowTypeRef::new(event_type, 1),
        required: false,
    }
}

fn capture_source_nodes() -> Vec<BotFlowNode> {
    [
        (QQ_NODE_MESSAGE_CREATED, "message"),
        (QQ_NODE_MESSAGE_UPDATED, "updated"),
        (QQ_NODE_MESSAGE_DELETED, "deleted"),
        (QQ_NODE_REACTION_ADDED, "reaction-add"),
        (QQ_NODE_REACTION_REMOVED, "reaction-remove"),
        (QQ_NODE_MEMBER_JOINED, "joined"),
        (QQ_NODE_MEMBER_LEFT, "left"),
        (QQ_NODE_BOT_CONNECTED, "connected"),
        (QQ_NODE_BOT_DISCONNECTED, "disconnected"),
    ]
    .into_iter()
    .map(|(node_type_id, node_id)| BotFlowNode {
        node_id: node_id.into(),
        node_type_id: node_type_id.into(),
        node_type_version: 1,
        config: json!({}),
        source: Some(BotFlowSourceSelector {
            protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
        }),
        position: BotFlowNodePosition::default(),
    })
    .collect()
}

fn capture_source_edges() -> Vec<BotFlowEdge> {
    [
        ("message", "event"),
        ("updated", "event"),
        ("deleted", "deleted"),
        ("reaction-add", "reaction"),
        ("reaction-remove", "reaction"),
        ("joined", "member"),
        ("left", "member"),
        ("connected", "lifecycle"),
        ("disconnected", "lifecycle"),
    ]
    .into_iter()
    .map(|(from, to_port)| BotFlowEdge {
        edge_id: format!("{from}-capture"),
        from_node_id: from.into(),
        from_port_id: "event".into(),
        to_node_id: "capture".into(),
        to_port_id: to_port.into(),
        kind: BotFlowEdgeKind::Event,
    })
    .collect()
}

fn capture_flow_registry(
    capture_manifest: &mutsuki_runtime_contracts::PluginManifest,
) -> Arc<BotFlowRegistry> {
    let catalog = BotNodeCatalog::from_manifests(&[
        qqbot_adapter_manifest(1, false),
        flow_router_manifest(),
        capture_manifest.clone(),
    ])
    .unwrap();
    Arc::new(
        BotFlowRegistry::with_snapshot(
            catalog,
            BotFlowSnapshot {
                revision: 1,
                flow: BotFlowDocument {
                    flow_id: "issue141.capture".into(),
                    name: "capture all QQ events".into(),
                    nodes: {
                        let mut nodes = capture_source_nodes();
                        nodes.push(BotFlowNode {
                            node_id: "capture".into(),
                            node_type_id: CAPTURE_PLUGIN_ID.into(),
                            node_type_version: 1,
                            config: json!({}),
                            source: None,
                            position: BotFlowNodePosition::default(),
                        });
                        nodes
                    },
                    edges: capture_source_edges(),
                },
            },
        )
        .unwrap(),
    )
}
