//! Subscription, quota, and licensing module for Vocrit AI.
//!
//! Free tier: 5 000 characters / calendar month.
//! Premium tier: unlimited, 10 000 FCFA / year, validated via Money Fusion.
//! AdminFree tier: unlimited, never expires, granted by master key or admin email.
//!
//! State lives in its own Tauri store (`subscription_store.json`) so it never
//! collides with app settings and survives settings resets.

use chrono::{Datelike, Duration, Utc};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use crate::portable;

pub const SUBSCRIPTION_STORE_PATH: &str = "subscription_store.json";
pub const FREE_TIER_MONTHLY_CHAR_LIMIT: u64 = 5_000;
pub const SUBSCRIPTION_PRICE_XOF: u64 = 10_000;
pub const SUBSCRIPTION_CURRENCY: &str = "XOF";
pub const SUBSCRIPTION_DURATION_DAYS: i64 = 365;

/// Master admin key — bearer gets AdminFree tier forever.
/// Set at build time with the `VOCRIT_ADMIN_MASTER_KEY` env var. When the
/// variable is not set, the offline admin-key path is DISABLED: shipping a
/// hardcoded default key would let anyone who reads the public repo (or runs
/// `strings` on the binary) unlock the app forever.
pub const ADMIN_MASTER_KEY: Option<&str> = option_env!("VOCRIT_ADMIN_MASTER_KEY");

/// Admin emails that auto-upgrade to AdminFree when activated.
/// Override with `VOCRIT_ADMIN_EMAILS` (comma separated) at build time.
const ADMIN_EMAILS_ENV: Option<&str> = option_env!("VOCRIT_ADMIN_EMAILS");

fn admin_emails() -> Vec<String> {
    ADMIN_EMAILS_ENV
        .unwrap_or("")
        .split(',')
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionTier {
    Free,
    Premium,
    AdminFree,
}

impl Default for SubscriptionTier {
    fn default() -> Self {
        Self::Free
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Type)]
pub struct SubscriptionState {
    pub tier: SubscriptionTier,
    pub email: Option<String>,
    pub license_key: Option<String>,
    /// ISO-8601 timestamp (UTC) when the premium license expires.
    pub expires_at: Option<String>,
    /// Characters transcribed in the current quota window.
    pub monthly_chars_used: u64,
    /// ISO-8601 timestamp of the start of the current quota window.
    pub quota_window_started_at: Option<String>,
    /// Stable device identifier used for license binding.
    pub device_id: Option<String>,
}

/// Snapshot returned to the frontend for rendering the paywall / quota bar.
#[derive(Serialize, Debug, Clone, Type)]
pub struct SubscriptionStatus {
    pub tier: SubscriptionTier,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub monthly_chars_used: u64,
    pub monthly_char_limit: u64,
    pub remaining_chars: u64,
    pub is_unlimited: bool,
    pub quota_window_started_at: Option<String>,
    pub price_xof: u64,
    pub price_currency: &'static str,
}

fn store_path() -> std::path::PathBuf {
    portable::store_path(SUBSCRIPTION_STORE_PATH)
}

pub fn load_state(app: &AppHandle) -> SubscriptionState {
    let store = match app.store(store_path()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open subscription store: {e}");
            return SubscriptionState::default();
        }
    };
    match store.get("subscription") {
        Some(v) => serde_json::from_value(v).unwrap_or_else(|e| {
            warn!("Failed to parse subscription state, resetting: {e}");
            SubscriptionState::default()
        }),
        None => SubscriptionState::default(),
    }
}

fn save_state(app: &AppHandle, state: &SubscriptionState) {
    let store = match app.store(store_path()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open subscription store for write: {e}");
            return;
        }
    };
    match serde_json::to_value(state) {
        Ok(v) => {
            store.set("subscription", v);
        }
        Err(e) => error!("Failed to serialize subscription state: {e}"),
    }
}

