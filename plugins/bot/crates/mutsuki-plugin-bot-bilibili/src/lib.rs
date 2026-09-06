// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    clippy::default_trait_access,
    clippy::if_not_else,
    clippy::manual_string_new,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_fields_in_debug,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::needless_update,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::unnecessary_wraps,
    clippy::unreadable_literal,
    clippy::useless_vec
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_link_parser::{MAX_LINK_CARD_MEDIA_BYTES, ResolvedLinkCard, preferred_event_url};
#[cfg(test)]
use mutsuki_bot_management::BilibiliCredentialSecretState;
use mutsuki_bot_management::{
    BilibiliBindVerifyResult, BilibiliManagementApi, BilibiliManagementError,
    BilibiliNotificationKind,
};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BotCommandEvent, BotEvent, BotExtMap, BotFlowContext, BotFlowEventEnvelope, BotFlowPayload,
    BotFlowTypeRef, BotMessage, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodeInvocation, BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult,
    BotNodeRole, BotTarget, MessageSegment,
};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_protocol_image::{
    CARD_RENDER, CardGradient, CardLayout, CardRenderRequest, ImageRenderResponse, QR_RENDER,
    QrRenderRequest, Rgba,
};
#[cfg(test)]
use mutsuki_runtime_contracts::SurfaceRequirement;
use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, ExecutionClass, ProtocolClass, ReadPlan, RunnerBatchCapability,
    RunnerContext, RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect,
    RuntimeError, ScalarValue, Task, TaskBatch, TaskOutcome, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway,
    RunnerDescriptorBuilder, RuntimeClientRef, TaskAwaitRunnerAdapter, TaskHandleFuture,
};
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

mod config;
mod management;
mod open_platform;
mod secure_media;

pub use config::{
    BILIBILI_APP_SECRET_FIELD, BILIBILI_APP_SECRET_KEY, BILIBILI_COOKIE_FIELD, BILIBILI_COOKIE_KEY,
    BILIBILI_OAUTH_CREDENTIAL_FIELD, BILIBILI_OAUTH_CREDENTIAL_KEY, bilibili_config_descriptor,
    bilibili_config_value,
};
pub use management::{BilibiliManagementService, BilibiliQrRenderer, BilibiliSecretPresence};
pub use open_platform::{
    BilibiliOpenPlatformCredential, BilibiliOpenPlatformHttpClient,
    BilibiliOpenPlatformHttpRequest, BilibiliOpenPlatformHttpResponse,
    BilibiliOpenPlatformRequestContext, OpenPlatformHttpMethod,
    ReqwestBilibiliOpenPlatformTransport, open_platform_signed_headers,
};

pub const PLUGIN_ID: &str = "mutsuki.bot.bilibili";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.runner";
pub const POLL_LIVE: &str = "mutsuki.bot.bilibili.poll/live@1";
pub const POLL_DYNAMIC: &str = "mutsuki.bot.bilibili.poll/dynamic@1";
pub const POLL_VIDEO: &str = "mutsuki.bot.bilibili.poll/video@1";
pub const NOTIFY_CARD: &str = "mutsuki.bot.bilibili.card/render@1";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.link/resolve@1";
pub const MANAGEMENT_COMMAND: &str = "mutsuki.bot.bilibili.management/command@1";
pub const RISK_CONTROL_STATUS_EVENT: &str = "mutsuki.bot.bilibili.risk_control/status@1";
pub const BILIBILI_EVENT_TYPE: &str = "mutsuki.bot.event.bilibili";
pub const BILIBILI_NOTIFICATION_NODE_TYPE: &str = "mutsuki.bot.bilibili.notification";
pub const BILIBILI_CARD_NODE_TYPE: &str = "mutsuki.bot.bilibili.card";
pub const MAX_MEDIA_BYTES: usize = MAX_LINK_CARD_MEDIA_BYTES;

pub struct RuntimeBilibiliQrRenderer {
    client: RuntimeClientRef,
    resources: Arc<dyn ResourceRegistryGateway>,
    next_task: AtomicU64,
}

