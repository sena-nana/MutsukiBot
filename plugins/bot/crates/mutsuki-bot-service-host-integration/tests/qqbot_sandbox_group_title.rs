use std::time::{Duration, Instant};

use mutsuki_bot_protocol::BotConversationKind;
use mutsuki_bot_sandbox::{
    SandboxAction, SandboxApi, SandboxMode, SandboxService, SandboxWriteRequest,
};
use mutsuki_bot_service_host_integration::{
    DEFAULT_MEDIA_PROVIDER_ID, SANDBOX_SERVICE_ID, configured_bot_plugin_catalog,
};
use mutsuki_bot_testkit::{FakeQqGatewayScript, FakeQqServer};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_sandbox_group_title_comes_from_group_info() {
    let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
        initial_events: vec![json!({
            "op": 0,
            "s": 2,
            "t": "GROUP_AT_MESSAGE_CREATE",
            "id": "group-title-event",
            "d": {
                "id": "group-title-message",
                "group_openid": "GROUP_1",
                "content": "<@BOT_OPENID> ping",
                "mentions": [{"id": "BOT_OPENID", "is_you": true, "bot": true}],
                "timestamp": "2026-07-12T10:00:00+08:00",
                "author": {"member_openid": "USER_1", "username": "tester"}
            }
        })],
        resumed_events: Vec::new(),
        close_delay: Duration::from_secs(2),
        ..FakeQqGatewayScript::default()
    })
    .await;
    let secret_key = format!("QQBOT_GROUP_TITLE_SECRET_{}", fake.websocket_addr().port());
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("local.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let qq = fake.config("group-title", "TEST_APP_ID", &secret_key);
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
            .merge(configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap())
            .unwrap();
        ServiceRuntimeBuilder::new(config)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .unwrap()
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut title = None;
    while Instant::now() < deadline {
        if let Ok(sandbox) = runtime.host_service::<SandboxService>(SANDBOX_SERVICE_ID) {
            let snapshot = sandbox.snapshot("").await.unwrap();
            if snapshot.mode != SandboxMode::Live {
                let _ = sandbox
                    .write(
                        "test",
                        SandboxWriteRequest {
                            operation_id: format!("live-{}", snapshot.revision),
                            expected_revision: snapshot.revision,
                            action: SandboxAction::SetMode {
                                mode: SandboxMode::Live,
                            },
                        },
                    )
                    .await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            title = snapshot
                .conversations
                .iter()
                .find(|item| item.kind == BotConversationKind::Group)
                .map(|item| item.title.clone());
            if title.as_deref() == Some("测试群") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    runtime.shutdown().await;
    fake.shutdown().await;
    assert_eq!(title.as_deref(), Some("测试群"));
}

fn product_toml(
    root: &std::path::Path,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    format!(
        r#"[service]
profile = "group-title"
instance_id = "group-title-{}"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "tcp-debug"
name = "group-title"
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