/// Roll over the monthly quota window if we're past its end.
/// A quota window lasts one calendar month from its start.
fn maybe_reset_quota(state: &mut SubscriptionState) -> bool {
    let now = Utc::now();
    let current_month = (now.year(), now.month());

    let needs_reset = match &state.quota_window_started_at {
        None => true,
        Some(iso) => match chrono::DateTime::parse_from_rfc3339(iso) {
            Ok(dt) => {
                let started_month = (dt.year(), dt.month());
                started_month != current_month
            }
            Err(_) => true,
        },
    };

    if needs_reset {
        state.monthly_chars_used = 0;
        state.quota_window_started_at = Some(now.to_rfc3339());
        true
    } else {
        false
    }
}

fn is_expired(state: &SubscriptionState) -> bool {
    let Some(iso) = &state.expires_at else {
        return false;
    };
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(dt) => dt < Utc::now(),
        Err(_) => true,
    }
}

fn effective_tier(state: &SubscriptionState) -> SubscriptionTier {
    match state.tier {
        SubscriptionTier::AdminFree => SubscriptionTier::AdminFree,
        SubscriptionTier::Premium if !is_expired(state) => SubscriptionTier::Premium,
        SubscriptionTier::Premium => SubscriptionTier::Free,
        SubscriptionTier::Free => SubscriptionTier::Free,
    }
}

fn to_status(state: &SubscriptionState) -> SubscriptionStatus {
    let tier = effective_tier(state);
    let is_unlimited = matches!(
        tier,
        SubscriptionTier::Premium | SubscriptionTier::AdminFree
    );
    let remaining = if is_unlimited {
        u64::MAX
    } else {
        FREE_TIER_MONTHLY_CHAR_LIMIT.saturating_sub(state.monthly_chars_used)
    };

    SubscriptionStatus {
        tier,
        email: state.email.clone(),
        expires_at: state.expires_at.clone(),
        monthly_chars_used: state.monthly_chars_used,
        monthly_char_limit: FREE_TIER_MONTHLY_CHAR_LIMIT,
        remaining_chars: remaining,
        is_unlimited,
        quota_window_started_at: state.quota_window_started_at.clone(),
        price_xof: SUBSCRIPTION_PRICE_XOF,
        price_currency: SUBSCRIPTION_CURRENCY,
    }
}

/// Called by the transcription pipeline BEFORE paste/type. If the call returns
/// `Err`, the caller MUST abort the paste and show the paywall instead.
pub fn try_consume_chars(app: &AppHandle, char_count: u64) -> Result<SubscriptionStatus, String> {
    let mut state = load_state(app);
    let reset = maybe_reset_quota(&mut state);

    let tier = effective_tier(&state);
    if matches!(
        tier,
        SubscriptionTier::Premium | SubscriptionTier::AdminFree
    ) {
        if reset {
            save_state(app, &state);
        }
        return Ok(to_status(&state));
    }

    let projected = state.monthly_chars_used.saturating_add(char_count);
    if projected > FREE_TIER_MONTHLY_CHAR_LIMIT {
        if reset {
            save_state(app, &state);
        }
        return Err(format!(
            "Free quota exceeded ({} / {} chars this month). Please subscribe to continue.",
            state.monthly_chars_used, FREE_TIER_MONTHLY_CHAR_LIMIT
        ));
    }

    state.monthly_chars_used = projected;
    save_state(app, &state);
    Ok(to_status(&state))
}