impl RuntimeBilibiliQrRenderer {
    #[must_use]
    pub fn new(client: RuntimeClientRef, resources: Arc<dyn ResourceRegistryGateway>) -> Self {
        Self {
            client,
            resources,
            next_task: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl BilibiliQrRenderer for RuntimeBilibiliQrRenderer {
    async fn render_qr(&self, content: &str) -> Result<Vec<u8>, BilibiliError> {
        let sequence = self.next_task.fetch_add(1, Ordering::Relaxed) + 1;
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let task_id = format!("bilibili.web.qr.{epoch_nanos}.{sequence}");
        let task = Task::new(
            task_id.clone(),
            QR_RENDER,
            serde_json::to_value(QrRenderRequest {
                content: content.into(),
                min_dimensions: 256,
            })
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?,
        );
        let handle = self
            .client
            .submit_batch(TaskBatch::one(format!("{task_id}.batch"), task))
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| BilibiliError::Transport("QR render returned no task handle".into()))?;
        let outcome = TaskHandleFuture::new(self.client.clone(), handle)
            .await
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let response: ImageRenderResponse = match outcome {
            TaskOutcome::Completed {
                output: Some(output),
                ..
            } => serde_json::from_value(output)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?,
            TaskOutcome::Completed { output: None, .. } => {
                return Err(BilibiliError::InvalidResponse(
                    "QR renderer completed without output".into(),
                ));
            }
            outcome => {
                return Err(BilibiliError::Transport(format!(
                    "QR renderer did not complete: {outcome:?}"
                )));
            }
        };
        self.resources
            .collect_read_plan(&ReadPlan {
                plan_id: format!("{task_id}.read"),
                resource: response.resource,
                operation: "collect".into(),
                args: Value::Null,
            })
            .map_err(|error| BilibiliError::Transport(error.to_string()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliPollKind {
    Live,
    Dynamic,
    Video,
}

impl BilibiliPollKind {
    pub fn protocol_id(&self) -> &'static str {
        match self {
            Self::Live => POLL_LIVE,
            Self::Dynamic => POLL_DYNAMIC,
            Self::Video => POLL_VIDEO,
        }
    }

    fn from_protocol_id(protocol_id: &str) -> Option<Self> {
        match protocol_id {
            POLL_LIVE => Some(Self::Live),
            POLL_DYNAMIC => Some(Self::Dynamic),
            POLL_VIDEO => Some(Self::Video),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PollRequest {
    pub subscription_id: String,
    pub uid: u64,
    pub target: BotTarget,
    pub outbound_binding: String,
}

/// Trigger event payload submitted into Bot Flow when polling finds a fresh
/// item. Card rendering and delivery are graph concerns; the plugin only
/// reports what changed and which chat the subscription watches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BilibiliNotification {
    pub kind: BilibiliPollKind,
    pub subscription_id: String,
    pub uid: u64,
    pub target: BotTarget,
    pub item_id: String,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkResolveRequest {
    pub url: String,
    pub target: BotTarget,
    pub outbound_binding: String,
    pub account_id: String,
    pub now_ms: u64,
    pub cooldown_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BilibiliSubscription {
    pub subscription_id: String,
    pub uid: u64,
    pub notifications: Vec<BilibiliPollKind>,
    pub target: BotTarget,
    pub outbound_binding: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BilibiliManagementConfig {
    pub enabled: bool,
    pub allow_self_binding: bool,
    pub admin_user_ids: Vec<String>,
    pub self_binding_notifications: Vec<BilibiliPollKind>,
    pub self_binding_outbound_binding: String,
}

impl Default for BilibiliManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_self_binding: false,
            admin_user_ids: Vec::new(),
            self_binding_notifications: vec![
                BilibiliPollKind::Live,
                BilibiliPollKind::Dynamic,
                BilibiliPollKind::Video,
            ],
            self_binding_outbound_binding: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkResolverConfig {
    pub enabled: bool,
    pub cooldown_ms: u64,
    pub account_to_binding: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliRiskControlBackend {
    Chromium,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilibiliRiskControlConfig {
    pub backend: BilibiliRiskControlBackend,
    pub timeout_ms: u64,
    pub max_response_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BilibiliBackendConfig {
    WebCookie {
        cookie_secret_key: String,
    },
    OpenPlatform {
        client_id: String,
        app_secret_key: String,
        oauth_credential_key: String,
        authorized_uid: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BilibiliBackendKind {
    WebCookie,
    OpenPlatform,
}

impl BilibiliBackendConfig {
    pub fn kind(&self) -> BilibiliBackendKind {
        match self {
            Self::WebCookie { .. } => BilibiliBackendKind::WebCookie,
            Self::OpenPlatform { .. } => BilibiliBackendKind::OpenPlatform,
        }
    }

    pub fn cookie_secret_key(&self) -> Option<&str> {
        match self {
            Self::WebCookie { cookie_secret_key } => Some(cookie_secret_key),
            Self::OpenPlatform { .. } => None,
        }
    }
}

impl BilibiliRiskControlConfig {
    fn validate(&self) -> Result<(), String> {
        if self.timeout_ms == 0 {
            return Err("risk_control.timeout_ms must be greater than zero".into());
        }
        if self.max_response_bytes == 0 {
            return Err("risk_control.max_response_bytes must be greater than zero".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilibiliConfig {
    pub backend: BilibiliBackendConfig,
    pub live_interval_ms: u64,
    pub dynamic_interval_ms: u64,
    pub video_interval_ms: u64,
    pub retry: RetryConfig,
    pub subscriptions: Vec<BilibiliSubscription>,
    pub link_resolver: LinkResolverConfig,
    #[serde(default)]
    pub media_provider_id: String,
    #[serde(default)]
    pub risk_control: Option<BilibiliRiskControlConfig>,
    #[serde(default)]
    pub management: BilibiliManagementConfig,
}

impl BilibiliConfig {
    pub fn validate(&self) -> Result<(), String> {
        match &self.backend {
            BilibiliBackendConfig::WebCookie { cookie_secret_key } => {
                if cookie_secret_key.trim().is_empty() {
                    return Err("backend.cookie_secret_key is required".into());
                }
            }
            BilibiliBackendConfig::OpenPlatform {
                client_id,
                app_secret_key,
                oauth_credential_key,
                authorized_uid,
            } => {
                if client_id.trim().is_empty()
                    || app_secret_key.trim().is_empty()
                    || oauth_credential_key.trim().is_empty()
                    || authorized_uid == &0
                {
                    return Err("open_platform backend requires client_id, app_secret_key, oauth_credential_key and authorized_uid".into());
                }
                if app_secret_key == oauth_credential_key {
                    return Err("open_platform secret keys must be distinct".into());
                }
                if self.management.enabled || self.risk_control.is_some() {
                    return Err("open_platform backend does not support Cookie management or Chromium risk control".into());
                }
                if self.link_resolver.enabled {
                    return Err("open_platform backend does not support Web link resolution".into());
                }
                for subscription in &self.subscriptions {
                    if subscription.uid != *authorized_uid {
                        return Err("open_platform subscriptions must target authorized_uid".into());
                    }
                    if subscription
                        .notifications
                        .iter()
                        .any(|kind| matches!(kind, BilibiliPollKind::Dynamic))
                    {
                        return Err("open_platform backend does not provide poll/dynamic".into());
                    }
                }
            }
        }
        if self.media_provider_id.trim().is_empty() {
            return Err("media_provider_id is required".into());
        }
        if [
            self.live_interval_ms,
            self.dynamic_interval_ms,
            self.video_interval_ms,
            self.retry.initial_backoff_ms,
            self.retry.max_backoff_ms,
        ]
        .contains(&0)
            || self.retry.max_attempts == 0
            || self.retry.initial_backoff_ms > self.retry.max_backoff_ms
        {
            return Err("poll intervals and retry/backoff must be positive and ordered".into());
        }
        for subscription in &self.subscriptions {
            if subscription.subscription_id.trim().is_empty()
                || subscription.uid == 0
                || subscription.notifications.is_empty()
                || subscription.outbound_binding.trim().is_empty()
            {
                return Err("subscriptions require id, uid, notification types and binding".into());
            }
        }
        let mut ids = self
            .subscriptions
            .iter()
            .map(|subscription| subscription.subscription_id.trim())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("subscription_id values must be unique".into());
        }
        if self.management.enabled
            && (self
                .management
                .self_binding_outbound_binding
                .trim()
                .is_empty()
                || (self.management.allow_self_binding
                    && self.management.self_binding_notifications.is_empty()))
        {
            return Err("enabled management requires self-binding defaults".into());
        }
        if let Some(risk_control) = &self.risk_control {
            risk_control.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SharedBilibiliConfig(Arc<RwLock<BilibiliConfig>>);

impl SharedBilibiliConfig {
    pub fn new(config: BilibiliConfig) -> Self {
        Self(Arc::new(RwLock::new(config)))
    }

    pub fn snapshot(&self) -> BilibiliConfig {
        self.0.read().expect("Bilibili config read lock").clone()
    }

    pub fn replace(&self, config: BilibiliConfig) {
        *self.0.write().expect("Bilibili config write lock") = config;
    }
}

impl fmt::Debug for SharedBilibiliConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedBilibiliConfig(..)")
    }
}

#[derive(Clone, Default)]
pub struct SharedBilibiliCredential(Arc<Mutex<Option<String>>>);

impl fmt::Debug for SharedBilibiliCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedBilibiliCredential([REDACTED])")
    }
}

impl SharedBilibiliCredential {
    pub fn set(&self, cookie: String) {
        *self.0.lock().expect("credential mutex") = Some(cookie);
    }

    pub fn clear(&self) {
        *self.0.lock().expect("credential mutex") = None;
    }

    pub fn is_loaded(&self) -> bool {
        self.0
            .lock()
            .expect("credential mutex")
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn raw(&self) -> Option<String> {
        self.0
            .lock()
            .expect("credential mutex")
            .clone()
            .filter(|value| !value.trim().is_empty())
    }

    fn get(&self) -> Result<String, BilibiliError> {
        self.raw().ok_or(BilibiliError::CookieExpired)
    }

    fn get_named(&self, name: &str) -> Result<String, BilibiliError> {
        self.raw()
            .ok_or_else(|| BilibiliError::OpenPlatformCredentialUnavailable(name.into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilibiliItem {
    pub id: String,
    pub title: String,
    pub url: String,
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BilibiliProfile {
    pub name: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilibiliQrCode {
    pub url: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliQrStatus {
    Pending,
    Scanned,
    Expired,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BilibiliQrPoll {
    pub status: BilibiliQrStatus,
    pub credential: Option<String>,
}

pub trait BilibiliCredentialStore: Send + Sync {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String>;
}

pub trait BilibiliConfigStore: Send + Sync {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum BilibiliError {
    #[error("Bilibili cookie is missing or expired")]
    CookieExpired,
    #[error("Bilibili request was rate limited")]
    RateLimited,
    #[error("Bilibili risk control rejected the request (code 352)")]
    RiskControl352,
    #[error("Bilibili domain is not allowed: {0}")]
    DomainDenied(String),
    #[error("Bilibili response is invalid: {0}")]
    InvalidResponse(String),
    #[error("Bilibili transport failed: {0}")]
    Transport(String),
    #[error("Bilibili management is unavailable: {0}")]
    ManagementUnavailable(String),
    #[error("Bilibili management request is forbidden")]
    Forbidden,
    #[error("Bilibili Open Platform credential is unavailable: {0}")]
    OpenPlatformCredentialUnavailable(String),
    #[error("Bilibili Open Platform credential is invalid: {0}")]
    OpenPlatformCredentialInvalid(String),
    #[error("Bilibili Open Platform permission is unavailable: {scope} (code {code})")]
    OpenPlatformPermissionDenied {
        code: i64,
        scope: String,
        request_id: Option<String>,
    },
    #[error("Bilibili Open Platform OAuth credential is expired")]
    OpenPlatformOAuthExpired { request_id: Option<String> },
    #[error("Bilibili Open Platform signature was rejected (code {code})")]
    OpenPlatformSignatureRejected {
        code: i64,
        request_id: Option<String>,
    },
    #[error("Bilibili Open Platform request failed with code {code}: {message}")]
    OpenPlatformApi {
        code: i64,
        message: String,
        request_id: Option<String>,
    },
    #[error("Bilibili Open Platform does not support capability: {0}")]
    OpenPlatformUnsupported(String),
}

pub trait BilibiliTransport: Send {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError>;
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError>;
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError>;
    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError>;
    fn qr_poll(&mut self, key: &str) -> Result<BilibiliQrPoll, BilibiliError>;
    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError>;
}

/// Blocking HTTP transport for the Bilibili runner.
///
/// The runner reaches this through `TaskAwaitRunnerAdapter`, which the Host polls on its sync
/// worker threads, so a call here occupies one worker and never the async reactor. Every request
/// is bounded by `timeout` to keep that occupancy finite. Console code paths are async and must
/// not call these methods inline; `management::in_blocking_section` marks the hand-off.
pub struct ReqwestBilibiliTransport {
    client: Option<Client>,
    credential: SharedBilibiliCredential,
    timeout: Duration,
}

impl ReqwestBilibiliTransport {
    pub fn new(credential: SharedBilibiliCredential, timeout: Duration) -> Self {
        Self {
            client: None,
            credential,
            timeout,
        }
    }

    fn client(&mut self) -> Result<&Client, BilibiliError> {
        if self.client.is_none() {
            self.client = Some(secure_media::try_media_client(
                self.timeout,
                "Mozilla/5.0 MutsukiBot/0.1",
            )?);
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }

    fn json(&mut self, url: &str) -> Result<Value, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let cookie = self.credential.get()?;
        let response = self
            .client()?
            .get(url)
            .header("Cookie", cookie)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if response.status().as_u16() == 429 {
            return Err(BilibiliError::RateLimited);
        }
        let value: Value = response
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        match value.get("code").and_then(Value::as_i64) {
            Some(-101) => Err(BilibiliError::CookieExpired),
            Some(-352 | 352) => Err(BilibiliError::RiskControl352),
            Some(code) if code != 0 => Err(BilibiliError::InvalidResponse(format!("code {code}"))),
            _ => Ok(value),
        }
    }

    fn wbi_url(
        &mut self,
        path: &str,
        params: Vec<(String, String)>,
    ) -> Result<String, BilibiliError> {
        let nav = self.json("https://api.bilibili.com/x/web-interface/nav")?;
        let img = string_field(&nav["data"]["wbi_img"], "img_url")?;
        let sub = string_field(&nav["data"]["wbi_img"], "sub_url")?;
        let key = wbi_mixin_key(&img, &sub)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .as_secs() as i64;
        Ok(format!(
            "https://api.bilibili.com{path}?{}",
            sign_wbi_query(&params, &key, now)
        ))
    }
}

impl BilibiliTransport for ReqwestBilibiliTransport {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let url = match kind {
            BilibiliPollKind::Live => self.wbi_url(
                "/x/space/wbi/acc/info",
                vec![("mid".into(), uid.to_string())],
            )?,
            BilibiliPollKind::Dynamic => format!(
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space?host_mid={uid}"
            ),
            BilibiliPollKind::Video => self.wbi_url(
                "/x/space/wbi/arc/search",
                vec![
                    ("mid".into(), uid.to_string()),
                    ("pn".into(), "1".into()),
                    ("ps".into(), "10".into()),
                ],
            )?,
        };
        parse_poll_items(kind, uid, self.json(&url)?)
    }

    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let mut parsed =
            Url::parse(url).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if parsed.host_str() == Some("b23.tv") {
            let client = self.client()?;
            parsed = secure_media::secure_resolve_redirect(client, parsed, allow_bilibili_url)?;
        }
        let path = parsed.path();
        let bvid = path.split('/').find(|part| part.starts_with("BV"));
        let api = bvid
            .map(|bvid| format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}"))
            .ok_or_else(|| BilibiliError::InvalidResponse("unsupported Bilibili link".into()))?;
        let value = self.json(&api)?;
        let data = &value["data"];
        Ok(ResolvedLinkCard {
            url: parsed.to_string(),
            title: string_field(data, "title")?,
            description: data
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            image_url: data
                .get("pic")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
    }

    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        let client = self.client()?;
        secure_media::secure_media_download(client, url, max_bytes, allow_bilibili_url)
    }

    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        let value: Value = self
            .client()?
            .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if value.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(BilibiliError::InvalidResponse(
                "QR generation failed".into(),
            ));
        }
        Ok(BilibiliQrCode {
            url: string_field(&value["data"], "url")?,
            key: string_field(&value["data"], "qrcode_key")?,
        })
    }

    fn qr_poll(&mut self, key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        let mut url = Url::parse("https://passport.bilibili.com/x/passport-login/web/qrcode/poll")
            .expect("static Bilibili QR URL");
        url.query_pairs_mut()
            .append_pair("qrcode_key", key)
            .append_pair("source", "main-fe-header");
        let value: Value = self
            .client()?
            .get(url)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        let code = value["data"]["code"]
            .as_i64()
            .ok_or_else(|| BilibiliError::InvalidResponse("QR status code".into()))?;
        let status = match code {
            86101 => BilibiliQrStatus::Pending,
            86090 => BilibiliQrStatus::Scanned,
            86038 => BilibiliQrStatus::Expired,
            0 => BilibiliQrStatus::Confirmed,
            _ => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "QR status code {code}"
                )));
            }
        };
        let credential = if status == BilibiliQrStatus::Confirmed {
            Some(credential_from_redirect(
                string_field(&value["data"], "url")?.as_str(),
            )?)
        } else {
            None
        };
        Ok(BilibiliQrPoll { status, credential })
    }

    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError> {
        let url = self.wbi_url(
            "/x/space/wbi/acc/info",
            vec![("mid".into(), uid.to_string())],
        )?;
        let value = self.json(&url)?;
        Ok(BilibiliProfile {
            name: string_field(&value["data"], "name")?,
            signature: value["data"]["sign"].as_str().unwrap_or_default().into(),
        })
    }
}

pub struct SqliteBilibiliRepository {
    connection: Mutex<Connection>,
}

impl SqliteBilibiliRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| rusqlite::Error::InvalidPath(parent.into()))?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS cursor (key TEXT PRIMARY KEY, value TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS cooldown (key TEXT PRIMARY KEY, seen_ms INTEGER NOT NULL);\
             CREATE TABLE IF NOT EXISTS qr_session (actor_id TEXT PRIMARY KEY, qr_key TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS binding_challenge (actor_id TEXT PRIMARY KEY, uid INTEGER NOT NULL, code TEXT NOT NULL);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn cursor(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row("SELECT value FROM cursor WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
    }

    fn set_cursor(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO cursor(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn cooldown_ready(
        &self,
        key: &str,
        now_ms: u64,
        cooldown_ms: u64,
    ) -> Result<bool, rusqlite::Error> {
        let connection = self.connection.lock().expect("sqlite mutex");
        let previous: Option<i64> = connection
            .query_row(
                "SELECT seen_ms FROM cooldown WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        let previous = previous
            .map(|previous| {
                u64::try_from(previous)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, previous))
            })
            .transpose()?;
        if previous.is_some_and(|previous| now_ms.saturating_sub(previous) < cooldown_ms) {
            return Ok(false);
        }
        Ok(true)
    }

    fn record_cooldown(&self, key: &str, now_ms: u64) -> Result<(), rusqlite::Error> {
        let now_ms = i64::try_from(now_ms)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO cooldown(key,seen_ms) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET seen_ms=excluded.seen_ms",
            params![key, now_ms],
        )?;
        Ok(())
    }

    pub fn set_qr_session(&self, actor_id: &str, key: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO qr_session(actor_id,qr_key) VALUES(?1,?2) ON CONFLICT(actor_id) DO UPDATE SET qr_key=excluded.qr_key",
            params![actor_id, key],
        )?;
        Ok(())
    }

    pub fn qr_session(&self, actor_id: &str) -> Result<Option<String>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row(
                "SELECT qr_key FROM qr_session WHERE actor_id = ?1",
                [actor_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn clear_qr_session(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .execute("DELETE FROM qr_session WHERE actor_id = ?1", [actor_id])?;
        Ok(())
    }

    pub fn set_binding_challenge(
        &self,
        actor_id: &str,
        uid: u64,
        code: &str,
    ) -> Result<(), rusqlite::Error> {
        let uid = i64::try_from(uid)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO binding_challenge(actor_id,uid,code) VALUES(?1,?2,?3) ON CONFLICT(actor_id) DO UPDATE SET uid=excluded.uid,code=excluded.code",
            params![actor_id, uid, code],
        )?;
        Ok(())
    }

    pub fn binding_challenge(
        &self,
        actor_id: &str,
    ) -> Result<Option<(u64, String)>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row(
                "SELECT uid,code FROM binding_challenge WHERE actor_id = ?1",
                [actor_id],
                |row| {
                    let uid: i64 = row.get(0)?;
                    let uid = u64::try_from(uid)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, uid))?;
                    Ok((uid, row.get(1)?))
                },
            )
            .optional()
    }

    pub fn clear_binding_challenge(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "DELETE FROM binding_challenge WHERE actor_id = ?1",
            [actor_id],
        )?;
        Ok(())
    }
}

pub struct BilibiliRunner {
    backend_kind: BilibiliBackendKind,
    transport: Box<dyn BilibiliTransport>,
    repository: Arc<SqliteBilibiliRepository>,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
    managed_config: Option<SharedBilibiliConfig>,
    management: Option<Arc<dyn BilibiliManagementApi>>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
    /// Cursor keys skipped while the push chain was unwired. The next wired
    /// poll baselines them so the frozen window never replays as a backlog.
    frozen_polls: BTreeSet<String>,
}

struct PreparedCards {
    result: RunnerResult,
    cards: Vec<PreparedCard>,
    cursor_update: Option<(Arc<SqliteBilibiliRepository>, String, String)>,
    cooldown_update: Option<(Arc<SqliteBilibiliRepository>, String, u64)>,
}

struct PreparedCard {
    target: BotTarget,
    request: CardRenderRequest,
    route: CardRoute,
}

enum CardRoute {
    Notify(String),
    Command(Option<String>),
}

impl BilibiliRunner {
    pub fn new(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
    ) -> Self {
        Self::new_for_backend(
            transport,
            repository,
            resources,
            media_provider_id,
            BilibiliBackendKind::WebCookie,
        )
    }

    pub fn new_for_backend(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
        backend_kind: BilibiliBackendKind,
    ) -> Self {
        Self {
            backend_kind,
            transport,
            repository,
            resources,
            media_provider_id: media_provider_id.into(),
            managed_config: None,
            management: None,
            flow_registry: None,
            frozen_polls: BTreeSet::new(),
        }
    }

    /// Shares the Flow registry so the poll path can ask whether the push
    /// Source chain is wired into the active graph. Without a registry the
    /// runner keeps the historical always-poll behavior.
    pub fn with_flow_registry(mut self, registry: Arc<BotFlowRegistry>) -> Self {
        self.flow_registry = Some(registry);
        self
    }

    /// `None` when no registry is injected (wiring unknown); otherwise whether
    /// the `mutsuki.bot.bilibili.notification` Source chain has downstream.
    fn push_source_wired(&self) -> Option<bool> {
        self.flow_registry.as_ref().map(|registry| {
            registry.source_wired(BOT_EVENT_INGEST_PROTOCOL_ID, Some((BILIBILI_EVENT_TYPE, 1)))
        })
    }

    pub fn with_management(
        mut self,
        config: SharedBilibiliConfig,
        management: Arc<dyn BilibiliManagementApi>,
    ) -> Self {
        self.managed_config = Some(config);
        self.management = Some(management);
        self
    }

    pub fn into_runtime_runner(
        self,
        client: RuntimeClientRef,
        risk_control: Option<BilibiliRiskControlConfig>,
    ) -> Box<dyn Runner> {
        let descriptor = runner_descriptor(
            self.managed_config.is_some(),
            risk_control.is_some(),
            self.backend_kind,
        );
        let state = Arc::new(Mutex::new(self));
        let factory = Box::new(move |ctx: AsyncRunnerContext, task: Task| {
            let state = state.clone();
            let risk_control = risk_control.clone();
            Box::pin(async move { run_task_async(ctx, task, state, risk_control).await })
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
                >
        });
        Box::new(
            TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false),
        )
    }

    fn finish_poll(
        &mut self,
        task: &Task,
        request: PollRequest,
        kind: BilibiliPollKind,
        items: Vec<BilibiliItem>,
        status: Option<DomainEvent>,
        force_baseline: bool,
    ) -> Result<RunnerResult, RuntimeError> {
        let key = poll_cursor_key(&kind, &request);
        let previous = if force_baseline {
            None
        } else {
            self.repository
                .cursor(&key)
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?
        };
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.events.extend(status);
        let Some(head) = items.first().map(|item| item.id.clone()) else {
            return Ok(result);
        };
        let Some(previous) = previous else {
            self.repository
                .set_cursor(&key, &head)
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
            return Ok(result);
        };
        for (index, item) in fresh_since(items, &previous).into_iter().enumerate() {
            result.tasks.push(notification_ingress_task(
                task,
                BilibiliNotification {
                    kind,
                    subscription_id: request.subscription_id.clone(),
                    uid: request.uid,
                    target: request.target.clone(),
                    item_id: item.id,
                    title: item.title,
                    url: item.url,
                    image_url: item.image_url,
                },
                index,
            ));
        }
        self.repository
            .set_cursor(&key, &head)
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
        Ok(result)
    }

    fn run_command(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        let command: BotCommandEvent = decode(task)?;
        let Some(management) = self.management.clone() else {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        };
        let config = self
            .managed_config
            .as_ref()
            .expect("management config is installed with the API")
            .snapshot();
        if !config.management.enabled {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        }
        let actor_id = command
            .source
            .actor
            .as_ref()
            .map(|actor| actor.user_id.as_str())
            .ok_or_else(|| bili_error(task, BilibiliError::Forbidden))?;
        let action = command.args.first().map(String::as_str).unwrap_or("help");
        let is_admin = config
            .management
            .admin_user_ids
            .iter()
            .any(|candidate| candidate == actor_id);
        match action {
            "help" => Ok(self.command_reply(
                task,
                &command,
                "可用命令：login、login-status、bind <uid>、verify、unbind、pause [订阅/UID]、resume [订阅/UID]、preview [订阅/UID]、list；管理员另可使用 subscribe <id> <uid> [live,dynamic,video] 与 unsubscribe <id>。",
                None,
            )),
            "login" => {
                Err(bili_error(
                    task,
                    BilibiliError::ManagementUnavailable(
                        "login requires the asynchronous image renderer".into(),
                    ),
                ))
            }
            "login-status" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let polled = management
                    .login_poll(actor_id)
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(task, &command, polled.message, None))
            }
            "bind" => {
                let uid = parse_uid(command.args.get(1)).map_err(|error| bili_error(task, error))?;
                let challenge = management
                    .bind_start(actor_id, uid, task.task_id.as_str())
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!(
                        "已为 {} ({}) 创建验证。请临时把 {} 加入 Bilibili 个性签名，然后通过验证节点继续。",
                        challenge.name, challenge.uid, challenge.code
                    ),
                    None,
                ))
            }
            "verify" => {
                match management
                    .bind_verify(
                        actor_id,
                        command.source.platform.as_str(),
                        command.source.target.clone(),
                    )
                    .map_err(|error| bili_management_error(task, error))?
                {
                    BilibiliBindVerifyResult::Verified(subscription) => Ok(self.command_reply(
                        task,
                        &command,
                        format!(
                            "验证成功，已绑定 UID {} 并写入产品配置。",
                            subscription.uid
                        ),
                        None,
                    )),
                    BilibiliBindVerifyResult::SignatureMismatch { code } => Ok(self.command_reply(
                        task,
                        &command,
                        format!("验证未通过：个性签名中尚未找到 {code}。"),
                        None,
                    )),
                }
            }
            "unbind" => {
                let removed = management
                    .unbind(actor_id)
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    if removed {
                        "已解除绑定并更新产品配置。"
                    } else {
                        "当前没有自助绑定。"
                    },
                    None,
                ))
            }
            "pause" | "resume" => {
                let paused = action == "pause";
                let view = management
                    .set_paused(
                        actor_id,
                        is_admin,
                        command.args.get(1).map(String::as_str),
                        paused,
                    )
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!(
                        "订阅 {} 已{}。",
                        view.subscription_id,
                        if paused { "暂停" } else { "恢复" }
                    ),
                    None,
                ))
            }
            "preview" => {
                Err(bili_error(
                    task,
                    BilibiliError::ManagementUnavailable(
                        "preview requires the asynchronous image renderer".into(),
                    ),
                ))
            }
            "list" => {
                let lines = management
                    .list(actor_id, is_admin)
                    .map_err(|error| bili_management_error(task, error))?
                    .into_iter()
                    .map(|subscription| {
                        format!(
                            "{} -> UID {} [{}]{}",
                            subscription.subscription_id,
                            subscription.uid,
                            subscription
                                .notifications
                                .iter()
                                .map(|kind| format!("{kind:?}").to_ascii_lowercase())
                                .collect::<Vec<_>>()
                                .join(","),
                            if subscription.paused { " (paused)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(self.command_reply(
                    task,
                    &command,
                    if lines.is_empty() {
                        "没有可管理的订阅。".into()
                    } else {
                        lines.join("\n")
                    },
                    None,
                ))
            }
            "subscribe" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let subscription_id = required_arg(&command.args, 1, "subscription id")
                    .map_err(|error| bili_error(task, error))?;
                let uid = parse_uid(command.args.get(2)).map_err(|error| bili_error(task, error))?;
                let notifications = parse_notifications(command.args.get(3))
                    .map_err(|error| bili_error(task, error))?;
                management
                    .subscribe(
                        subscription_id.clone(),
                        uid,
                        notifications,
                        command.source.target.clone(),
                        config.management.self_binding_outbound_binding.clone(),
                    )
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!("订阅 {subscription_id} 已写入产品配置。"),
                    None,
                ))
            }
            "unsubscribe" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let subscription_id = required_arg(&command.args, 1, "subscription id")
                    .map_err(|error| bili_error(task, error))?;
                management
                    .unsubscribe(&subscription_id)
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!("订阅 {subscription_id} 已从产品配置删除。"),
                    None,
                ))
            }
            _ => Ok(self.command_reply(task, &command, "未知 Bilibili 管理命令。", None)),
        }
    }

    fn command_reply(
        &self,
        task: &Task,
        command: &BotCommandEvent,
        text: impl Into<String>,
        image: Option<mutsuki_runtime_contracts::ResourceRef>,
    ) -> RunnerResult {
        let mut segments = Vec::new();
        if let Some(resource) = image {
            segments.push(MessageSegment::Image { resource });
        }
        segments.push(MessageSegment::Text { text: text.into() });
        let binding = self
            .managed_config
            .as_ref()
            .map(SharedBilibiliConfig::snapshot)
            .map(|config| config.management.self_binding_outbound_binding);
        command_outbound_result(
            task,
            BotMessage {
                message_id: None,
                target: command.source.target.clone(),
                sender: None,
                segments,
                reply_to: command
                    .source
                    .message
                    .as_ref()
                    .and_then(|message| message.message_id.clone()),
                time_ms: None,
                ext: BotExtMap::new(),
            },
            binding.as_deref(),
        )
    }

    fn prepare_card_request(
        &mut self,
        card: ResolvedLinkCard,
        task: &Task,
        layout: CardLayout,
        kicker: &str,
        live: bool,
    ) -> Result<CardRenderRequest, RuntimeError> {
        let cover = if let Some(image_url) = card.image_url {
            let bytes = self
                .transport
                .download(&image_url, MAX_MEDIA_BYTES)
                .map_err(|error| bili_error(task, error))?;
            let resource = self
                .resources
                .create_blob_resource(
                    &self.media_provider_id,
                    "mutsuki.bot.image.original.v1",
                    bytes,
                )
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
            Some(resource)
        } else {
            None
        };
        Ok(CardRenderRequest {
            brand: "哔哩哔哩".into(),
            title: card.title,
            description: card.description,
            url: card.url,
            cover,
            fallback_gradient: CardGradient {
                start: Rgba {
                    red: 251,
                    green: 114,
                    blue: 153,
                    alpha: 255,
                },
                end: Rgba {
                    red: 0,
                    green: 174,
                    blue: 236,
                    alpha: 255,
                },
            },
            layout,
            kicker: kicker.into(),
            live,
            ..CardRenderRequest::default()
        })
    }
}

fn layout_for_poll(kind: BilibiliPollKind) -> (CardLayout, &'static str, bool) {
    match kind {
        BilibiliPollKind::Live => (CardLayout::Row, "直播", true),
        BilibiliPollKind::Dynamic => (CardLayout::Feed, "动态", false),
        BilibiliPollKind::Video => (CardLayout::Media, "投稿", false),
    }
}

fn poll_description(kind: BilibiliPollKind) -> &'static str {
    match kind {
        BilibiliPollKind::Live => "直播状态更新",
        BilibiliPollKind::Dynamic => "发布了新动态",
        BilibiliPollKind::Video => "发布了新投稿",
    }
}

fn layout_for_url(url: &str) -> (CardLayout, &'static str, bool) {
    let parsed = Url::parse(url).ok();
    let host = parsed
        .as_ref()
        .and_then(|value| value.host_str())
        .unwrap_or_default();
    let path = parsed.as_ref().map_or("", Url::path);
    if host.contains("live.bilibili") {
        (CardLayout::Hero, "直播", true)
    } else if host == "t.bilibili.com" || path.contains("/opus") || path.contains("/dynamic") {
        (CardLayout::Feed, "动态", false)
    } else {
        (CardLayout::Media, "投稿", false)
    }
}

async fn run_task_async(
    ctx: AsyncRunnerContext,
    mut task: Task,
    state: Arc<Mutex<BilibiliRunner>>,
    risk_control: Option<BilibiliRiskControlConfig>,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id == MANAGEMENT_COMMAND {
        let invocation = serde_json::from_value::<BotNodeInvocation>(task.payload.to_value()).ok();
        if let Some(invocation) = &invocation {
            task.payload = mutsuki_runtime_contracts::TaskPayload::from_local(
                invocation.input.payload.value.clone(),
            );
        }
        let result = run_management_task(ctx, task, state).await?;
        return if let Some(invocation) = invocation {
            wrap_management_node_result(result, invocation)
        } else {
            Ok(result)
        };
    }
    if task.protocol_id == LINK_RESOLVE {
        let payload: Value = task.payload.clone().into();
        let invocation = serde_json::from_value::<BotNodeInvocation>(payload).ok();
        let request = match &invocation {
            Some(invocation) => {
                link_resolve_request_from_invocation(invocation).map_err(|error| {
                    RuntimeFailure::new(bili_error(&task, BilibiliError::InvalidResponse(error)))
                })?
            }
            None => decode(&task).map_err(RuntimeFailure::new)?,
        };
        let prepared = {
            let mut runner = state.lock().expect("Bilibili runner mutex");
            let cooldown_key = format!("{}:{}", request.account_id, request.url);
            if !runner
                .repository
                .cooldown_ready(&cooldown_key, request.now_ms, request.cooldown_ms)
                .map_err(|error| {
                    RuntimeFailure::new(bili_error(
                        &task,
                        BilibiliError::Transport(error.to_string()),
                    ))
                })?
            {
                None
            } else {
                let card = runner
                    .transport
                    .resolve(&request.url)
                    .map_err(|error| RuntimeFailure::new(bili_error(&task, error)))?;
                let (layout, kicker, live) = layout_for_url(&card.url);
                let card = runner
                    .prepare_card_request(card, &task, layout, kicker, live)
                    .map_err(RuntimeFailure::new)?;
                Some(PreparedCards {
                    result: RunnerResult::completed(task.task_id.clone()),
                    cards: vec![PreparedCard {
                        target: request.target,
                        request: card,
                        route: CardRoute::Notify(request.outbound_binding),
                    }],
                    cursor_update: None,
                    cooldown_update: Some((
                        runner.repository.clone(),
                        cooldown_key,
                        request.now_ms,
                    )),
                })
            }
        };
        let result = match prepared {
            Some(prepared) => render_cards(&ctx, &task, prepared).await?,
            None => RunnerResult::completed(task.task_id.clone()),
        };
        return if let Some(invocation) = invocation {
            wrap_management_node_result(result, invocation)
        } else {
            Ok(result)
        };
    }
    if task.protocol_id == NOTIFY_CARD {
        let invocation = serde_json::from_value::<BotNodeInvocation>(task.payload.to_value())
            .map_err(|error| {
                RuntimeFailure::new(bili_error(
                    &task,
                    BilibiliError::InvalidResponse(format!(
                        "notify card node requires a flow invocation: {error}"
                    )),
                ))
            })?;
        return run_notification_card(&ctx, &task, state, invocation).await;
    }
    let request: PollRequest = decode(&task).map_err(RuntimeFailure::new)?;
    let kind = BilibiliPollKind::from_protocol_id(task.protocol_id.as_str()).ok_or_else(|| {
        RuntimeFailure::new(bili_error(
            &task,
            BilibiliError::InvalidResponse("unsupported poll protocol".into()),
        ))
    })?;
    // Flow is the only initiation surface: when the push Source chain is not
    // wired the business is frozen, so the upstream poll is skipped entirely.
    // The skipped cursor key baselines on the first wired poll instead of
    // replaying the frozen window as a notification backlog.
    let poll_plan = {
        let mut runner = state.lock().expect("Bilibili runner mutex");
        let key = poll_cursor_key(&kind, &request);
        match runner.push_source_wired() {
            Some(false) => {
                runner.frozen_polls.insert(key);
                None
            }
            Some(true) => Some(runner.frozen_polls.take(&key).is_some()),
            None => Some(false),
        }
    };
    let Some(force_baseline) = poll_plan else {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.output = Some(json!({ "push_wired": false, "poll_skipped": true }));
        return Ok(result);
    };
    let attempt = state
        .lock()
        .expect("Bilibili runner mutex")
        .transport
        .poll(&kind, request.uid);
    match attempt {
        Ok(items) => state
            .lock()
            .expect("Bilibili runner mutex")
            .finish_poll(&task, request, kind, items, None, force_baseline)
            .map_err(RuntimeFailure::new),
        Err(BilibiliError::RiskControl352)
            if kind == BilibiliPollKind::Dynamic && risk_control.is_some() =>
        {
            run_chromium_risk_control_fallback(
                ctx,
                task,
                request,
                state,
                risk_control.expect("checked"),
            )
            .await
        }
        Err(error) => Err(RuntimeFailure::new(bili_error(&task, error))),
    }
}

const BILIBILI_LINK_HOSTS: &[&str] = &["b23.tv", "bilibili.com", "hdslb.com"];

#[derive(Default, Deserialize)]
#[serde(default)]
struct BilibiliLinkFlowConfig {
    url: Option<String>,
    outbound_binding: Option<String>,
    cooldown_ms: Option<u64>,
}

fn link_resolve_request_from_invocation(
    invocation: &BotNodeInvocation,
) -> Result<LinkResolveRequest, String> {
    let flow: BilibiliLinkFlowConfig =
        serde_json::from_value(invocation.config.clone()).map_err(|error| error.to_string())?;
    let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| error.to_string())?;
    let url = flow
        .url
        .filter(|value| !value.is_empty())
        .or_else(|| preferred_event_url(&event, BILIBILI_LINK_HOSTS))
        .ok_or_else(|| "bilibili url is missing".to_string())?;
    let outbound_binding = flow
        .outbound_binding
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    let cooldown_ms = flow.cooldown_ms.unwrap_or(1_000);
    Ok(LinkResolveRequest {
        url,
        target: event.target,
        outbound_binding,
        account_id: event.bot.account_id,
        now_ms: event.time_ms.max(0).cast_unsigned(),
        cooldown_ms,
    })
}

fn wrap_management_node_result(
    mut result: RunnerResult,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    let outputs = result
        .tasks
        .drain(..)
        .map(|task| {
            if task.protocol_id != BOT_MESSAGE_SEND_PROTOCOL_ID {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    "bot.bilibili.node.unexpected_task",
                    PLUGIN_ID,
                    task.protocol_id,
                )));
            }
            let message: BotMessage =
                serde_json::from_value(task.payload.to_value()).map_err(|error| {
                    RuntimeFailure::new(RuntimeError::new(
                        "bot.bilibili.node.output_invalid",
                        PLUGIN_ID,
                        error.to_string(),
                    ))
                })?;
            Ok(BotNodeOutput {
                port_id: "message".into(),
                event: BotFlowEventEnvelope {
                    event_id: format!("{}:message", invocation.input.event_id),
                    protocol_id: BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
                    payload: BotFlowPayload {
                        event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                        value: serde_json::to_value(message).expect("BotMessage serializes"),
                    },
                    context: invocation.input.context.clone(),
                    trace_id: invocation.input.trace_id.clone(),
                    correlation_id: invocation.input.correlation_id.clone(),
                },
            })
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs,
            metadata: Default::default(),
        })
        .expect("BotNodeResult serializes"),
    );
    Ok(result)
}

