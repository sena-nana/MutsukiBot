use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use mutsuki_bot_management::{
    BilibiliBindChallengeResult, BilibiliBindVerifyResult, BilibiliCredentialSecretState,
    BilibiliLoginPollResult, BilibiliLoginSession, BilibiliLoginStartResult, BilibiliManagementApi,
    BilibiliManagementError, BilibiliManagementStatus, BilibiliNotificationKind,
    BilibiliPreviewCardView, BilibiliQrLoginStatus, BilibiliSubscriptionView,
};
use mutsuki_bot_protocol::BotTarget;
use mutsuki_plugin_bot_bilibili_web::*;
use mutsuki_web_extension_api::{WebExtension, content_hash};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Default)]
struct Api {
    clears: Mutex<u32>,
    unsubs: Mutex<u32>,
}

fn unused() -> BilibiliManagementError {
    BilibiliManagementError {
        code: "bilibili.management_unavailable".into(),
        message: "unused".into(),
    }
}

#[async_trait]
impl BilibiliManagementApi for Api {
    fn status(&self) -> BilibiliManagementStatus {
        BilibiliManagementStatus {
            backend: "web_cookie".into(),
            management_enabled: true,
            allow_self_binding: false,
            cookie_secret_key: Some("BILIBILI_COOKIE".into()),
            cookie_secret_state: Some(BilibiliCredentialSecretState::Present),
            credential_loaded: true,
            subscription_count: 0,
            reason: None,
            push_wired: Some(true),
        }
    }

    fn login_start_session(
        &self,
        _actor_id: &str,
    ) -> Result<BilibiliLoginSession, BilibiliManagementError> {
        Ok(BilibiliLoginSession {
            url: "https://passport.bilibili.com/qr".into(),
            key: "secret-qr-key".into(),
        })
    }

    async fn login_start(
        &self,
        _actor_id: &str,
    ) -> Result<BilibiliLoginStartResult, BilibiliManagementError> {
        Ok(BilibiliLoginStartResult {
            url: "https://passport.bilibili.com/qr".into(),
            key: "secret-qr-key".into(),
            qr_png: vec![],
            qr_png_base64: "cWFy".into(),
        })
    }

    fn login_poll(
        &self,
        _actor_id: &str,
    ) -> Result<BilibiliLoginPollResult, BilibiliManagementError> {
        Ok(BilibiliLoginPollResult {
            status: BilibiliQrLoginStatus::Pending,
            message: "waiting".into(),
        })
    }

    fn credential_clear(&self) -> Result<(), BilibiliManagementError> {
        *self.clears.lock().unwrap() += 1;
        Ok(())
    }

    fn list(
        &self,
        _actor_id: &str,
        _is_admin: bool,
    ) -> Result<Vec<BilibiliSubscriptionView>, BilibiliManagementError> {
        Ok(Vec::new())
    }

    fn subscribe(
        &self,
        _subscription_id: String,
        _uid: u64,
        _notifications: Vec<BilibiliNotificationKind>,
        _target: BotTarget,
        _outbound_binding: String,
    ) -> Result<BilibiliSubscriptionView, BilibiliManagementError> {
        Err(unused())
    }

    fn unsubscribe(&self, _subscription_id: &str) -> Result<(), BilibiliManagementError> {
        *self.unsubs.lock().unwrap() += 1;
        Ok(())
    }

    fn set_paused(
        &self,
        _actor_id: &str,
        _is_admin: bool,
        _selector: Option<&str>,
        _paused: bool,
    ) -> Result<BilibiliSubscriptionView, BilibiliManagementError> {
        Err(unused())
    }

    fn preview(
        &self,
        _actor_id: &str,
        _is_admin: bool,
        _selector: Option<&str>,
    ) -> Result<BilibiliPreviewCardView, BilibiliManagementError> {
        Err(unused())
    }

    fn bind_start(
        &self,
        _operator_user_id: &str,
        _uid: u64,
        _challenge_seed: &str,
    ) -> Result<BilibiliBindChallengeResult, BilibiliManagementError> {
        Err(unused())
    }

    fn bind_verify(
        &self,
        _operator_user_id: &str,
        _platform: &str,
        _target: BotTarget,
    ) -> Result<BilibiliBindVerifyResult, BilibiliManagementError> {
        Err(unused())
    }

