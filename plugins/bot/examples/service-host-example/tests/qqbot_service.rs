// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::match_wildcard_for_single_variants,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::time::Duration;

use bot_echo::{echo_manifest, echo_runner};
use mutsuki_bot_flow::{BotFlowRegistry, BotNodeCatalog};
use mutsuki_bot_protocol::BotFlowSnapshot;
use mutsuki_bot_service_host_integration::{
    BotFlowRouterConfiguredPlugin, DEFAULT_MEDIA_PROVIDER_ID, configured_bot_plugin_catalog,
};
use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_config_service::{ConfigProviderRegistry, ConfigService, InMemoryConfigRepository};
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
use mutsuki_plugin_bot_command::BOT_COMMAND_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::BOT_FLOW_ROUTER_PLUGIN_ID;
use mutsuki_service_config::{
    ConfigOverrides, ConfiguredPluginSelection, IpcTransport, ServiceConfig,
};
use mutsuki_service_control::{ControlCommand, ControlResponse, ControlResult, HealthReport};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[tokio::test]
async fn configured_qqbot_requires_host_secret_during_service_preflight() {
    let mut service = test_service_config().await.0;
    service.plugins.configured = vec![configured_qq("MISSING_QQBOT_SECRET", json!({}))];
    let error = match ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(
            configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap(),
        )
        .start()
        .await
    {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("QQBot ServiceRuntime unexpectedly started without Host secret")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("MISSING_QQBOT_SECRET"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configured_service_runtime_runs_resume_echo_ping_and_clean_shutdown() {
    let fake = FakeQqServer::start().await;
    let secret_key = format!("QQBOT_TEST_SECRET_{}", fake.websocket_addr().port());
    let home = tempfile::tempdir().unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);
    let qq = fake.config("integration", "TEST_APP_ID", &secret_key);
    std::fs::write(
        home.path().join("local.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let config_path = home.path().join("local.toml");
    std::fs::write(&config_path, product_toml(home.path(), address, &qq)).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();
    let flow_registry = example_flow_registry();
    let config = std::sync::Arc::new(
        ConfigService::new(
            std::sync::Arc::new(ConfigProviderRegistry::default()),
            std::sync::Arc::new(InMemoryConfigRepository::default()),
        )
        .unwrap(),
    );
    let mut catalog = configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap();
    catalog
        .merge(mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap())
        .unwrap();
    catalog
        .register(BotFlowRouterConfiguredPlugin::with_registry(
            config,
            flow_registry,
        ))
        .unwrap();
    let control_config = service.clone();

    let runtime = ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(catalog)
        .register_builtin_plugin(echo_manifest(1))
        .register_builtin_runner(|| echo_runner(1))
        .start()
        .await
        .unwrap();

    let sends = fake.wait_for_sends(2, Duration::from_secs(5)).await;
    assert_eq!(sends[0]["content"], "hello");
    assert_eq!(sends[1]["content"], "pong");

    let ControlResult::PluginList(plugins) =
        control(&control_config, ControlCommand::PluginList).await
    else {
        panic!("unexpected plugin response");
    };
    let plugin_json = serde_json::to_string(&plugins).unwrap();
    for id in [
        QQBOT_ADAPTER_PLUGIN_ID,
        BOT_FLOW_ROUTER_PLUGIN_ID,
        BOT_COMMAND_PLUGIN_ID,
        "example.bot.echo",
    ] {
        assert!(plugin_json.contains(id), "missing configured plugin {id}");
    }
    let mut last_health: Option<HealthReport> = None;
    let health = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ControlResult::HealthCheck(health) =
                control(&control_config, ControlCommand::HealthCheck).await
            else {
                panic!("unexpected health response");
            };
            let ready = health.event_sources == "ok";
            last_health = Some(health);
            if ready {
                break last_health.clone().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("QQ Gateway health becomes ready; last health: {last_health:?}"));
    assert_eq!(health.service, "ok");
    assert_eq!(health.event_sources, "ok");
    assert_eq!(
        health.components["mutsuki.bot.qqbot.gateway:integration"]["connected"],
        true
    );
    let ControlResult::TaskList(tasks) = control(&control_config, ControlCommand::TaskList).await
    else {
        panic!("unexpected task list response");
    };
    let task_json = serde_json::to_string(&tasks).unwrap();
    assert!(task_json.contains("echo-1"));
    assert!(task_json.contains("ping-1"));
    assert!(!task_json.contains("TEST_CLIENT_SECRET"));
    assert!(!task_json.contains("TEST_ACCESS_TOKEN"));

    assert!(matches!(
        control(&control_config, ControlCommand::ServiceShutdown).await,
        ControlResult::ServiceShutdown
    ));
    runtime
        .run_until_shutdown_signal(std::future::pending::<String>())
        .await
        .unwrap();
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert!(snapshot.account_checks >= 2);
    assert_eq!(snapshot.clean_closes, 1);
}

fn product_toml(
    root: &std::path::Path,
    ipc_addr: std::net::SocketAddr,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    format!(
        r#"[service]
profile = "qqbot-fake"
instance_id = "qqbot-fake"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = true
transport = "tcp-debug"
name = "qqbot-fake"
tcp_debug_addr = "{}"
token = "test-token"

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
id = "example.bot.echo"
[plugins.configured.config]

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

[security]
secret_file = "local.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        ipc_addr,
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

fn example_flow_registry() -> std::sync::Arc<BotFlowRegistry> {
    let catalog = BotNodeCatalog::from_manifests(&qqbot_echo::qqbot_echo_manifests()).unwrap();
    std::sync::Arc::new(
        BotFlowRegistry::with_snapshot(
            catalog,
            BotFlowSnapshot {
                revision: 1,
                flow: qqbot_echo::qqbot_echo_and_ping_flow(),
            },
        )
        .unwrap(),
    )
}

fn configured_qq(secret_key: &str, overrides: Value) -> ConfiguredPluginSelection {
    let mut config = serde_json::to_value(mutsuki_plugin_bot_adapter_qqbot::QqBotConfig::new(
        "configured",
        "TEST_APP_ID",
    ))
    .unwrap();
    config["client_secret_key"] = Value::String(secret_key.into());
    if let (Some(base), Some(overrides)) = (config.as_object_mut(), overrides.as_object()) {
        base.extend(overrides.clone());
    }
    ConfiguredPluginSelection {
        id: QQBOT_ADAPTER_PLUGIN_ID.into(),
        enabled: true,
        config,
    }
}

async fn test_service_config() -> (ServiceConfig, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);
    let mut service = ServiceConfig::default();
    service.service.instance_id = "qqbot-integration".into();
    service.service.home_dir = home.path().to_path_buf();
    service.service.log_dir = home.path().join("logs");
    service.service.run_dir = home.path().join("run");
    service.plugins.dynamic_dirs.clear();
    service.plugins.disabled_dir = home.path().join("disabled");
    service.observe.console = false;
    service.ipc.enabled = true;
    service.ipc.transport = IpcTransport::TcpDebug;
    service.ipc.tcp_debug_addr = Some(address.to_string());
    service.ipc.token = Some("test-token".into());
    std::fs::create_dir_all(&service.service.log_dir).unwrap();
    std::fs::create_dir_all(&service.service.run_dir).unwrap();
    (service, home)
}

async fn control(config: &ServiceConfig, command: ControlCommand) -> ControlResult {
    let client = mutsuki_service_ipc::ControlClient::new(config.into());
    match client.request(command).await.unwrap() {
        ControlResponse::Ok(result) => result,
        response => panic!("control failed: {response:?}"),
    }
}