async fn run_management_task(
    ctx: AsyncRunnerContext,
    task: Task,
    state: Arc<Mutex<BilibiliRunner>>,
) -> RuntimeResult<RunnerResult> {
    let command: BotCommandEvent = decode(&task).map_err(RuntimeFailure::new)?;
    let action = command.args.first().map(String::as_str).unwrap_or("help");
    if action != "login" && action != "preview" {
        return state
            .lock()
            .expect("Bilibili runner mutex")
            .run_command(&task)
            .map_err(RuntimeFailure::new);
    }
    let (management, config, actor_id, is_admin) = {
        let runner = state.lock().expect("Bilibili runner mutex");
        let Some(management) = runner.management.clone() else {
            return Ok(RunnerResult::completed(task.task_id));
        };
        let config = runner
            .managed_config
            .as_ref()
            .expect("management config is installed with the API")
            .snapshot();
        if !config.management.enabled {
            return Ok(RunnerResult::completed(task.task_id));
        }
        let actor_id = command
            .source
            .actor
            .as_ref()
            .map(|actor| actor.user_id.clone())
            .ok_or_else(|| RuntimeFailure::new(bili_error(&task, BilibiliError::Forbidden)))?;
        let is_admin = config
            .management
            .admin_user_ids
            .iter()
            .any(|candidate| candidate == &actor_id);
        (management, config, actor_id, is_admin)
    };
    if action == "login" {
        require_admin(is_admin).map_err(|error| RuntimeFailure::new(bili_error(&task, error)))?;
        let session = management
            .login_start_session(&actor_id)
            .map_err(|error| RuntimeFailure::new(bili_management_error(&task, error)))?;
        let rendered = render_qr_child(&ctx, &task, session.url).await?;
        return Ok(state
            .lock()
            .expect("Bilibili runner mutex")
            .command_reply(
                &task,
                &command,
                "请使用 Bilibili App 扫码确认，然后发送 /bili login-status；二维码不会把 Cookie 写入聊天或 Task payload。",
                Some(rendered.resource),
            ));
    }
    match management.preview(&actor_id, is_admin, command.args.get(1).map(String::as_str)) {
        Ok(card) => {
            let (layout, kicker, live) = layout_for_url(&card.url);
            let request = state
                .lock()
                .expect("Bilibili runner mutex")
                .prepare_card_request(
                    ResolvedLinkCard {
                        url: card.url,
                        title: card.title,
                        description: card.description,
                        image_url: card.image_url,
                    },
                    &task,
                    layout,
                    kicker,
                    live,
                )
                .map_err(RuntimeFailure::new)?;
            render_cards(
                &ctx,
                &task,
                PreparedCards {
                    result: RunnerResult::completed(task.task_id.clone()),
                    cards: vec![PreparedCard {
                        target: command.source.target,
                        request,
                        route: CardRoute::Command(Some(
                            config.management.self_binding_outbound_binding,
                        )),
                    }],
                    cursor_update: None,
                    cooldown_update: None,
                },
            )
            .await
        }
        Err(error) if error.message.contains("暂无可预览") => Ok(state
            .lock()
            .expect("Bilibili runner mutex")
            .command_reply(&task, &command, error.message, None)),
        Err(error) => Err(RuntimeFailure::new(bili_management_error(&task, error))),
    }
}