    fn unbind(&self, _operator_user_id: &str) -> Result<bool, BilibiliManagementError> {
        Err(unused())
    }
}

#[tokio::test]
async fn bilibili_management_rpc_strips_qr_secrets_and_requires_confirmation() {
    let api = Arc::new(Api::default());
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let extension = BilibiliWebExtension::new(api.clone()).with_frontend_assets(&assets);
    // The manifest is what the Host serves and what the client integrity-checks, so it — not the
    // bundle's rendered copy — is the contract this test can hold.
    let descriptor = extension.descriptor();
    let entry = descriptor
        .assets
        .iter()
        .find(|asset| asset.path == descriptor.entry)
        .expect("manifest declares its entry asset");
    let bytes = std::fs::read(assets.join(&descriptor.entry)).unwrap();
    assert_eq!(entry.bytes, bytes.len() as u64);
    assert_eq!(entry.content_hash, content_hash(&bytes));
    std::fs::write(
        shell_dir.path().join("index.html"),
        "<!doctype html><main></main>",
    )
    .unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.bilibili".into(),
                name: "Bilibili".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: shell_dir.path().into(),
                index_file: "index.html".into(),
                import_map: serde_json::Map::default(),
            },
        ))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .extension(extension)
        .auth_token("local-dev")
        .build()
        .unwrap();
    host.start().await.unwrap();
    let address = host.listen_addr().unwrap().to_string();

    assert_eq!(
        rpc(&address, "status", json!({})).await.unwrap()["backend"],
        "web_cookie"
    );

    let started = rpc(&address, "login.start", json!({})).await.unwrap();
    assert_eq!(started["qr_png_base64"], "cWFy");
    assert!(started.get("url").is_none());
    assert!(started.get("key").is_none());
    assert!(!started.to_string().contains("secret-qr-key"));

    let missing = rpc(
        &address,
        "subscriptions.subscribe",
        json!({
            "subscription_id": "sub-1",
            "uid": 7,
            "outbound_binding": "qq-main"
        }),
    )
    .await
    .unwrap_err();
    assert!(missing.contains("bilibili.invalid_argument"));

    assert!(
        rpc(&address, "credential.clear", json!({}))
            .await
            .unwrap_err()
            .contains("bilibili.confirmation_required")
    );
    assert_eq!(*api.clears.lock().unwrap(), 0);
    rpc(&address, "credential.clear", json!({ "confirmed": true }))
        .await
        .unwrap();
    assert_eq!(*api.clears.lock().unwrap(), 1);

    assert!(
        rpc(
            &address,
            "subscriptions.unsubscribe",
            json!({ "subscription_id": "sub-1" }),
        )
        .await
        .unwrap_err()
        .contains("bilibili.confirmation_required")
    );
    assert_eq!(*api.unsubs.lock().unwrap(), 0);
    rpc(
        &address,
        "subscriptions.unsubscribe",
        json!({ "subscription_id": "sub-1", "confirmed": true }),
    )
    .await
    .unwrap();
    assert_eq!(*api.unsubs.lock().unwrap(), 1);

    host.stop().await.unwrap();
}

async fn rpc(address: &str, method: &str, params: Value) -> Result<Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut socket, _) = connect_async(format!("ws://{address}/ws"))
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(Message::Binary(
            WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: Vec::new(),
                auth_token: Some("local-dev".into()),
            }
            .encode()
            .unwrap()
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let Message::Binary(bytes) = socket
        .next()
        .await
        .ok_or_else(|| "missing hello ack".to_string())?
        .map_err(|error| error.to_string())?
    else {
        return Err("unexpected hello ack".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::HelloAck { .. } => {}
        _ => return Err("unexpected hello ack".into()),
    }
    socket
        .send(Message::Binary(
            WireMessage::Rpc(RpcRequest {
                id: Uuid::new_v4(),
                namespace: PLUGIN_ID.into(),
                method: method.into(),
                params,
            })
            .encode()
            .unwrap()
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let Message::Binary(bytes) = socket
        .next()
        .await
        .ok_or_else(|| "missing response".to_string())?
        .map_err(|error| error.to_string())?
    else {
        return Err("unexpected response".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(format!("{}: {}", error.code, error.message)),
            None => Ok(result.result.unwrap_or(Value::Null)),
        },
        _ => Err("unexpected wire message".into()),
    }
}