fn generate_device_id() -> String {
    use sha2::{Digest, Sha256};
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let seed = format!("vocrit-ai:{host}:{user}");
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn ensure_device_id(state: &mut SubscriptionState) {
    if state.device_id.is_none() {
        state.device_id = Some(generate_device_id());
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

#[tauri::command]
#[specta::specta]
pub fn get_subscription_status(app: AppHandle) -> SubscriptionStatus {
    let mut state = load_state(&app);
    let mut dirty = maybe_reset_quota(&mut state);
    if state.device_id.is_none() {
        ensure_device_id(&mut state);
        dirty = true;
    }
    if dirty {
        save_state(&app, &state);
    }
    to_status(&state)
}

#[tauri::command]
#[specta::specta]
pub fn record_transcription_usage(
    app: AppHandle,
    char_count: u64,
) -> Result<SubscriptionStatus, String> {
    try_consume_chars(&app, char_count)
}

fn backend_base_url() -> String {
    option_env!("VOCRIT_LICENSE_BACKEND_URL")
        .unwrap_or("https://api.vocrit.ai")
        .trim_end_matches('/')
        .to_string()
}

/// Activate a license key.
///
/// Admin paths (checked locally, work offline):
/// - the key matches the build-time `VOCRIT_ADMIN_MASTER_KEY` (if configured)
/// - the email is in the build-time `VOCRIT_ADMIN_EMAILS` list
///
/// Every other key is verified against the licensing backend
/// (`/api/license/verify`) before any tier upgrade is stored. A key is never
/// trusted just because the user typed it.
#[tauri::command]
#[specta::specta]
pub async fn activate_license(
    app: AppHandle,
    email: String,
    license_key: String,
) -> Result<SubscriptionStatus, String> {
    let email_norm = email.trim().to_lowercase();
    if email_norm.is_empty() {
        return Err("Email is required".into());
    }
    let key_norm = license_key.trim().to_string();
    if key_norm.is_empty() {
        return Err("License key is required".into());
    }

    let mut state = load_state(&app);
    ensure_device_id(&mut state);
    let device_id = state.device_id.clone().unwrap_or_default();

    let is_admin_key = ADMIN_MASTER_KEY.is_some_and(|k| !k.is_empty() && key_norm == k);
    let is_admin_email = admin_emails().contains(&email_norm);

    if is_admin_key || is_admin_email {
        state.email = Some(email_norm.clone());
        state.license_key = Some(key_norm);
        state.tier = SubscriptionTier::AdminFree;
        state.expires_at = None;
        save_state(&app, &state);
        info!("Activated AdminFree tier for {}", email_norm);
        return Ok(to_status(&state));
    }

    // Verify the key with the licensing backend before trusting it.
    let url = format!("{}/api/license/verify", backend_base_url());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "key": key_norm,
            "email": email_norm,
            "deviceId": device_id,
        }))
        .send()
        .await
        .map_err(|e| format!("Could not reach licensing backend to verify the key: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("License verification failed ({})", resp.status()));
    }

    #[derive(Deserialize)]
    struct VerifyResp {
        valid: bool,
        tier: Option<String>,
        #[serde(rename = "expiresAt")]
        expires_at: Option<String>,
        reason: Option<String>,
    }
    let parsed: VerifyResp = resp
        .json()
        .await
        .map_err(|e| format!("Bad verification response: {e}"))?;

    if !parsed.valid {
        return Err(format!(
            "License key rejected: {}",
            parsed.reason.unwrap_or_else(|| "invalid key".into())
        ));
    }

    state.email = Some(email_norm.clone());
    state.license_key = Some(key_norm);
    if parsed.tier.as_deref() == Some("admin_free") {
        state.tier = SubscriptionTier::AdminFree;
        state.expires_at = None;
    } else {
        state.tier = SubscriptionTier::Premium;
        state.expires_at = parsed.expires_at.or_else(|| {
            Some((Utc::now() + Duration::days(SUBSCRIPTION_DURATION_DAYS)).to_rfc3339())
        });
    }

    save_state(&app, &state);
    info!(
        "Activated {:?} tier for {} (verified by backend)",
        state.tier, email_norm
    );
    Ok(to_status(&state))
}

/// Sign the user out of their subscription, returning to Free tier but keeping
/// the current quota usage (prevents trivial reset abuse).
#[tauri::command]
#[specta::specta]
pub fn deactivate_license(app: AppHandle) -> SubscriptionStatus {
    let mut state = load_state(&app);
    state.tier = SubscriptionTier::Free;
    state.license_key = None;
    state.expires_at = None;
    save_state(&app, &state);
    to_status(&state)
}

/// Returned to the frontend after initiating payment, so it can open the
/// Money Fusion checkout URL in the user's browser.
#[derive(Serialize, Debug, Clone, Type)]
pub struct PaymentSession {
    pub session_id: String,
    pub checkout_url: String,
    pub amount_xof: u64,
    pub currency: &'static str,
    pub provider: &'static str,
}

/// Initiate a Money Fusion checkout via the Vocrit AI licensing backend.
/// The actual Money Fusion call happens server-side so the merchant secret
/// never touches the desktop client.
///
/// The backend base URL can be overridden at build time with
/// `VOCRIT_LICENSE_BACKEND_URL`; defaults to a placeholder that the project
/// owner must configure before shipping.
#[tauri::command]
#[specta::specta]
pub async fn initiate_subscription_payment(
    app: AppHandle,
    email: String,
    phone: String,
) -> Result<PaymentSession, String> {
    let email_norm = email.trim().to_lowercase();
    let phone_norm = phone.trim().to_string();
    if email_norm.is_empty() {
        return Err("Email is required".into());
    }
    if phone_norm.is_empty() {
        return Err("Phone number is required".into());
    }

    let mut state = load_state(&app);
    ensure_device_id(&mut state);
    save_state(&app, &state);
    let device_id = state.device_id.clone().unwrap_or_default();

    let url = format!("{}/api/checkout", backend_base_url());

    let body = serde_json::json!({
        "email": email_norm,
        "phone": phone_norm,
        "device_id": device_id,
        "provider": "money_fusion",
        "plan": "annual",
        "amount_xof": SUBSCRIPTION_PRICE_XOF,
        "currency": SUBSCRIPTION_CURRENCY,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Could not reach licensing backend: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Checkout failed ({status}): {text}"));
    }

    #[derive(Deserialize)]
    struct CheckoutResp {
        session_id: String,
        checkout_url: String,
    }
    let parsed: CheckoutResp = resp
        .json()
        .await
        .map_err(|e| format!("Bad checkout response: {e}"))?;

    Ok(PaymentSession {
        session_id: parsed.session_id,
        checkout_url: parsed.checkout_url,
        amount_xof: SUBSCRIPTION_PRICE_XOF,
        currency: SUBSCRIPTION_CURRENCY,
        provider: "money_fusion",
    })
}

/// Poll the backend to learn if a checkout session has been paid. If it has,
/// the backend returns a license_key which we activate locally.
#[tauri::command]
#[specta::specta]
pub async fn check_payment_and_activate(
    app: AppHandle,
    session_id: String,
) -> Result<SubscriptionStatus, String> {
    // Session ids are nanoid-style tokens; refuse anything else so the value
    // can't be used to rewrite the request path.
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("Invalid session id".into());
    }

    let state = load_state(&app);
    let device_id = state.device_id.clone().unwrap_or_default();

    let url = format!(
        "{}/api/checkout/{session_id}?device_id={device_id}",
        backend_base_url()
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Could not reach licensing backend: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Payment status check failed ({})", resp.status()));
    }

    #[derive(Deserialize)]
    struct StatusResp {
        status: String,
        email: Option<String>,
        license_key: Option<String>,
    }
    let parsed: StatusResp = resp
        .json()
        .await
        .map_err(|e| format!("Bad status response: {e}"))?;

    if parsed.status != "paid" {
        return Err(format!(
            "Payment not completed yet (status: {})",
            parsed.status
        ));
    }

    let email = parsed.email.ok_or("Backend omitted email")?;
    let key = parsed.license_key.ok_or("Backend omitted license_key")?;
    activate_license(app, email, key).await
}