async fn render_qr_child(
    ctx: &AsyncRunnerContext,
    task: &Task,
    content: String,
) -> RuntimeResult<ImageRenderResponse> {
    let outcome = ctx
        .call_raw(
            QR_RENDER,
            serde_json::to_value(QrRenderRequest {
                content,
                min_dimensions: 256,
            })
            .map_err(|error| {
                RuntimeFailure::new(bili_error(
                    task,
                    BilibiliError::InvalidResponse(error.to_string()),
                ))
            })?,
        )
        .await?;
    decode_render_outcome(task, outcome, "QR")
}

async fn render_cards(
    ctx: &AsyncRunnerContext,
    task: &Task,
    mut prepared: PreparedCards,
) -> RuntimeResult<RunnerResult> {
    for (index, card) in prepared.cards.into_iter().enumerate() {
        let url = card.request.url.clone();
        let outcome = ctx
            .call_raw(
                CARD_RENDER,
                serde_json::to_value(card.request).map_err(|error| {
                    RuntimeFailure::new(bili_error(
                        task,
                        BilibiliError::InvalidResponse(error.to_string()),
                    ))
                })?,
            )
            .await?;
        let rendered = decode_render_outcome(task, outcome, "card")?;
        let message = BotMessage {
            message_id: None,
            target: card.target,
            sender: None,
            segments: vec![
                MessageSegment::Image {
                    resource: rendered.resource,
                },
                MessageSegment::Text { text: url },
            ],
            reply_to: None,
            time_ms: None,
            ext: BotExtMap::new(),
        };
        match card.route {
            CardRoute::Notify(binding) => {
                prepared
                    .result
                    .tasks
                    .push(outbound_task(task, message, &binding, index));
            }
            CardRoute::Command(binding) => {
                let binding = binding.as_deref().filter(|value| !value.is_empty());
                prepared
                    .result
                    .tasks
                    .extend(command_outbound_result(task, message, binding).tasks);
            }
        }
    }
    if let Some((repository, key, head)) = prepared.cursor_update {
        repository.set_cursor(&key, &head).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::Transport(error.to_string()),
            ))
        })?;
    }
    if let Some((repository, key, now_ms)) = prepared.cooldown_update {
        repository.record_cooldown(&key, now_ms).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::Transport(error.to_string()),
            ))
        })?;
    }
    Ok(prepared.result)
}

fn notification_ingress_task(
    parent: &Task,
    notification: BilibiliNotification,
    index: usize,
) -> Task {
    let context = BotFlowContext {
        bot: None,
        target: Some(notification.target.clone()),
        actor: None,
        ext: BotExtMap::new(),
    };
    let envelope = BotFlowEventEnvelope {
        event_id: format!("{}:notify:{index}", parent.task_id),
        protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
        payload: BotFlowPayload {
            event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
            value: serde_json::to_value(notification).expect("BilibiliNotification serializes"),
        },
        context,
        trace_id: parent.trace_id.clone().map(Into::into),
        correlation_id: parent
            .correlation_id
            .clone()
            .or_else(|| Some(parent.task_id.to_string())),
    };
    let mut child = Task::new(
        format!("mutsuki.bot.flow.ingress:{}:notify:{index}", parent.task_id),
        BOT_FLOW_INGRESS_PROTOCOL_ID,
        mutsuki_runtime_contracts::TaskPayload::from_local(envelope),
    );
    child.registry_generation = parent.registry_generation;
    child
}

async fn run_notification_card(
    ctx: &AsyncRunnerContext,
    task: &Task,
    state: Arc<Mutex<BilibiliRunner>>,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    let notification: BilibiliNotification =
        serde_json::from_value(invocation.input.payload.value.clone()).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::InvalidResponse(error.to_string()),
            ))
        })?;
    let request = {
        let mut runner = state.lock().expect("Bilibili runner mutex");
        let (layout, kicker, live) = layout_for_poll(notification.kind);
        runner
            .prepare_card_request(
                ResolvedLinkCard {
                    url: notification.url.clone(),
                    title: notification.title.clone(),
                    description: poll_description(notification.kind).into(),
                    image_url: notification.image_url.clone(),
                },
                task,
                layout,
                kicker,
                live,
            )
            .map_err(RuntimeFailure::new)?
    };
    let outcome = ctx
        .call_raw(
            CARD_RENDER,
            serde_json::to_value(request).map_err(|error| {
                RuntimeFailure::new(bili_error(
                    task,
                    BilibiliError::InvalidResponse(error.to_string()),
                ))
            })?,
        )
        .await?;
    let rendered = decode_render_outcome(task, outcome, "card")?;
    let message = BotMessage {
        message_id: None,
        target: notification.target,
        sender: None,
        segments: vec![
            MessageSegment::Image {
                resource: rendered.resource,
            },
            MessageSegment::Text {
                text: notification.url,
            },
        ],
        reply_to: None,
        time_ms: None,
        ext: BotExtMap::new(),
    };
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "message".into(),
                event: BotFlowEventEnvelope {
                    event_id: format!("{}:message", invocation.input.event_id),
                    protocol_id: BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
                    payload: BotFlowPayload {
                        event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                        value: serde_json::to_value(message).expect("BotMessage serializes"),
                    },
                    context: invocation.input.context.clone(),
                    trace_id: invocation.input.trace_id.clone(),
                    correlation_id: invocation.input.correlation_id.clone(),
                },
            }],
            metadata: Default::default(),
        })
        .expect("BotNodeResult serializes"),
    );
    Ok(result)
}

fn decode_render_outcome(
    task: &Task,
    outcome: impl Into<TaskOutcome>,
    kind: &str,
) -> RuntimeResult<ImageRenderResponse> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::InvalidResponse(error.to_string()),
            ))
        }),
        TaskOutcome::Completed { output: None, .. } => Err(RuntimeFailure::new(bili_error(
            task,
            BilibiliError::InvalidResponse(format!("{kind} renderer completed without output")),
        ))),
        _ => Err(RuntimeFailure::new(bili_error(
            task,
            BilibiliError::Transport(format!("{kind} renderer child task failed")),
        ))),
    }
}

async fn run_chromium_risk_control_fallback(
    ctx: AsyncRunnerContext,
    task: Task,
    request: PollRequest,
    state: Arc<Mutex<BilibiliRunner>>,
    risk_control: BilibiliRiskControlConfig,
) -> RuntimeResult<RunnerResult> {
    let (resources, media_provider_id) = {
        let runner = state.lock().expect("Bilibili runner mutex");
        (runner.resources.clone(), runner.media_provider_id.clone())
    };
    let output = resources
        .create_cow_state_resource(
            &media_provider_id,
            "mutsuki.browser.snapshot.output",
            SNAPSHOT_SCHEMA,
            Vec::new(),
        )
        .map_err(|error| risk_control_failure(&task, "resource.create", error.to_string()))?;
    let snapshot_request = BrowserSnapshotRequest {
        url: format!("https://space.bilibili.com/{}/dynamic", request.uid),
        output_resource: output.clone(),
        wait_mode: BrowserWaitMode::Selector,
        selector: Some("body".into()),
        timeout_ms: risk_control.timeout_ms,
    };
    let outcome = ctx
        .call_raw(
            SNAPSHOT,
            serde_json::to_value(snapshot_request)
                .map_err(|error| risk_control_failure(&task, "request.encode", error))?,
        )
        .await
        .map_err(|error| risk_control_failure(&task, "snapshot.task", error.to_string()))?;
    if !matches!(outcome.into_outcome(), TaskOutcome::Completed { .. }) {
        return Err(risk_control_failure(
            &task,
            "snapshot.outcome",
            "Chromium snapshot task did not complete",
        ));
    }
    let latest = resources
        .open_resource_descriptor(output.ref_id.as_str())
        .map_err(|error| risk_control_failure(&task, "resource.open", error.to_string()))?;
    let bytes = resources
        .collect_read_plan(&ReadPlan {
            plan_id: format!("bilibili.risk-control.read.{}", task.task_id),
            resource: latest,
            operation: "collect".into(),
            args: Value::Null,
        })
        .map_err(|error| risk_control_failure(&task, "resource.read", error.to_string()))?;
    if bytes.len() > risk_control.max_response_bytes {
        return Err(risk_control_failure(
            &task,
            "response.oversized",
            format!(
                "Chromium response is {} bytes; maximum is {}",
                bytes.len(),
                risk_control.max_response_bytes
            ),
        ));
    }
    let snapshot: BrowserSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| risk_control_failure(&task, "response.decode", error))?;
    ensure_bilibili_domain(&snapshot.final_url)
        .map_err(|error| risk_control_failure(&task, "redirect.denied", error))?;
    let items = parse_dynamic_snapshot(&snapshot.html)
        .map_err(|error| risk_control_failure(&task, "dom.parse", error))?;
    let status = DomainEvent {
        event_id: format!("{}:risk-control", task.task_id),
        kind: RISK_CONTROL_STATUS_EVENT.into(),
        payload: json!({
            "task_id": task.task_id,
            "uid": request.uid,
            "risk_control_code": 352,
            "backend": "chromium",
            "status": "degraded",
            "fallback": "succeeded"
        }),
    };
    state
        .lock()
        .expect("Bilibili runner mutex")
        .finish_poll(
            &task,
            request,
            BilibiliPollKind::Dynamic,
            items,
            Some(status),
            false,
        )
        .map_err(RuntimeFailure::new)
}

fn poll_cursor_key(kind: &BilibiliPollKind, request: &PollRequest) -> String {
    format!("{kind:?}:{}:{}", request.uid, request.subscription_id)
}

pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    manifest_for_backend(BilibiliBackendKind::WebCookie, false, false)
}

pub fn manifest_for_config(config: &BilibiliConfig) -> mutsuki_runtime_contracts::PluginManifest {
    manifest_for_backend(
        config.backend.kind(),
        config.management.enabled,
        config.risk_control.is_some(),
    )
}

#[must_use]
pub fn manifest_for_backend(
    backend_kind: BilibiliBackendKind,
    management_enabled: bool,
    risk_control_enabled: bool,
) -> mutsuki_runtime_contracts::PluginManifest {
    let mut builder = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestRunner {
            descriptor: runner_descriptor(management_enabled, risk_control_enabled, backend_kind),
        }))
        .protocol_handler(protocol(POLL_LIVE), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(POLL_VIDEO), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(NOTIFY_CARD), RUNNER_ID, "orchestration");
    if backend_kind == BilibiliBackendKind::WebCookie {
        builder = builder
            .protocol_handler(protocol(POLL_DYNAMIC), RUNNER_ID, "orchestration")
            .protocol_handler(protocol(LINK_RESOLVE), RUNNER_ID, "orchestration");
    }
    let mut nodes = Vec::new();
    nodes.push(BotNodeDescriptor {
        node_type_id: BILIBILI_NOTIFICATION_NODE_TYPE.into(),
        version: 1,
        title: "Bilibili 通知".into(),
        category: "Bilibili".into(),
        role: BotNodeRole::Source,
        binding: None,
        ports: vec![BotNodePortDescriptor {
            port_id: "event".into(),
            title: "事件".into(),
            direction: BotNodePortDirection::Output,
            event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
            required: false,
        }],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    });
    nodes.push(BotNodeDescriptor {
        node_type_id: BILIBILI_CARD_NODE_TYPE.into(),
        version: 1,
        title: "Bilibili 推送卡片".into(),
        category: "Bilibili".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{NOTIFY_CARD}"),
            protocol_id: NOTIFY_CARD.into(),
            runner_hint: Some(RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "通知".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "message".into(),
                title: "发送消息".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                required: false,
            },
        ],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    });
    if backend_kind == BilibiliBackendKind::WebCookie {
        nodes.push(BotNodeDescriptor {
            node_type_id: "mutsuki.bot.bilibili.resolve".into(),
            version: 1,
            title: "Bilibili 链接".into(),
            category: "链接".into(),
            role: BotNodeRole::Processor,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{LINK_RESOLVE}"),
                protocol_id: LINK_RESOLVE.into(),
                runner_hint: Some(RUNNER_ID.into()),
            }),
            ports: vec![
                BotNodePortDescriptor {
                    port_id: "event".into(),
                    title: "事件".into(),
                    direction: BotNodePortDirection::Input,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                    required: true,
                },
                BotNodePortDescriptor {
                    port_id: "message".into(),
                    title: "发送消息".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                    required: false,
                },
            ],
            config_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "url": {"type": "string", "title": "Bilibili 链接"},
                    "outbound_binding": {"type": "string", "title": "出站绑定"},
                    "cooldown_ms": {
                        "type": "integer",
                        "title": "冷却（毫秒）",
                        "minimum": 0
                    }
                }
            }),
        });
    }
    if management_enabled {
        builder =
            builder.protocol_handler(protocol(MANAGEMENT_COMMAND), RUNNER_ID, "orchestration");
        nodes.push(BotNodeDescriptor {
            node_type_id: "mutsuki.bot.bilibili.management".into(),
            version: 1,
            title: "Bilibili 管理".into(),
            category: "Bilibili".into(),
            role: BotNodeRole::Processor,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{MANAGEMENT_COMMAND}"),
                protocol_id: MANAGEMENT_COMMAND.into(),
                runner_hint: Some(RUNNER_ID.into()),
            }),
            ports: vec![
                BotNodePortDescriptor {
                    port_id: "command".into(),
                    title: "管理请求".into(),
                    direction: BotNodePortDirection::Input,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.command.event", 1),
                    required: true,
                },
                BotNodePortDescriptor {
                    port_id: "message".into(),
                    title: "回复消息".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                    required: false,
                },
            ],
            config_schema: json!({"type": "object", "additionalProperties": false}),
        });
    }
    if !nodes.is_empty() {
        builder = builder.extension(
            BotNodeCatalogFragment { nodes }
                .into_plugin_extension()
                .expect("Bilibili node catalog serializes"),
        );
    }
    let mut manifest = builder.build().manifest;
    for protocol_id in manifest.provides.runners[0]
        .accepted_protocol_ids
        .iter()
        .cloned()
    {
        manifest
            .provides
            .protocol_classes
            .insert(protocol_id, ProtocolClass::Effect);
    }
    manifest
}

fn runner_descriptor(
    management: bool,
    risk_control: bool,
    backend_kind: BilibiliBackendKind,
) -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID);
    let protocols: &[&str] = match backend_kind {
        BilibiliBackendKind::WebCookie => &[
            POLL_LIVE,
            POLL_DYNAMIC,
            POLL_VIDEO,
            NOTIFY_CARD,
            LINK_RESOLVE,
        ],
        BilibiliBackendKind::OpenPlatform => &[POLL_LIVE, POLL_VIDEO, NOTIFY_CARD],
    };
    for protocol in protocols {
        builder = builder.accepted_protocol(*protocol);
    }
    if management {
        builder = builder
            .accepted_protocol(MANAGEMENT_COMMAND)
            .requires_protocol(QR_RENDER);
    }
    if risk_control {
        builder = builder.requires_protocol(SNAPSHOT);
    }
    builder
        .requires_protocol(CARD_RENDER)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            ..Default::default()
        })
        .metadata("domain", ScalarValue::String("bilibili".into()))
        .metadata(
            "backend",
            ScalarValue::String(
                match backend_kind {
                    BilibiliBackendKind::WebCookie => "web_cookie",
                    BilibiliBackendKind::OpenPlatform => "open_platform",
                }
                .into(),
            ),
        )
        .build()
}

