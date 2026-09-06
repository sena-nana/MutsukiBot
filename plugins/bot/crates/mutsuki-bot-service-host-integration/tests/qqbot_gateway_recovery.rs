use std::time::Duration;

use mutsuki_bot_service_host_integration::{
    DEFAULT_MEDIA_PROVIDER_ID, configured_bot_plugin_catalog,
};
use mutsuki_bot_testkit::{FakeQqGatewayScript, FakeQqIdentifyOutcome, FakeQqServer};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntimeBuilder;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_recovery_chooses_identify_or_resume() {
    for (after_identify, expected_op) in [
        (FakeQqIdentifyOutcome::ReadyThenClose(4006), 2),
        (FakeQqIdentifyOutcome::ReadyThenClose(4009), 6),
        (FakeQqIdentifyOutcome::ReadyThenReconnectOpcode, 6),
        (FakeQqIdentifyOutcome::RejectIdentify, 2),
    ] {
        let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
            initial_events: Vec::new(),
            resumed_events: Vec::new(),
            after_identify,
            ..FakeQqGatewayScript::default()
        })
        .await;
        let secret_key = format!("QQBOT_RECOVERY_SECRET_{}", fake.websocket_addr().port());
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("local.secret.toml"),
            format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
        )
        .unwrap();
        let qq = fake.config("recovery", "TEST_APP_ID", &secret_key);
        let config_path = root.path().join("local.toml");
        std::fs::write(&config_path, product_toml(root.path(), &qq)).unwrap();
        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path),
            ..Default::default()
        })
        .unwrap();
        let runtime = {
            let mut catalog =
                mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap();
            catalog
                .merge(
                    configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap(),
                )
                .unwrap();
            ServiceRuntimeBuilder::new(config)
                .with_configured_plugin_catalog(catalog)
                .start()
                .await
                .unwrap()
        };
        let frames = fake.wait_for_auth_frames(2, Duration::from_secs(3)).await;
        assert_eq!(frames[0]["op"], 2, "{after_identify:?}");
        assert_eq!(frames[1]["op"], expected_op, "{after_identify:?}");
        runtime.shutdown().await;
        fake.shutdown().await;
    }
}

fn product_toml(
    root: &std::path::Path,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    format!(
        r#"[service]
profile = "gateway-recovery"
instance_id = "gateway-recovery-{}"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "tcp-debug"
name = "gateway-recovery"
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

[security]
secret_file = "local.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        qq.client_secret_key,
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
