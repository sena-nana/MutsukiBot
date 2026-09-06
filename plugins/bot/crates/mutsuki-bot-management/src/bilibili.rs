//! Public Bilibili console management contract.
//!
//! Web extensions depend on this API and its DTOs. The owner plugin implements
//! the trait; product assembly injects that implementation.

use async_trait::async_trait;
use mutsuki_bot_protocol::BotTarget;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliManagementChangeArea {
    Status,
    Login,
    Subscriptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilibiliManagementChangeEvent {
    pub revision: u64,
    pub areas: Vec<BilibiliManagementChangeArea>,
}

pub struct BilibiliManagementChangeSubscription {
    pub(crate) receiver: broadcast::Receiver<BilibiliManagementChangeEvent>,
}

impl BilibiliManagementChangeSubscription {
    pub fn new(receiver: broadcast::Receiver<BilibiliManagementChangeEvent>) -> Self {
        Self { receiver }
    }

    pub async fn changed(&mut self) -> Option<BilibiliManagementChangeEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliNotificationKind {
    Live,
    Dynamic,
    Video,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliQrLoginStatus {
    Pending,
    Scanned,
    Expired,
    Confirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliCredentialSecretState {
    Absent,
    Present,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliManagementStatus {
    pub backend: String,
    pub management_enabled: bool,
    pub allow_self_binding: bool,
    pub cookie_secret_key: Option<String>,
    pub cookie_secret_state: Option<BilibiliCredentialSecretState>,
    pub credential_loaded: bool,
    pub subscription_count: usize,
    pub reason: Option<String>,
    /// Whether the push Source chain is wired into the active Bot Flow graph;
    /// `None` when the assembly did not share a Flow registry.
    #[serde(default)]
    pub push_wired: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliLoginStartResult {
    pub url: String,
    pub key: String,
    #[serde(skip)]
    pub qr_png: Vec<u8>,
    pub qr_png_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliLoginSession {
    pub url: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliLoginPollResult {
    pub status: BilibiliQrLoginStatus,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliSubscriptionView {
    pub subscription_id: String,
    pub uid: u64,
    pub notifications: Vec<BilibiliNotificationKind>,
    pub target: BotTarget,
    pub outbound_binding: String,
    pub paused: bool,
    pub owner_user_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliPreviewCardView {
    pub title: String,
    pub url: String,
    pub description: String,
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BilibiliBindChallengeResult {
    pub uid: u64,
    pub name: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum BilibiliBindVerifyResult {
    Verified(BilibiliSubscriptionView),
    SignatureMismatch { code: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BilibiliManagementError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for BilibiliManagementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BilibiliManagementError {}

/// Owner-facing Bilibili management operations used by the console WebExtension.
#[async_trait]
pub trait BilibiliManagementApi: Send + Sync {
    fn subscribe_changes(&self) -> Option<BilibiliManagementChangeSubscription> {
        None
    }

    fn status(&self) -> BilibiliManagementStatus;

    fn login_start_session(
        &self,
        actor_id: &str,
    ) -> Result<BilibiliLoginSession, BilibiliManagementError>;

    async fn login_start(
        &self,
        actor_id: &str,
    ) -> Result<BilibiliLoginStartResult, BilibiliManagementError>;

    fn login_poll(
        &self,
        actor_id: &str,
    ) -> Result<BilibiliLoginPollResult, BilibiliManagementError>;

    fn credential_clear(&self) -> Result<(), BilibiliManagementError>;

    fn list(
        &self,
        actor_id: &str,
        is_admin: bool,
    ) -> Result<Vec<BilibiliSubscriptionView>, BilibiliManagementError>;

    fn subscribe(
        &self,
        subscription_id: String,
        uid: u64,
        notifications: Vec<BilibiliNotificationKind>,
        target: BotTarget,
        outbound_binding: String,
    ) -> Result<BilibiliSubscriptionView, BilibiliManagementError>;

    fn unsubscribe(&self, subscription_id: &str) -> Result<(), BilibiliManagementError>;

    fn set_paused(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
        paused: bool,
    ) -> Result<BilibiliSubscriptionView, BilibiliManagementError>;

    fn preview(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
    ) -> Result<BilibiliPreviewCardView, BilibiliManagementError>;

    fn bind_start(
        &self,
        operator_user_id: &str,
        uid: u64,
        challenge_seed: &str,
    ) -> Result<BilibiliBindChallengeResult, BilibiliManagementError>;

    fn bind_verify(
        &self,
        operator_user_id: &str,
        platform: &str,
        target: BotTarget,
    ) -> Result<BilibiliBindVerifyResult, BilibiliManagementError>;

    fn unbind(&self, operator_user_id: &str) -> Result<bool, BilibiliManagementError>;
}