fn protocol(id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(id)
        .input_schema(json!({"type":"object"}))
        .output_schema(json!({"type":"object"}))
        .error_schema(json!({"type":"object"}))
        .build()
}

struct ManifestRunner {
    descriptor: RunnerDescriptor,
}
impl Runner for ManifestRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            RuntimeError::new("runner.unavailable", PLUGIN_ID, "manifest_only"),
        ))
    }
}

fn decode<T: for<'de> Deserialize<'de>>(task: &Task) -> Result<T, RuntimeError> {
    serde_json::from_value(task.payload.clone().into())
        .map_err(|error| bili_error(task, BilibiliError::InvalidResponse(error.to_string())))
}

fn command_outbound_result(
    task: &Task,
    message: BotMessage,
    binding: Option<&str>,
) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    let mut child = Task::new(
        format!("{}:reply", task.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).expect("BotMessage serializes"),
    );
    child.trace_id = task.trace_id.clone();
    child.correlation_id = task
        .correlation_id
        .clone()
        .or_else(|| Some(task.task_id.to_string()));
    child.registry_generation = task.registry_generation;
    child.target_binding_id = binding.filter(|value| !value.is_empty()).map(Into::into);
    result.tasks.push(child);
    result
}

fn require_admin(is_admin: bool) -> Result<(), BilibiliError> {
    is_admin.then_some(()).ok_or(BilibiliError::Forbidden)
}

pub(crate) fn required_arg(
    args: &[String],
    index: usize,
    name: &str,
) -> Result<String, BilibiliError> {
    args.get(index)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(format!("missing {name}")))
}

pub(crate) fn parse_uid(value: Option<&String>) -> Result<u64, BilibiliError> {
    value
        .ok_or_else(|| BilibiliError::InvalidResponse("missing Bilibili UID".into()))?
        .parse::<u64>()
        .ok()
        .filter(|uid| *uid > 0)
        .ok_or_else(|| BilibiliError::InvalidResponse("invalid Bilibili UID".into()))
}

pub(crate) fn parse_notifications(
    value: Option<&String>,
) -> Result<Vec<BilibiliNotificationKind>, BilibiliError> {
    let values = value
        .map(|value| value.split(',').collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["live", "dynamic", "video"]);
    let mut notifications = Vec::new();
    for value in values {
        let kind = match value.trim() {
            "live" => BilibiliNotificationKind::Live,
            "dynamic" => BilibiliNotificationKind::Dynamic,
            "video" => BilibiliNotificationKind::Video,
            unknown => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "unknown notification type {unknown}"
                )));
            }
        };
        if !notifications.contains(&kind) {
            notifications.push(kind);
        }
    }
    if notifications.is_empty() {
        return Err(BilibiliError::InvalidResponse(
            "notification types must not be empty".into(),
        ));
    }
    Ok(notifications)
}

pub(crate) fn binding_code(actor_id: &str, uid: u64, task_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{actor_id}:{uid}:{task_id}")));
    format!("mutsuki-{}", &digest[..8])
}

pub(crate) fn self_subscription_id_for(platform: &str, actor_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{platform}:{actor_id}")));
    format!("self-{}", &digest[..12])
}

pub(crate) fn select_subscription(
    config: &BilibiliConfig,
    actor_id: &str,
    is_admin: bool,
    selector: Option<&str>,
) -> Result<usize, BilibiliError> {
    let matches = config
        .subscriptions
        .iter()
        .enumerate()
        .filter(|(_, subscription)| {
            is_admin || subscription.owner_user_id.as_deref() == Some(actor_id)
        })
        .filter(|(_, subscription)| {
            selector.is_none_or(|selector| {
                subscription.subscription_id == selector || subscription.uid.to_string() == selector
            })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(BilibiliError::ManagementUnavailable(
            "matching subscription was not found".into(),
        )),
        _ => Err(BilibiliError::ManagementUnavailable(
            "subscription selector is ambiguous".into(),
        )),
    }
}

fn outbound_task(parent: &Task, message: BotMessage, binding: &str, index: usize) -> Task {
    let mut task = Task::new(
        format!("{}:notify:{index}", parent.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).expect("BotMessage serializes"),
    );
    task.target_binding_id = Some(binding.into());
    task.trace_id = parent.trace_id.clone();
    task.correlation_id = parent
        .correlation_id
        .clone()
        .or_else(|| Some(parent.task_id.to_string()));
    task
}

fn bili_error(task: &Task, error: BilibiliError) -> RuntimeError {
    let code = match &error {
        BilibiliError::CookieExpired => "bilibili.cookie_expired",
        BilibiliError::RateLimited => "bilibili.rate_limited",
        BilibiliError::RiskControl352 => "bilibili.risk_control_352",
        BilibiliError::Forbidden => "bilibili.management_forbidden",
        BilibiliError::ManagementUnavailable(_) => "bilibili.management_unavailable",
        BilibiliError::OpenPlatformCredentialUnavailable(_)
        | BilibiliError::OpenPlatformCredentialInvalid(_) => "bilibili.open_platform.credentials",
        BilibiliError::OpenPlatformPermissionDenied { .. } => {
            "bilibili.open_platform.permission_denied"
        }
        BilibiliError::OpenPlatformOAuthExpired { .. } => "bilibili.open_platform.oauth_expired",
        BilibiliError::OpenPlatformSignatureRejected { .. } => {
            "bilibili.open_platform.signature_rejected"
        }
        BilibiliError::OpenPlatformApi { .. } => "bilibili.open_platform.api_failed",
        BilibiliError::OpenPlatformUnsupported(_) => {
            "bilibili.open_platform.unsupported_capability"
        }
        _ => "bilibili.request_failed",
    };
    let mut runtime = RuntimeError::new(code, PLUGIN_ID, format!("bilibili.{}", task.task_id));
    runtime
        .evidence
        .insert("detail".into(), ScalarValue::String(error.to_string()));
    match &error {
        BilibiliError::OpenPlatformPermissionDenied {
            code,
            scope,
            request_id,
        } => {
            runtime.evidence.insert(
                "open_platform_code".into(),
                ScalarValue::String(code.to_string()),
            );
            runtime
                .evidence
                .insert("required_scope".into(), ScalarValue::String(scope.clone()));
            if let Some(request_id) = request_id {
                runtime
                    .evidence
                    .insert("request_id".into(), ScalarValue::String(request_id.clone()));
            }
        }
        BilibiliError::OpenPlatformOAuthExpired {
            request_id: Some(request_id),
        } => {
            runtime
                .evidence
                .insert("request_id".into(), ScalarValue::String(request_id.clone()));
        }
        BilibiliError::OpenPlatformSignatureRejected { code, request_id }
        | BilibiliError::OpenPlatformApi {
            code, request_id, ..
        } => {
            runtime.evidence.insert(
                "open_platform_code".into(),
                ScalarValue::String(code.to_string()),
            );
            if let Some(request_id) = request_id {
                runtime
                    .evidence
                    .insert("request_id".into(), ScalarValue::String(request_id.clone()));
            }
        }
        _ => {}
    }
    if code == "bilibili.risk_control_352" {
        for (key, value) in [
            ("risk_control_code", "352"),
            ("fallback_status", "not_configured"),
            ("degraded", "true"),
        ] {
            runtime
                .evidence
                .insert(key.into(), ScalarValue::String(value.into()));
        }
    }
    runtime
}

fn bili_management_error(task: &Task, error: BilibiliManagementError) -> RuntimeError {
    let mut runtime =
        RuntimeError::new(error.code, PLUGIN_ID, format!("bilibili.{}", task.task_id));
    runtime
        .evidence
        .insert("detail".into(), ScalarValue::String(error.message));
    runtime
}

fn risk_control_failure(task: &Task, route: &str, detail: impl fmt::Display) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "bilibili.risk_control_fallback_failed",
        PLUGIN_ID,
        format!("bilibili.risk_control.{route}.{}", task.task_id),
    );
    for (key, value) in [
        ("risk_control_code", "352"),
        ("backend", "chromium"),
        ("fallback_status", "failed"),
        ("degraded", "true"),
    ] {
        error
            .evidence
            .insert(key.into(), ScalarValue::String(value.into()));
    }
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.to_string()));
    RuntimeFailure::new(error)
}

fn ensure_bilibili_domain(value: &str) -> Result<(), BilibiliError> {
    let url = Url::parse(value).map_err(|error| BilibiliError::DomainDenied(error.to_string()))?;
    allow_bilibili_url(&url)
}

pub(crate) fn allow_bilibili_url(url: &Url) -> Result<(), BilibiliError> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "b23.tv"
        || host == "bilibili.com"
        || host.ends_with(".bilibili.com")
        || host == "hdslb.com"
        || host.ends_with(".hdslb.com");
    if url.scheme() == "https" && allowed && url.username().is_empty() && url.password().is_none() {
        Ok(())
    } else {
        Err(BilibiliError::DomainDenied(host))
    }
}

fn credential_from_redirect(value: &str) -> Result<String, BilibiliError> {
    let url =
        Url::parse(value).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let allowed = [
        "SESSDATA",
        "bili_jct",
        "DedeUserID",
        "DedeUserID__ckMd5",
        "buvid3",
    ];
    let values = url
        .query_pairs()
        .filter(|(key, value)| allowed.contains(&key.as_ref()) && !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    if !values.iter().any(|value| value.starts_with("SESSDATA=")) {
        return Err(BilibiliError::InvalidResponse(
            "QR login response did not contain SESSDATA".into(),
        ));
    }
    Ok(values.join("; "))
}

fn string_field(value: &Value, field: &str) -> Result<String, BilibiliError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(field.into()))
}

fn parse_dynamic_snapshot(html: &str) -> Result<Vec<BilibiliItem>, BilibiliError> {
    let document = Html::parse_document(html);
    let cards = Selector::parse(".bili-dyn-list__item")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let links = Selector::parse("a[href]")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let images = Selector::parse("img[src]")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let mut items = Vec::new();
    for card in document.select(&cards) {
        let Some((id, url)) = card.select(&links).find_map(|link| {
            let href = link.value().attr("href")?;
            dynamic_id_and_url(href)
        }) else {
            continue;
        };
        let title = first_card_text(
            card,
            &[
                ".bili-rich-text",
                ".bili-dyn-card-video__title",
                ".bili-dyn-card-opus__summary",
            ],
        )
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "新动态".into())
        .chars()
        .take(80)
        .collect();
        let image_url = card
            .select(&images)
            .filter_map(|image| image.value().attr("src"))
            .filter_map(normalize_browser_url)
            .find(|url| ensure_bilibili_domain(url).is_ok());
        items.push(BilibiliItem {
            id,
            title,
            url,
            image_url,
        });
    }
    Ok(items)
}

fn first_card_text(card: ElementRef<'_>, selectors: &[&str]) -> Option<String> {
    selectors.iter().find_map(|selector| {
        let selector = Selector::parse(selector).ok()?;
        let text = card
            .select(&selector)
            .next()?
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then_some(text)
    })
}

fn dynamic_id_and_url(value: &str) -> Option<(String, String)> {
    let url = normalize_browser_url(value)?;
    ensure_bilibili_domain(&url).ok()?;
    let parsed = Url::parse(&url).ok()?;
    let host = parsed.host_str()?;
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let id = match (host, segments.as_slice()) {
        ("t.bilibili.com", [id]) => *id,
        ("bilibili.com" | "www.bilibili.com", ["opus", id]) => *id,
        _ => return None,
    };
    id.chars()
        .all(|character| character.is_ascii_digit())
        .then(|| (id.to_owned(), format!("https://t.bilibili.com/{id}")))
}

fn normalize_browser_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("//") {
        Some(format!("https:{value}"))
    } else if value.starts_with('/') {
        Some(format!("https://www.bilibili.com{value}"))
    } else if value.starts_with("https://") {
        Some(value.to_owned())
    } else {
        None
    }
}

