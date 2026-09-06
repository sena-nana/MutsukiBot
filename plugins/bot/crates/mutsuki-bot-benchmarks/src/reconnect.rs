use std::time::{Duration, Instant};

use bot_echo::{echo_manifest, echo_runner};
use mutsuki_bot_flow::{BotFlowRegistry, BotNodeCatalog};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BotFlowDocument, BotFlowEdge,
    BotFlowEdgeKind, BotFlowNode, BotFlowNodePosition, BotFlowSnapshot, BotFlowSourceSelector,
    BotFlowTypeRef,
};
use mutsuki_bot_service_host_integration::{
    BotFlowRouterConfiguredPlugin, DEFAULT_MEDIA_PROVIDER_ID, configured_bot_plugin_catalog,
};
use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_config_service::{ConfigProviderRegistry, ConfigService, InMemoryConfigRepository};
use mutsuki_plugin_bot_adapter_qqbot::tasks::qqbot_adapter_manifest;
use mutsuki_plugin_bot_command::{BOT_COMMAND_MATCH_NODE_TYPE_ID, bot_command_manifest};
use mutsuki_plugin_bot_event_router::flow_router_manifest;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

use crate::measurement::{Sample, allocation_delta, allocation_snapshot, process_cpu_time_ns};

pub fn reconnect_sample() -> Sample {
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let run = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_connection_workload(None));
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 10_000_000,
        events: 2,
        queue_depth: 1,
        dropped: 0,
        deferred: 1,
        retried: 1,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 0,
        output: run.output,
        allocations,
        allocated_bytes,
    }
}

pub fn connection_idle_sample(idle_window_ms: u64) -> Sample {
    let run = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_connection_workload(Some(Duration::from_millis(
            idle_window_ms,
        ))));
    Sample {
        elapsed_ns: run.idle_elapsed_ns,
        // Idle case owns its CPU boundary: only the post-resume idle window counts.
        // The outer harness must not wrap setup/reconnect/shutdown into cpu_time_ns.
        cpu_time_ns: run.idle_cpu_time_ns,
        idle_cpu_time_ns: run.idle_cpu_time_ns,
        simulated_platform_ns: u128::from(idle_window_ms) * 1_000_000,
        events: 0,
        queue_depth: 0,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 1,
        output: json!({
            "idle_window_ms": idle_window_ms,
            "connections": run.output["connections"],
            "auth_ops": run.output["auth_ops"],
            "clean_closes": run.output["clean_closes"]
        }),
        allocations: run.idle_allocations,
        allocated_bytes: run.idle_allocated_bytes,
    }
}

struct ConnectionRun {
    output: serde_json::Value,
    idle_elapsed_ns: u128,
    idle_cpu_time_ns: u128,
    idle_allocations: u64,
    idle_allocated_bytes: u64,
}