fn parse_poll_items(
    kind: &BilibiliPollKind,
    uid: u64,
    value: Value,
) -> Result<Vec<BilibiliItem>, BilibiliError> {
    match kind {
        BilibiliPollKind::Live => {
            let live = &value["data"]["live_room"];
            let status = live
                .get("liveStatus")
                .or_else(|| live.get("live_status"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
                == 1;
            Ok(vec![BilibiliItem {
                id: status.to_string(),
                title: live
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("直播")
                    .into(),
                url: live
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(&format!("https://space.bilibili.com/{uid}"))
                    .into(),
                image_url: live
                    .get("cover")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }])
        }
        BilibiliPollKind::Dynamic => {
            let items = value["data"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok(items
                .into_iter()
                .filter_map(|item| {
                    let id = item.get("id_str")?.as_str()?.to_owned();
                    let modules = &item["modules"]["module_dynamic"];
                    Some(BilibiliItem {
                        id: id.clone(),
                        title: modules["desc"]["text"]
                            .as_str()
                            .unwrap_or("新动态")
                            .chars()
                            .take(80)
                            .collect(),
                        url: format!("https://t.bilibili.com/{id}"),
                        image_url: modules["major"]["archive"]["cover"]
                            .as_str()
                            .map(ToOwned::to_owned),
                    })
                })
                .collect())
        }
        BilibiliPollKind::Video => {
            let items = value["data"]["list"]["vlist"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok(items
                .into_iter()
                .filter_map(|item| {
                    let bvid = item.get("bvid")?.as_str()?.to_owned();
                    Some(BilibiliItem {
                        id: bvid.clone(),
                        title: item["title"].as_str().unwrap_or("新投稿").into(),
                        url: format!("https://www.bilibili.com/video/{bvid}"),
                        image_url: item["pic"].as_str().map(|url| format!("https:{url}")),
                    })
                })
                .collect())
        }
    }
}

fn fresh_since(items: Vec<BilibiliItem>, previous: &str) -> Vec<BilibiliItem> {
    let mut fresh = items
        .into_iter()
        .take_while(|item| item.id != previous)
        .collect::<Vec<_>>();
    fresh.reverse();
    fresh
}

pub fn sign_wbi_query(params: &[(String, String)], mixin_key: &str, unix_seconds: i64) -> String {
    let mut params = params.to_vec();
    params.push(("wts".into(), unix_seconds.to_string()));
    params.sort_by(|left, right| left.0.cmp(&right.0));
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            params
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .finish();
    let signature = format!("{:x}", md5::compute(format!("{encoded}{mixin_key}")));
    format!("{encoded}&w_rid={signature}")
}

fn wbi_mixin_key(img_url: &str, sub_url: &str) -> Result<String, BilibiliError> {
    const TABLE: [usize; 64] = [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19,
        29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
    ];
    let filename = |value: &str| {
        Url::parse(value)
            .ok()?
            .path_segments()?
            .next_back()?
            .split('.')
            .next()
            .map(ToOwned::to_owned)
    };
    let source = format!(
        "{}{}",
        filename(img_url).ok_or_else(|| BilibiliError::InvalidResponse("wbi img key".into()))?,
        filename(sub_url).ok_or_else(|| BilibiliError::InvalidResponse("wbi sub key".into()))?
    );
    let chars = source.chars().collect::<Vec<_>>();
    if chars.len() < 64 {
        return Err(BilibiliError::InvalidResponse("wbi key length".into()));
    }
    Ok(TABLE.iter().take(32).map(|index| chars[*index]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        BotAccountRef, BotEvent, BotEventKind, BotFlowDocument, BotFlowEdge, BotFlowEdgeKind,
        BotFlowNode, BotFlowNodePosition, BotFlowSourceSelector, BotNodeWiring, BotPlatform,
        BotUser,
    };
    use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, CommandPlan, DispatchLane, ExportPlan, OrderingRequirement,
        PlanReceipt, ReadPlan, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
        ResourceSealState, ResourceSemantic, SnapshotDescriptor, StreamPlan, TaskBatch, TaskHandle,
        WorkResourcePlan, WritePlan,
    };
    use mutsuki_runtime_sdk::{ResourcePlanGateway, RuntimeClient};

    #[derive(Default)]
    struct FakeTransportState {
        signature: String,
    }

    struct FakeTransport(Arc<Mutex<FakeTransportState>>);

    impl BilibiliTransport for FakeTransport {
        fn poll(
            &mut self,
            _kind: &BilibiliPollKind,
            uid: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            Ok(vec![BilibiliItem {
                id: "dynamic-1".into(),
                title: "latest".into(),
                url: format!("https://t.bilibili.com/{uid}"),
                image_url: None,
            }])
        }

        fn resolve(&mut self, _url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            unreachable!()
        }

        fn download(&mut self, _url: &str, _max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }

        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            Ok(BilibiliQrCode {
                url: "https://passport.bilibili.com/qr".into(),
                key: "qr-key".into(),
            })
        }

        fn qr_poll(&mut self, _key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            Ok(BilibiliQrPoll {
                status: BilibiliQrStatus::Confirmed,
                credential: Some("SESSDATA=ROTATED".into()),
            })
        }

        fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError> {
            Ok(BilibiliProfile {
                name: format!("user-{uid}"),
                signature: self.0.lock().unwrap().signature.clone(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingCredentialStore(Mutex<Vec<(String, String)>>);

    impl BilibiliCredentialStore for RecordingCredentialStore {
        fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
            self.0.lock().unwrap().push((key.into(), credential));
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingConfigStore(Mutex<Vec<BilibiliConfig>>);

    impl BilibiliConfigStore for RecordingConfigStore {
        fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
            self.0.lock().unwrap().push(config.clone());
            Ok(())
        }
    }

    struct UnusedResources;

    impl ResourcePlanGateway for UnusedResources {
        fn collect_read_plan(&self, _: &ReadPlan) -> RuntimeResult<Vec<u8>> {
            unreachable!()
        }
        fn snapshot_read_plan(
            &self,
            _: &ReadPlan,
            _: &str,
            _: &str,
        ) -> RuntimeResult<SnapshotDescriptor> {
            unreachable!()
        }
        fn open_stream_plan(&self, _: &ReadPlan) -> RuntimeResult<StreamPlan> {
            unreachable!()
        }
        fn execute_export_plan(&self, _: &ExportPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn commit_write_plan(&self, _: &WritePlan, _: Vec<u8>) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_plan(&self, _: &CommandPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_batch(&self, _: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
        fn execute_saga_plan(&self, _: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
    }

    impl ResourceRegistryGateway for UnusedResources {
        fn open_resource_descriptor(&self, _: &str) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_blob_resource(&self, _: &str, _: &str, _: Vec<u8>) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_cow_state_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_capability_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
    }

    struct SnapshotResources {
        bytes: Vec<u8>,
    }

    impl ResourcePlanGateway for SnapshotResources {
        fn collect_read_plan(&self, _: &ReadPlan) -> RuntimeResult<Vec<u8>> {
            Ok(self.bytes.clone())
        }
        fn snapshot_read_plan(
            &self,
            _: &ReadPlan,
            _: &str,
            _: &str,
        ) -> RuntimeResult<SnapshotDescriptor> {
            unreachable!()
        }
        fn open_stream_plan(&self, _: &ReadPlan) -> RuntimeResult<StreamPlan> {
            unreachable!()
        }
        fn execute_export_plan(&self, _: &ExportPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn commit_write_plan(&self, _: &WritePlan, _: Vec<u8>) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_plan(&self, _: &CommandPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_batch(&self, _: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
        fn execute_saga_plan(&self, _: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
    }

    impl ResourceRegistryGateway for SnapshotResources {
        fn open_resource_descriptor(&self, _: &str) -> RuntimeResult<ResourceRef> {
            Ok(snapshot_resource())
        }
        fn create_blob_resource(&self, _: &str, _: &str, _: Vec<u8>) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_cow_state_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> RuntimeResult<ResourceRef> {
            Ok(snapshot_resource())
        }
        fn create_capability_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
    }

    fn snapshot_resource() -> ResourceRef {
        ResourceRef {
            ref_id: "bilibili-risk-snapshot".into(),
            resource_id: ResourceId {
                kind_id: "browser.snapshot".into(),
                slot_id: "bilibili-risk-snapshot".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::CowVersionedState,
            provider_id: "memory".into(),
            resource_kind: "browser.snapshot".into(),
            schema: SNAPSHOT_SCHEMA.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: "memory".into(),
                method: "memory".into(),
            },
            size_hint: Some(0),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    struct CompletedChildClient;

    impl RuntimeClient for CompletedChildClient {
        fn submit_batch(&self, _: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            unreachable!()
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(Some(TaskOutcome::Completed {
                task_id: handle.task_id.clone(),
                output: None,
                output_ref: None,
            }))
        }
    }

    struct RenderedChildClient;

    impl RuntimeClient for RenderedChildClient {
        fn submit_batch(&self, _: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            unreachable!()
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(Some(TaskOutcome::Completed {
                task_id: handle.task_id.clone(),
                output: Some(
                    serde_json::to_value(ImageRenderResponse {
                        resource: rendered_image_resource(),
                        width: 1200,
                        height: 630,
                        byte_len: 128,
                    })
                    .unwrap(),
                ),
                output_ref: None,
            }))
        }
    }

    fn rendered_image_resource() -> ResourceRef {
        let mut resource = snapshot_resource();
        resource.ref_id = "bilibili-rendered-card".into();
        resource.resource_id.kind_id = "blob".into();
        resource.resource_id.slot_id = "bilibili-rendered-card".into();
        resource.resource_kind = "blob".into();
        resource.schema = mutsuki_protocol_image::PNG_SCHEMA.into();
        resource.semantic = ResourceSemantic::FrozenValue;
        resource
    }

    struct LinkTransport;

    impl BilibiliTransport for LinkTransport {
        fn poll(
            &mut self,
            _: &BilibiliPollKind,
            _: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            unreachable!()
        }
        fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            Ok(ResolvedLinkCard {
                url: url.into(),
                title: "视频标题".into(),
                description: "视频简介".into(),
                image_url: None,
            })
        }
        fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }
        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            unreachable!()
        }
        fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            unreachable!()
        }
        fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
            unreachable!()
        }
    }

    struct MultiPollTransport;

    impl BilibiliTransport for MultiPollTransport {
        fn poll(
            &mut self,
            _: &BilibiliPollKind,
            _: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            Ok(["3", "2", "1"]
                .into_iter()
                .map(|id| BilibiliItem {
                    id: id.into(),
                    title: format!("动态 {id}"),
                    url: format!("https://t.bilibili.com/{id}"),
                    image_url: None,
                })
                .collect())
        }
        fn resolve(&mut self, _: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            unreachable!()
        }
        fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }
        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            unreachable!()
        }
        fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            unreachable!()
        }
        fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
            unreachable!()
        }
    }

    struct RiskControlledTransport;

    impl BilibiliTransport for RiskControlledTransport {
        fn poll(
            &mut self,
            _: &BilibiliPollKind,
            _: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            Err(BilibiliError::RiskControl352)
        }
        fn resolve(&mut self, _: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            unreachable!()
        }
        fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }
        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            unreachable!()
        }
        fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            unreachable!()
        }
        fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
            unreachable!()
        }
    }

    #[test]
    fn wbi_signature_is_deterministic_with_fixed_clock() {
        let signed = sign_wbi_query(&[("mid".into(), "1".into())], "secret", 1_700_000_000);
        assert_eq!(
            signed,
            sign_wbi_query(&[("mid".into(), "1".into())], "secret", 1_700_000_000)
        );
        assert!(signed.contains("wts=1700000000&w_rid="));
    }

    #[test]
    fn credential_debug_and_errors_never_expose_cookie() {
        let credential = SharedBilibiliCredential::default();
        credential.set("SESSDATA=top-secret".into());
        assert!(!format!("{credential:?}").contains("top-secret"));
        assert!(
            !BilibiliError::CookieExpired
                .to_string()
                .contains("SESSDATA")
        );
    }

    #[test]
    fn qr_redirect_extracts_only_cookie_fields_and_requires_sessdata() {
        let credential = credential_from_redirect(
            "https://www.bilibili.com/?SESSDATA=abc%3D%3D&bili_jct=csrf&ignored=value",
        )
        .unwrap();
        assert_eq!(credential, "SESSDATA=abc==; bili_jct=csrf");
        assert!(credential_from_redirect("https://www.bilibili.com/?bili_jct=x").is_err());
    }

    #[test]
    fn management_manifest_exposes_a_behavior_node_without_a_command_name() {
        let base = manifest();
        assert!(
            base.requires
                .contains(&SurfaceRequirement::task_protocol(CARD_RENDER))
        );
        assert!(
            !base
                .requires
                .contains(&SurfaceRequirement::task_protocol(QR_RENDER))
        );
        let base_nodes = BotNodeCatalogFragment::from_plugin_extension(
            base.provides.extensions.first().unwrap(),
        )
        .unwrap()
        .unwrap();
        #[allow(clippy::items_after_statements)]
        fn node_ids(nodes: &BotNodeCatalogFragment) -> Vec<&str> {
            nodes
                .nodes
                .iter()
                .map(|node| node.node_type_id.as_str())
                .collect()
        }
        assert_eq!(
            node_ids(&base_nodes),
            [
                BILIBILI_NOTIFICATION_NODE_TYPE,
                BILIBILI_CARD_NODE_TYPE,
                "mutsuki.bot.bilibili.resolve"
            ]
        );
        let notification = &base_nodes.nodes[0];
        assert_eq!(notification.role, BotNodeRole::Source);
        assert!(notification.binding.is_none());
        let notification_port = &notification.ports[0];
        assert_eq!(notification_port.direction, BotNodePortDirection::Output);
        assert_eq!(notification_port.event_type.type_id, BILIBILI_EVENT_TYPE);
        assert_eq!(notification_port.event_type.version, 1);
        let card = &base_nodes.nodes[1];
        assert_eq!(card.role, BotNodeRole::Processor);
        assert_eq!(card.binding.as_ref().unwrap().protocol_id, NOTIFY_CARD);
        assert_eq!(card.ports[0].direction, BotNodePortDirection::Input);
        assert_eq!(card.ports[0].event_type.type_id, BILIBILI_EVENT_TYPE);
        assert_eq!(card.ports[1].event_type.type_id, "mutsuki.bot.message.send");

        let mut config = managed_config();
        let managed = manifest_for_config(&config);
        assert!(
            managed
                .requires
                .contains(&SurfaceRequirement::task_protocol(QR_RENDER))
        );
        assert!(
            managed.provides.runners[0]
                .accepted_protocol_ids
                .iter()
                .any(|protocol_id| protocol_id == MANAGEMENT_COMMAND)
        );
        let extension = managed.provides.extensions.first().unwrap();
        let nodes = BotNodeCatalogFragment::from_plugin_extension(extension)
            .unwrap()
            .unwrap();
        assert_eq!(
            node_ids(&nodes),
            [
                BILIBILI_NOTIFICATION_NODE_TYPE,
                BILIBILI_CARD_NODE_TYPE,
                "mutsuki.bot.bilibili.resolve",
                "mutsuki.bot.bilibili.management"
            ]
        );
        assert_eq!(
            nodes.nodes[3].binding.as_ref().unwrap().protocol_id,
            MANAGEMENT_COMMAND
        );
        assert!(
            managed.provides.runners[0]
                .accepted_protocol_ids
                .iter()
                .all(
                    |protocol_id| managed.provides.protocol_classes.get(protocol_id.as_str())
                        == Some(&ProtocolClass::Effect)
                )
        );

        config.risk_control = Some(BilibiliRiskControlConfig {
            backend: BilibiliRiskControlBackend::Chromium,
            timeout_ms: 1_000,
            max_response_bytes: 1024,
        });
        let risk_control = manifest_for_config(&config);
        assert!(
            risk_control
                .requires
                .contains(&SurfaceRequirement::task_protocol(SNAPSHOT))
        );
        assert_eq!(
            risk_control.provides.runners[0].execution_class,
            ExecutionClass::Orchestration
        );
    }

    #[test]
    fn dynamic_snapshot_parser_keeps_bilibili_urls_and_normalizes_cards() {
        let items = parse_dynamic_snapshot(
            r#"<article class="bili-dyn-list__item">
                <a href="https://www.bilibili.com/opus/123456">detail</a>
                <div class="bili-rich-text">  hello   browser fallback </div>
                <img src="//i0.hdslb.com/bfs/archive/cover.jpg">
            </article>
            <article class="bili-dyn-list__item">
                <a href="https://evil.example/opus/999">denied</a>
            </article>"#,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "123456");
        assert_eq!(items[0].title, "hello browser fallback");
        assert_eq!(items[0].url, "https://t.bilibili.com/123456");
        assert_eq!(
            items[0].image_url.as_deref(),
            Some("https://i0.hdslb.com/bfs/archive/cover.jpg")
        );
    }

    #[test]
    fn explicit_chromium_backend_awaits_snapshot_and_reports_degraded_success() {
        let html = r#"<article class="bili-dyn-list__item">
            <a href="https://t.bilibili.com/42">detail</a>
            <div class="bili-rich-text">fallback item</div>
        </article>"#;
        let bytes = serde_json::to_vec(&BrowserSnapshot {
            final_url: "https://space.bilibili.com/7/dynamic".into(),
            title: "space".into(),
            html: html.into(),
        })
        .unwrap();
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let runner = BilibiliRunner::new(
            Box::new(RiskControlledTransport),
            repository.clone(),
            Arc::new(SnapshotResources { bytes }),
            "memory",
        );
        let mut runner = runner.into_runtime_runner(
            Arc::new(CompletedChildClient),
            Some(BilibiliRiskControlConfig {
                backend: BilibiliRiskControlBackend::Chromium,
                timeout_ms: 5_000,
                max_response_bytes: 64 * 1024,
            }),
        );
        let task = Task::new(
            "risk-control",
            POLL_DYNAMIC,
            serde_json::to_value(PollRequest {
                subscription_id: "sub".into(),
                uid: 7,
                target: BotTarget::Group {
                    group_id: "group".into(),
                },
                outbound_binding: "qq-main".into(),
            })
            .unwrap(),
        );
        let batch = command_batch(vec![task]);
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
        let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
        let waiting = waiting.results[0].result.as_ref().unwrap();
        assert_eq!(waiting.tasks[0].protocol_id, SNAPSHOT);
        assert!(waiting.task_await.is_some());

        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        assert!(
            completed
                .events
                .iter()
                .any(|event| event.kind == RISK_CONTROL_STATUS_EVENT
                    && event.payload["fallback"] == "succeeded")
        );
        assert_eq!(
            repository.cursor("Dynamic:7:sub").unwrap().as_deref(),
            Some("42")
        );
    }

    #[test]
    fn link_resolution_awaits_card_renderer_then_sends_image_and_url() {
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let mut runner = BilibiliRunner::new(
            Box::new(LinkTransport),
            repository.clone(),
            Arc::new(UnusedResources),
            "memory",
        )
        .into_runtime_runner(Arc::new(RenderedChildClient), None);
        let task = Task::new(
            "link-card",
            LINK_RESOLVE,
            serde_json::to_value(LinkResolveRequest {
                url: "https://www.bilibili.com/video/BV1Visual".into(),
                target: BotTarget::Group {
                    group_id: "group".into(),
                },
                outbound_binding: "qq-main".into(),
                account_id: "account".into(),
                now_ms: 1_000,
                cooldown_ms: 60_000,
            })
            .unwrap(),
        );
        let batch = command_batch(vec![task]);
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
        let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
        let waiting = waiting.results[0].result.as_ref().unwrap();
        assert_eq!(waiting.tasks[0].protocol_id, CARD_RENDER);
        let request: CardRenderRequest =
            serde_json::from_value(waiting.tasks[0].payload.to_value()).unwrap();
        assert_eq!(request.brand, "哔哩哔哩");
        assert_eq!(request.layout, CardLayout::Media);
        assert_eq!(request.kicker, "投稿");
        assert_eq!(
            request.fallback_gradient.start,
            Rgba {
                red: 251,
                green: 114,
                blue: 153,
                alpha: 255,
            }
        );
        assert!(waiting.task_await.is_some());
        assert!(
            repository
                .cooldown_ready(
                    "account:https://www.bilibili.com/video/BV1Visual",
                    1_000,
                    60_000,
                )
                .unwrap()
        );

        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        assert_eq!(completed.tasks.len(), 1);
        let message: BotMessage =
            serde_json::from_value(completed.tasks[0].payload.to_value()).unwrap();
        assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
        assert_eq!(
            message.segments[1],
            MessageSegment::Text {
                text: "https://www.bilibili.com/video/BV1Visual".into(),
            }
        );
        assert!(
            !repository
                .cooldown_ready(
                    "account:https://www.bilibili.com/video/BV1Visual",
                    1_000,
                    60_000,
                )
                .unwrap()
        );
    }

    #[test]
    fn link_resolution_from_flow_invocation_reads_event_url() {
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let mut runner = BilibiliRunner::new(
            Box::new(LinkTransport),
            repository,
            Arc::new(UnusedResources),
            "memory",
        )
        .into_runtime_runner(Arc::new(RenderedChildClient), None);
        let event = BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "account".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1_000,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            actor: None,
            message: Some(BotMessage::text(
                BotTarget::Group {
                    group_id: "group".into(),
                },
                "https://www.bilibili.com/video/BV1Visual",
            )),
            raw: None,
            ext: Default::default(),
        };
        let invocation = BotNodeInvocation {
            flow_id: "link".into(),
            graph_revision: 1,
            execution_id: "exec".into(),
            node_id: "bili".into(),
            input_port_id: "event".into(),
            wiring: BotNodeWiring::default(),
            config: json!({"outbound_binding": "qq-main", "cooldown_ms": 60_000}),
            input: BotFlowEventEnvelope {
                event_id: "e1".into(),
                protocol_id: mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                    value: serde_json::to_value(&event).unwrap(),
                },
                context: mutsuki_bot_protocol::BotFlowContext {
                    bot: Some(event.bot.clone()),
                    target: Some(event.target.clone()),
                    actor: None,
                    ext: Default::default(),
                },
                trace_id: None,
                correlation_id: None,
            },
        };
        let task = Task::new(
            "link-flow",
            LINK_RESOLVE,
            serde_json::to_value(invocation).unwrap(),
        );
        let batch = command_batch(vec![task]);
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
        let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
        assert_eq!(
            waiting.results[0].result.as_ref().unwrap().tasks[0].protocol_id,
            CARD_RENDER
        );
        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        assert!(completed.tasks.is_empty());
        let node: BotNodeResult =
            serde_json::from_value(completed.output.clone().unwrap()).unwrap();
        assert_eq!(node.outputs[0].port_id, "message");
        let message: BotMessage =
            serde_json::from_value(node.outputs[0].event.payload.value.clone()).unwrap();
        assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
    }

    #[test]
    fn login_command_awaits_qr_renderer_then_sends_rendered_resource() {
        let state = Arc::new(Mutex::new(FakeTransportState::default()));
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let config = SharedBilibiliConfig::new(managed_config());
        let management = Arc::new(BilibiliManagementService::new(
            config.clone(),
            SharedBilibiliCredential::default(),
            Box::new(FakeTransport(state.clone())),
            repository.clone(),
            Arc::new(RecordingCredentialStore::default()),
            Arc::new(RecordingConfigStore::default()),
            Arc::new(AlwaysPresentSecrets),
        ));
        let mut runner = BilibiliRunner::new(
            Box::new(FakeTransport(state)),
            repository,
            Arc::new(UnusedResources),
            "memory",
        )
        .with_management(config, management)
        .into_runtime_runner(Arc::new(RenderedChildClient), None);
        let task = command_task("login-render", "admin", &["login"]);
        let batch = command_batch(vec![task]);
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
        let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
        let waiting = waiting.results[0].result.as_ref().unwrap();
        assert_eq!(waiting.tasks[0].protocol_id, QR_RENDER);
        let request: QrRenderRequest =
            serde_json::from_value(waiting.tasks[0].payload.to_value()).unwrap();
        assert_eq!(request.content, "https://passport.bilibili.com/qr");
        assert_eq!(request.min_dimensions, 256);

        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        let message: BotMessage =
            serde_json::from_value(completed.tasks[0].payload.to_value()).unwrap();
        assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
        assert!(matches!(message.segments[1], MessageSegment::Text { .. }));
    }

    #[test]
    fn multiple_fresh_items_are_emitted_as_flow_events_in_order() {
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        repository.set_cursor("Dynamic:7:sub", "1").unwrap();
        let mut runner = BilibiliRunner::new(
            Box::new(MultiPollTransport),
            repository.clone(),
            Arc::new(UnusedResources),
            "memory",
        )
        .into_runtime_runner(Arc::new(CompletedChildClient), None);
        let task = Task::new(
            "multi-notify",
            POLL_DYNAMIC,
            serde_json::to_value(PollRequest {
                subscription_id: "sub".into(),
                uid: 7,
                target: BotTarget::Group {
                    group_id: "group".into(),
                },
                outbound_binding: "qq-main".into(),
            })
            .unwrap(),
        );
        let batch = command_batch(vec![task]);
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);

        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        assert_eq!(completed.tasks.len(), 2);
        assert!(
            completed
                .tasks
                .iter()
                .all(|task| task.protocol_id == BOT_FLOW_INGRESS_PROTOCOL_ID)
        );
        let urls = completed
            .tasks
            .iter()
            .map(|task| {
                let envelope: BotFlowEventEnvelope =
                    serde_json::from_value(task.payload.to_value()).unwrap();
                assert_eq!(
                    envelope.protocol_id,
                    mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID
                );
                assert_eq!(envelope.payload.event_type.type_id, BILIBILI_EVENT_TYPE);
                let notification: BilibiliNotification =
                    serde_json::from_value(envelope.payload.value).unwrap();
                assert_eq!(notification.kind, BilibiliPollKind::Dynamic);
                assert_eq!(notification.uid, 7);
                assert_eq!(
                    notification.target,
                    BotTarget::Group {
                        group_id: "group".into(),
                    }
                );
                notification.url
            })
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            vec![
                "https://t.bilibili.com/2".to_string(),
                "https://t.bilibili.com/3".to_string(),
            ]
        );
        assert_eq!(repository.cursor("Dynamic:7:sub").unwrap().unwrap(), "3");
    }

    #[test]
    fn notification_card_node_renders_subscription_payload_into_message_send() {
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let mut runner = BilibiliRunner::new(
            Box::new(LinkTransport),
            repository,
            Arc::new(UnusedResources),
            "memory",
        )
        .into_runtime_runner(Arc::new(RenderedChildClient), None);
        let notification = BilibiliNotification {
            kind: BilibiliPollKind::Live,
            subscription_id: "sub".into(),
            uid: 42,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            item_id: "true".into(),
            title: "晚上 eight 点开播".into(),
            url: "https://live.bilibili.com/1000".into(),
            image_url: None,
        };
        let invocation = BotNodeInvocation {
            flow_id: "push".into(),
            graph_revision: 1,
            execution_id: "exec".into(),
            node_id: "card".into(),
            input_port_id: "event".into(),
            wiring: BotNodeWiring::default(),
            config: json!({}),
            input: BotFlowEventEnvelope {
                event_id: "notify-1".into(),
                protocol_id: mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
                    value: serde_json::to_value(&notification).unwrap(),
                },
                context: mutsuki_bot_protocol::BotFlowContext {
                    bot: None,
                    target: Some(notification.target.clone()),
                    actor: None,
                    ext: BotExtMap::new(),
                },
                trace_id: None,
                correlation_id: None,
            },
        };
        let task = Task::new(
            "notify-card",
            NOTIFY_CARD,
            serde_json::to_value(invocation).unwrap(),
        );
        let batch = command_batch(vec![task]);
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);

        let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
        let waiting = waiting.results[0].result.as_ref().unwrap();
        assert_eq!(waiting.tasks[0].protocol_id, CARD_RENDER);
        let request: CardRenderRequest =
            serde_json::from_value(waiting.tasks[0].payload.to_value()).unwrap();
        assert_eq!(request.layout, CardLayout::Row);
        assert_eq!(request.kicker, "直播");
        assert!(request.live);
        assert_eq!(request.cover, None);
        assert!(waiting.task_await.is_some());

        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        let output = completed.output.as_ref().unwrap();
        let node_result: BotNodeResult = serde_json::from_value(output.clone()).unwrap();
        assert_eq!(node_result.outputs.len(), 1);
        let output = &node_result.outputs[0];
        assert_eq!(output.port_id, "message");
        assert_eq!(
            output.event.payload.event_type.type_id,
            "mutsuki.bot.message.send"
        );
        let message: BotMessage =
            serde_json::from_value(output.event.payload.value.clone()).unwrap();
        assert_eq!(
            message.target,
            BotTarget::Group {
                group_id: "group".into(),
            }
        );
        assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
        assert_eq!(
            message.segments[1],
            MessageSegment::Text {
                text: "https://live.bilibili.com/1000".into(),
            }
        );
    }

    #[test]
    fn management_flow_rotates_secret_persists_verified_binding_and_previews_without_cursor() {
        let state = Arc::new(Mutex::new(FakeTransportState::default()));
        let config = SharedBilibiliConfig::new(managed_config());
        let credential = SharedBilibiliCredential::default();
        let credential_store = Arc::new(RecordingCredentialStore::default());
        let config_store = Arc::new(RecordingConfigStore::default());
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let management = Arc::new(BilibiliManagementService::new(
            config.clone(),
            credential,
            Box::new(FakeTransport(state.clone())),
            repository.clone(),
            credential_store.clone(),
            config_store.clone(),
            Arc::new(AlwaysPresentSecrets),
        ));
        let mut runner = BilibiliRunner::new(
            Box::new(FakeTransport(state.clone())),
            repository.clone(),
            Arc::new(UnusedResources),
            "memory",
        )
        .with_management(config.clone(), management.clone());

        repository.set_qr_session("admin", "qr-key").unwrap();
        let login = runner
            .run_command(&command_task("login", "admin", &["login-status"]))
            .unwrap();
        assert!(
            runner
                .run_command(&command_task("forbidden-login", "alice", &["login-status"]))
                .is_err()
        );
        assert_eq!(credential_store.0.lock().unwrap().len(), 1);
        assert!(
            !serde_json::to_string(&login.tasks)
                .unwrap()
                .contains("ROTATED")
        );

        runner
            .run_command(&command_task("bind", "alice", &["bind", "42"]))
            .unwrap();
        let (_, code) = repository.binding_challenge("alice").unwrap().unwrap();
        state.lock().unwrap().signature = format!("hello {code}");
        runner
            .run_command(&command_task("verify", "alice", &["verify"]))
            .unwrap();
        let snapshot = config.snapshot();
        assert_eq!(snapshot.subscriptions.len(), 1);
        assert_eq!(snapshot.subscriptions[0].uid, 42);
        assert_eq!(
            snapshot.subscriptions[0].owner_user_id.as_deref(),
            Some("alice")
        );

        runner
            .run_command(&command_task("pause", "alice", &["pause"]))
            .unwrap();
        assert!(config.snapshot().subscriptions[0].paused);
        assert!(config_store.0.lock().unwrap().len() >= 2);

        assert!(repository.cursor("Dynamic:42").unwrap().is_none());
        let preview = management.preview("alice", false, None).unwrap();
        assert_eq!(preview.title, "latest");
        assert!(repository.cursor("Dynamic:42").unwrap().is_none());
    }

    #[tokio::test]
    async fn management_service_web_subscribe_and_clear_never_echo_cookie() {
        let config = SharedBilibiliConfig::new(managed_config());
        let credential = SharedBilibiliCredential::default();
        credential.set("SESSDATA=secret-cookie".into());
        let credential_store = Arc::new(RecordingCredentialStore::default());
        let config_store = Arc::new(RecordingConfigStore::default());
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let management = BilibiliManagementService::new(
            config.clone(),
            credential.clone(),
            Box::new(FakeTransport(Arc::new(Mutex::new(
                FakeTransportState::default(),
            )))),
            repository,
            credential_store.clone(),
            config_store.clone(),
            Arc::new(AlwaysPresentSecrets),
        );
        let status = management.status();
        let mut changes = management
            .subscribe_changes()
            .expect("Bilibili change source");
        assert!(status.available);
        assert!(status.credential_loaded);
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("secret-cookie")
        );

        let view = management
            .subscribe(
                "sub-1".into(),
                7,
                vec![BilibiliNotificationKind::Live],
                BotTarget::Group {
                    group_id: "g1".into(),
                },
                "qq-main".into(),
            )
            .unwrap();
        assert_eq!(view.subscription_id, "sub-1");
        assert_eq!(config.snapshot().subscriptions.len(), 1);
        assert_eq!(config_store.0.lock().unwrap().len(), 1);
        let changed = changes.changed().await.expect("subscription change");
        assert!(
            changed
                .areas
                .contains(&mutsuki_bot_management::BilibiliManagementChangeArea::Subscriptions)
        );

        management.credential_clear().unwrap();
        assert!(!credential.is_loaded());
        assert_eq!(credential_store.0.lock().unwrap().last().unwrap().1, "");
    }

    #[test]
    fn management_service_subscribe_rejects_duplicate_id_and_empty_target() {
        let mut seed = managed_config();
        seed.subscriptions.push(BilibiliSubscription {
            subscription_id: "sub-1".into(),
            uid: 42,
            notifications: vec![BilibiliPollKind::Dynamic],
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            outbound_binding: "qq-main".into(),
            paused: false,
            owner_user_id: Some("alice".into()),
        });
        let config = SharedBilibiliConfig::new(seed);
        let config_store = Arc::new(RecordingConfigStore::default());
        let management = BilibiliManagementService::new(
            config.clone(),
            SharedBilibiliCredential::default(),
            Box::new(FakeTransport(Arc::new(Mutex::new(
                FakeTransportState::default(),
            )))),
            Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
            Arc::new(RecordingCredentialStore::default()),
            config_store.clone(),
            Arc::new(AlwaysPresentSecrets),
        );

        assert_eq!(
            management
                .subscribe(
                    "sub-1".into(),
                    7,
                    vec![BilibiliNotificationKind::Live],
                    BotTarget::Group {
                        group_id: "g2".into(),
                    },
                    "qq-main".into(),
                )
                .unwrap_err()
                .code,
            "bilibili.request_failed"
        );
        assert_eq!(
            config.snapshot().subscriptions[0].owner_user_id.as_deref(),
            Some("alice")
        );

        assert_eq!(
            management
                .subscribe(
                    "sub-2".into(),
                    7,
                    vec![BilibiliNotificationKind::Live],
                    BotTarget::Group {
                        group_id: "".into(),
                    },
                    "qq-main".into(),
                )
                .unwrap_err()
                .code,
            "bilibili.request_failed"
        );
        assert_eq!(config.snapshot().subscriptions.len(), 1);
        assert!(config_store.0.lock().unwrap().is_empty());
    }

    struct AlwaysPresentSecrets;

    impl BilibiliSecretPresence for AlwaysPresentSecrets {
        fn inspect(&self, _key: &str) -> BilibiliCredentialSecretState {
            BilibiliCredentialSecretState::Present
        }
    }

    struct FakeQrRenderer;

    #[async_trait]
    impl BilibiliQrRenderer for FakeQrRenderer {
        async fn render_qr(&self, content: &str) -> Result<Vec<u8>, BilibiliError> {
            assert_eq!(content, "https://passport.bilibili.com/qr");
            Ok(vec![1, 2, 3, 4])
        }
    }

    #[tokio::test]
    async fn management_login_uses_bound_renderer_for_web_console_payload() {
        let management = BilibiliManagementService::new(
            SharedBilibiliConfig::new(managed_config()),
            SharedBilibiliCredential::default(),
            Box::new(FakeTransport(Arc::new(Mutex::new(
                FakeTransportState::default(),
            )))),
            Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
            Arc::new(RecordingCredentialStore::default()),
            Arc::new(RecordingConfigStore::default()),
            Arc::new(AlwaysPresentSecrets),
        );
        management.bind_qr_renderer(Arc::new(FakeQrRenderer));
        let result = management.login_start("web-console").await.unwrap();
        assert_eq!(result.url, "https://passport.bilibili.com/qr");
        assert_eq!(result.qr_png, vec![1, 2, 3, 4]);
        assert_eq!(result.qr_png_base64, "AQIDBA==");
    }

    fn managed_config() -> BilibiliConfig {
        BilibiliConfig {
            backend: BilibiliBackendConfig::WebCookie {
                cookie_secret_key: "BILIBILI_COOKIE".into(),
            },
            live_interval_ms: 1_000,
            dynamic_interval_ms: 1_000,
            video_interval_ms: 1_000,
            retry: RetryConfig {
                max_attempts: 3,
                initial_backoff_ms: 10,
                max_backoff_ms: 100,
            },
            subscriptions: Vec::new(),
            link_resolver: LinkResolverConfig {
                enabled: false,
                cooldown_ms: 1_000,
                account_to_binding: BTreeMap::new(),
            },
            media_provider_id: "memory".into(),
            risk_control: None,
            management: BilibiliManagementConfig {
                enabled: true,
                allow_self_binding: true,
                admin_user_ids: vec!["admin".into()],
                self_binding_notifications: vec![BilibiliPollKind::Dynamic],
                self_binding_outbound_binding: "qq-main".into(),
            },
        }
    }

    fn command_task(task_id: &str, actor_id: &str, args: &[&str]) -> Task {
        let target = BotTarget::Group {
            group_id: "group".into(),
        };
        Task::new(
            task_id,
            MANAGEMENT_COMMAND,
            serde_json::to_value(BotCommandEvent {
                source: BotEvent {
                    event_id: format!("event-{task_id}"),
                    platform: BotPlatform::QqBot,
                    bot: BotAccountRef {
                        account_id: "bot".into(),
                        platform: BotPlatform::QqBot,
                    },
                    kind: BotEventKind::MessageCreated,
                    time_ms: 1,
                    target: target.clone(),
                    actor: Some(BotUser {
                        user_id: actor_id.into(),
                        display_name: None,
                        avatar_url: None,
                    }),
                    message: Some(BotMessage::text(target, "/bili")),
                    raw: None,
                    ext: BotExtMap::new(),
                },
                name: "bili".into(),
                args: args.iter().map(|value| (*value).into()).collect(),
                command_path: vec!["bili".into()],
                typed_args: Default::default(),
                raw_text: format!("/bili {}", args.join(" ")),
            })
            .unwrap(),
        )
    }

    fn command_batch(tasks: Vec<Task>) -> WorkBatch {
        WorkBatch {
            batch_id: "batch".into(),
            tick_id: "tick".into(),
            batch_key: RUNNER_ID.into(),
            entries: tasks
                .iter()
                .enumerate()
                .map(|(index, task)| BatchEntry {
                    entry_id: format!("entry-{index}").into(),
                    task_id: task.task_id.clone(),
                    trace_id: None,
                    parent_id: None,
                    payload_index: index,
                    resource_requirement_indices: Vec::new(),
                    cancel_index: None,
                    deadline_tick: None,
                    priority: 0,
                    lane: DispatchLane::Normal,
                    ordering: OrderingRequirement::PreserveSubmitOrder,
                })
                .collect(),
            payload: BatchPayload::from_tasks(&tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    #[test]
    fn first_cursor_is_persisted_without_history() {
        let repo = SqliteBilibiliRepository::open(":memory:").unwrap();
        assert!(repo.cursor("dynamic:1").unwrap().is_none());
        repo.set_cursor("dynamic:1", "newest").unwrap();
        assert_eq!(repo.cursor("dynamic:1").unwrap().as_deref(), Some("newest"));
    }

    #[test]
    fn dynamic_and_video_items_are_emitted_oldest_first_after_cursor() {
        let item = |id: &str| BilibiliItem {
            id: id.into(),
            title: id.into(),
            url: format!("https://www.bilibili.com/{id}"),
            image_url: None,
        };
        let fresh = fresh_since(vec![item("3"), item("2"), item("1")], "1");
        assert_eq!(
            fresh
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "3"]
        );
    }

    #[test]
    fn cooldown_is_persisted_in_sqlite() {
        let repo = SqliteBilibiliRepository::open(":memory:").unwrap();
        assert!(repo.cooldown_ready("account:url", 100, 50).unwrap());
        repo.record_cooldown("account:url", 100).unwrap();
        assert!(!repo.cooldown_ready("account:url", 120, 50).unwrap());
        assert!(repo.cooldown_ready("account:url", 151, 50).unwrap());
    }

    #[test]
    fn rate_limit_cookie_and_352_have_distinct_runtime_codes() {
        let task = Task::new("error", POLL_LIVE, Value::Null);
        assert_eq!(
            bili_error(&task, BilibiliError::RateLimited).code,
            "bilibili.rate_limited"
        );
        assert_eq!(
            bili_error(&task, BilibiliError::CookieExpired).code,
            "bilibili.cookie_expired"
        );
        let risk_control = bili_error(&task, BilibiliError::RiskControl352);
        assert_eq!(risk_control.code, "bilibili.risk_control_352");
        assert_eq!(
            risk_control.evidence.get("fallback_status"),
            Some(&ScalarValue::String("not_configured".into()))
        );
    }

    #[test]
    fn risk_control_config_rejects_unbounded_limits() {
        let mut config = managed_config();
        config.risk_control = Some(BilibiliRiskControlConfig {
            backend: BilibiliRiskControlBackend::Chromium,
            timeout_ms: 0,
            max_response_bytes: 1024,
        });
        assert!(config.validate().unwrap_err().contains("timeout_ms"));
        config.risk_control.as_mut().unwrap().timeout_ms = 1000;
        config.risk_control.as_mut().unwrap().max_response_bytes = 0;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("max_response_bytes")
        );
    }

    #[test]
    fn open_platform_config_rejects_web_only_capabilities_and_wrong_uid() {
        let mut config = managed_config();
        config.backend = BilibiliBackendConfig::OpenPlatform {
            client_id: "client".into(),
            app_secret_key: "BILIBILI_OPEN_APP_SECRET".into(),
            oauth_credential_key: "BILIBILI_OPEN_OAUTH".into(),
            authorized_uid: 42,
        };
        assert!(config.validate().unwrap_err().contains("Cookie management"));
        config.management = BilibiliManagementConfig::default();
        config.subscriptions.push(BilibiliSubscription {
            subscription_id: "dynamic".into(),
            uid: 42,
            notifications: vec![BilibiliPollKind::Dynamic],
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
            paused: false,
            owner_user_id: None,
        });
        assert!(config.validate().unwrap_err().contains("poll/dynamic"));
        config.subscriptions[0].notifications = vec![BilibiliPollKind::Video];
        config.subscriptions[0].uid = 7;
        assert!(config.validate().unwrap_err().contains("authorized_uid"));
        config.subscriptions[0].uid = 42;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn open_platform_manifest_advertises_only_live_and_video() {
        let mut config = managed_config();
        config.backend = BilibiliBackendConfig::OpenPlatform {
            client_id: "client".into(),
            app_secret_key: "BILIBILI_OPEN_APP_SECRET".into(),
            oauth_credential_key: "BILIBILI_OPEN_OAUTH".into(),
            authorized_uid: 42,
        };
        config.management = BilibiliManagementConfig::default();
        let manifest = manifest_for_config(&config);
        let protocols = manifest
            .provides
            .protocols
            .iter()
            .map(|protocol| protocol.protocol_id.as_str())
            .collect::<Vec<_>>();
        assert!(protocols.contains(&POLL_LIVE));
        assert!(protocols.contains(&POLL_VIDEO));
        assert!(protocols.contains(&NOTIFY_CARD));
        assert!(!protocols.contains(&POLL_DYNAMIC));
        assert!(!protocols.contains(&LINK_RESOLVE));
        let nodes = BotNodeCatalogFragment::from_plugin_extension(
            manifest.provides.extensions.first().unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            nodes
                .nodes
                .iter()
                .map(|node| node.node_type_id.as_str())
                .collect::<Vec<_>>(),
            [BILIBILI_NOTIFICATION_NODE_TYPE, BILIBILI_CARD_NODE_TYPE]
        );
        assert_eq!(
            manifest.provides.runners[0]
                .accepted_protocol_ids
                .iter()
                .map(|protocol_id| protocol_id.as_str())
                .collect::<Vec<_>>(),
            vec![POLL_LIVE, POLL_VIDEO, NOTIFY_CARD]
        );
    }

    #[test]
    fn poll_protocol_is_the_kind_discriminator() {
        assert_eq!(
            BilibiliPollKind::from_protocol_id(POLL_DYNAMIC),
            Some(BilibiliPollKind::Dynamic)
        );
        assert_eq!(BilibiliPollKind::from_protocol_id(LINK_RESOLVE), None);
    }

    struct CountingTransport {
        polls: Arc<AtomicU64>,
    }

    impl BilibiliTransport for CountingTransport {
        fn poll(
            &mut self,
            _kind: &BilibiliPollKind,
            uid: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            let sequence = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(vec![BilibiliItem {
                id: format!("item-{sequence}"),
                title: format!("item {sequence}"),
                url: format!("https://t.bilibili.com/{uid}/{sequence}"),
                image_url: None,
            }])
        }

        fn resolve(&mut self, _url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            unreachable!()
        }

        fn download(&mut self, _url: &str, _max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }

        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            unreachable!()
        }

        fn qr_poll(&mut self, _key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            unreachable!()
        }

        fn profile(&mut self, _uid: u64) -> Result<BilibiliProfile, BilibiliError> {
            unreachable!()
        }
    }

    fn push_flow_document(wired: bool) -> BotFlowDocument {
        let mut flow = BotFlowDocument {
            flow_id: "push".into(),
            name: "push".into(),
            nodes: vec![
                BotFlowNode {
                    node_id: "push".into(),
                    node_type_id: BILIBILI_NOTIFICATION_NODE_TYPE.into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: Some(BotFlowSourceSelector {
                        protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                        event_type: Some(BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1)),
                    }),
                    position: BotFlowNodePosition::default(),
                },
                BotFlowNode {
                    node_id: "card".into(),
                    node_type_id: BILIBILI_CARD_NODE_TYPE.into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: None,
                    position: BotFlowNodePosition::default(),
                },
            ],
            edges: Vec::new(),
        };
        if wired {
            flow.edges.push(BotFlowEdge {
                edge_id: "push-card".into(),
                from_node_id: "push".into(),
                from_port_id: "event".into(),
                to_node_id: "card".into(),
                to_port_id: "event".into(),
                kind: BotFlowEdgeKind::Event,
            });
        }
        flow
    }

    fn push_registry(wired: bool) -> Arc<BotFlowRegistry> {
        use mutsuki_bot_flow::BotNodeCatalog;
        use mutsuki_bot_protocol::BotFlowSnapshot;

        let manifest = manifest_for_backend(BilibiliBackendKind::WebCookie, false, false);
        Arc::new(
            BotFlowRegistry::with_snapshot(
                BotNodeCatalog::from_manifests(&[manifest]).unwrap(),
                BotFlowSnapshot {
                    revision: 1,
                    flow: push_flow_document(wired),
                },
            )
            .unwrap(),
        )
    }

    fn run_poll_task(runner: &mut Box<dyn Runner>, task_id: &str) -> RunnerResult {
        let task = Task::new(
            task_id,
            POLL_LIVE,
            serde_json::to_value(PollRequest {
                subscription_id: "sub".into(),
                uid: 7,
                target: BotTarget::Group {
                    group_id: "group".into(),
                },
                outbound_binding: "qq-main".into(),
            })
            .unwrap(),
        );
        let context =
            RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
        let batch = command_batch(vec![task]);
        let completed = runner.run_batch(context, batch).unwrap();
        completed.results[0].result.as_ref().unwrap().clone()
    }

    fn count_runner(
        polls: Arc<AtomicU64>,
        registry: Option<Arc<BotFlowRegistry>>,
    ) -> Box<dyn Runner> {
        let mut runner = BilibiliRunner::new(
            Box::new(CountingTransport {
                polls: polls.clone(),
            }),
            Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
            Arc::new(UnusedResources),
            "memory",
        );
        if let Some(registry) = registry {
            runner = runner.with_flow_registry(registry);
        }
        runner.into_runtime_runner(Arc::new(CompletedChildClient), None)
    }

    #[test]
    fn unwired_push_chain_skips_polling_and_reports_the_freeze() {
        let polls = Arc::new(AtomicU64::new(0));
        let mut runner = count_runner(polls.clone(), Some(push_registry(false)));

        let result = run_poll_task(&mut runner, "frozen-poll");
        assert_eq!(
            polls.load(Ordering::SeqCst),
            0,
            "unwired chain must not poll"
        );
        assert_eq!(result.output.as_ref().unwrap()["poll_skipped"], json!(true));
        assert_eq!(result.output.as_ref().unwrap()["push_wired"], json!(false));
    }

    #[test]
    fn wired_push_chain_polls_and_fans_out_notifications() {
        let polls = Arc::new(AtomicU64::new(0));
        let mut runner = count_runner(polls.clone(), Some(push_registry(true)));

        let first = run_poll_task(&mut runner, "first-poll");
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        assert!(first.tasks.is_empty(), "first poll baselines the cursor");

        let second = run_poll_task(&mut runner, "second-poll");
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        assert_eq!(second.tasks.len(), 1);
        assert_eq!(second.tasks[0].protocol_id, BOT_FLOW_INGRESS_PROTOCOL_ID);
    }

    #[test]
    fn forced_baseline_poll_advances_the_cursor_without_notifications() {
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        repository.set_cursor("Live:7:sub", "item-8").unwrap();
        let mut runner = BilibiliRunner::new(
            Box::new(CountingTransport {
                polls: Arc::new(AtomicU64::new(0)),
            }),
            repository.clone(),
            Arc::new(UnusedResources),
            "memory",
        );
        let request = PollRequest {
            subscription_id: "sub".into(),
            uid: 7,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
        };
        let task = Task::new(
            "baseline",
            POLL_LIVE,
            serde_json::to_value(&request).unwrap(),
        );
        let items = vec![BilibiliItem {
            id: "item-9".into(),
            title: "newest".into(),
            url: "https://t.bilibili.com/9".into(),
            image_url: None,
        }];

        let result = runner
            .finish_poll(&task, request, BilibiliPollKind::Live, items, None, true)
            .unwrap();
        assert!(
            result.tasks.is_empty(),
            "the frozen window must not replay as a notification backlog"
        );
        assert_eq!(
            repository.cursor("Live:7:sub").unwrap().as_deref(),
            Some("item-9")
        );
    }

    fn managed_status_service(state: Arc<Mutex<FakeTransportState>>) -> BilibiliManagementService {
        BilibiliManagementService::new(
            SharedBilibiliConfig::new(managed_config()),
            SharedBilibiliCredential::default(),
            Box::new(FakeTransport(state)),
            Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
            Arc::new(RecordingCredentialStore::default()),
            Arc::new(RecordingConfigStore::default()),
            Arc::new(AlwaysPresentSecrets),
        )
    }

    #[test]
    fn management_status_reports_push_wiring_only_with_a_shared_registry() {
        let status =
            managed_status_service(Arc::new(Mutex::new(FakeTransportState::default()))).status();
        assert_eq!(status.push_wired, None);

        let status = managed_status_service(Arc::new(Mutex::new(FakeTransportState::default())))
            .with_flow_registry(push_registry(false))
            .status();
        assert_eq!(status.push_wired, Some(false));

        let status = managed_status_service(Arc::new(Mutex::new(FakeTransportState::default())))
            .with_flow_registry(push_registry(true))
            .status();
        assert_eq!(status.push_wired, Some(true));
    }
}