async fn run_connection_workload(idle_window: Option<Duration>) -> ConnectionRun {
    let fake = FakeQqServer::start().await;
    let secret_key = format!("QQBOT_BENCHMARK_SECRET_{}", fake.websocket_addr().port());
    let home = tempfile::tempdir().unwrap();
    let qq = fake.config("benchmark", "BENCHMARK_APP", &secret_key);
    std::fs::write(
        home.path().join("local.secret.toml"),
        format!("[secrets]\n{secret_key} = \"BENCHMARK_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let config_path = home.path().join("local.toml");
    std::fs::write(&config_path, product_toml(home.path(), &qq)).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();
    let flow_registry = echo_flow_registry();
    let config = std::sync::Arc::new(
        ConfigService::new(
            std::sync::Arc::new(ConfigProviderRegistry::default()),
            std::sync::Arc::new(InMemoryConfigRepository::default()),
        )
        .unwrap(),
    );
    let mut catalog = configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap();
    catalog
        .register(BotFlowRouterConfiguredPlugin::with_registry(
            config,
            flow_registry,
        ))
        .unwrap();
    let runtime = ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(catalog)
        .register_builtin_plugin(echo_manifest(1))
        .register_builtin_runner(|| echo_runner(1))
        .start()
        .await
        .unwrap();
    let sends = fake.wait_for_sends(2, Duration::from_secs(5)).await;
    let idle_allocation_start = allocation_snapshot();
    let idle_cpu_start = process_cpu_time_ns();
    let idle_started = Instant::now();
    if let Some(idle_window) = idle_window {
        tokio::time::sleep(idle_window).await;
    }
    let idle_elapsed_ns = idle_started.elapsed().as_nanos();
    let idle_cpu_time_ns = process_cpu_time_ns().saturating_sub(idle_cpu_start);
    let (idle_allocations, idle_allocated_bytes) = allocation_delta(idle_allocation_start);
    runtime.shutdown().await;
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
    let output = json!({
        "sends": sends
            .iter()
            .map(|send| send["content"].clone())
            .collect::<Vec<_>>(),
        "connections": snapshot.websocket_connections,
        "auth_ops": snapshot
            .gateway_auth_frames
            .iter()
            .map(|frame| frame["op"].clone())
            .collect::<Vec<_>>(),
        "account_checks_at_least_two": snapshot.account_checks >= 2,
        "clean_closes": snapshot.clean_closes
    });
    ConnectionRun {
        output,
        idle_elapsed_ns,
        idle_cpu_time_ns,
        idle_allocations,
        idle_allocated_bytes,
    }
}

fn product_toml(
    root: &std::path::Path,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    format!(
        r#"[service]
profile = "bot-benchmark"
instance_id = "bot-benchmark"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "tcp-debug"
name = "bot-benchmark"
token = "benchmark-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[[plugins.configured]]
id = "mutsuki.bot.router.flow"
[plugins.configured.config]

[[plugins.configured]]
id = "mutsuki.bot.command"
[plugins.configured.config]

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
id = "example.bot.echo"

[security]
secret_file = "local.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        qq.account_id,
        qq.app_id,
        qq.client_secret_key,
        qq.token_url,
        qq.openapi_base_url,
    )
}

fn echo_flow_registry() -> std::sync::Arc<BotFlowRegistry> {
    let catalog = BotNodeCatalog::from_manifests(&[
        qqbot_adapter_manifest(1, false),
        flow_router_manifest(),
        bot_command_manifest(1),
        echo_manifest(1),
    ])
    .unwrap();
    std::sync::Arc::new(
        BotFlowRegistry::with_snapshot(
            catalog,
            BotFlowSnapshot {
                revision: 1,
                flow: echo_and_ping_flow(),
            },
        )
        .unwrap(),
    )
}

fn echo_and_ping_flow() -> BotFlowDocument {
    BotFlowDocument {
        flow_id: "benchmark.qq.commands".into(),
        name: "QQ echo/ping".into(),
        nodes: vec![
            flow_node(
                "source",
                mutsuki_plugin_bot_adapter_qqbot::tasks::QQ_NODE_MESSAGE_CREATED,
                json!({}),
                Some(BotFlowSourceSelector {
                    protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                    event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
                }),
            ),
            command_node(
                "echo-command",
                "echo",
                json!([{
                    "name": "text",
                    "kind": "string",
                    "optional": false,
                    "variadic": true
                }]),
            ),
            flow_node("echo", "example.bot.echo", json!({}), None),
            flow_node("echo-send", "mutsuki.bot.qq.send", json!({}), None),
            command_node("ping-command", "ping", json!([])),
            flow_node("ping", "example.bot.ping", json!({}), None),
            flow_node("ping-send", "mutsuki.bot.qq.send", json!({}), None),
        ],
        edges: vec![
            flow_edge("source-echo", "source", "event", "echo-command", "event"),
            flow_edge(
                "echo-command-echo",
                "echo-command",
                "matched",
                "echo",
                "command",
            ),
            flow_edge("echo-send", "echo", "message", "echo-send", "input"),
            flow_edge("source-ping", "source", "event", "ping-command", "event"),
            flow_edge(
                "ping-command-ping",
                "ping-command",
                "matched",
                "ping",
                "command",
            ),
            flow_edge("ping-send", "ping", "message", "ping-send", "input"),
        ],
    }
}

fn command_node(node_id: &str, command: &str, arguments: serde_json::Value) -> BotFlowNode {
    flow_node(
        node_id,
        BOT_COMMAND_MATCH_NODE_TYPE_ID,
        json!({
            "prefixes": ["/"],
            "path": [command],
            "aliases": [],
            "arguments": arguments
        }),
        None,
    )
}

fn flow_node(
    node_id: &str,
    node_type_id: &str,
    config: serde_json::Value,
    source: Option<BotFlowSourceSelector>,
) -> BotFlowNode {
    BotFlowNode {
        node_id: node_id.into(),
        node_type_id: node_type_id.into(),
        node_type_version: 1,
        config,
        source,
        position: BotFlowNodePosition::default(),
    }
}

fn flow_edge(
    edge_id: &str,
    from_node_id: &str,
    from_port_id: &str,
    to_node_id: &str,
    to_port_id: &str,
) -> BotFlowEdge {
    BotFlowEdge {
        edge_id: edge_id.into(),
        from_node_id: from_node_id.into(),
        from_port_id: from_port_id.into(),
        to_node_id: to_node_id.into(),
        to_port_id: to_port_id.into(),
        kind: BotFlowEdgeKind::Event,
    }
}
