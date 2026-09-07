//! `billing.*` business logic: the plan/quota vocabulary
//! ([`quota_exceeded`]), the outbound Stripe Checkout client
//! ([`BillingClient`], with [`StripeClient`] the real implementation and
//! [`FakeBillingClient`] the test double named in the PRD's technical
//! considerations), inbound webhook verification and processing
//! ([`verify_stripe_signature`], [`process_webhook`]), and the
//! `billing.plans`/`billing.status`/`billing.checkout` tool handlers.
//! `handler.rs` is the only place that dispatches to these from a wire
//! call; `admin.rs` owns `admin.billing_ledger`/`admin.plan_set` (kept
//! there, not here, so every `admin.*` handler lives in one file the way
//! every `host.*` one lives in `control.rs`) but reuses this module's
//! ledger vocabulary.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::db::{BillingEventInsert, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

/// One paragraph telling an agent what to do with a checkout URL (AC5) --
/// shared by every successful `billing.checkout` response rather than
/// inlined at each call site.
const CHECKOUT_INSTRUCTIONS: &str = "Give this URL to the human who authorized you; they \
    complete payment there (it expires in about 24 hours). Once they're done, call \
    billing.status to confirm the upgrade -- the webhook that flips your plan usually lands \
    within a few seconds of payment.";

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// SHA-256 of raw bytes, hex-encoded -- `billing_events.payload_sha256`
/// (migration 0006): a durable fingerprint of exactly what was received,
/// without storing the payload itself.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// A from-scratch HMAC-SHA256 (technical considerations: "Signature
/// verification is a 30-line HMAC") -- this crate has no `hmac` crate
/// dependency, the same avoid-a-dependency-for-something-small rationale
/// as `state::rfc3339_from_unix`'s hand-rolled calendar math. Standard
/// construction: `H((key' xor opad) || H((key' xor ipad) || message))`,
/// `key'` the key zero-padded (or, if longer than the block size,
/// pre-hashed then zero-padded) to SHA-256's 64-byte block size.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The wire error returned for any signature/parsing failure on
/// `POST /billing/webhook` (AC7: the response is 400, no row is written).
fn invalid_webhook(message: impl Into<String>) -> AppError {
    AppError::Structured {
        code: "invalid_webhook_signature",
        message: message.into(),
        data: json!({}),
    }
}

/// Parses Stripe's `Stripe-Signature` header (`t=<unix>,v1=<hex>[,v1=<hex>...]`),
/// recomputes HMAC-SHA256 over `"{t}.{payload}"` with `secret`, and checks
/// it against every `v1` value present (Stripe rotates signing secrets by
/// sending multiple) plus the `tolerance_secs` staleness window (AC7: a
/// timestamp older than 300s is rejected same as a bad signature).
pub fn verify_stripe_signature(
    payload: &[u8],
    header: &str,
    secret: &str,
    tolerance_secs: i64,
    now_unix: i64,
) -> Result<(), AppError> {
    let mut timestamp: Option<i64> = None;
    let mut signatures: Vec<String> = Vec::new();
    for part in header.split(',') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        match k {
            "t" => timestamp = v.parse::<i64>().ok(),
            "v1" => signatures.push(v.to_string()),
            _ => {}
        }
    }
    let Some(t) = timestamp else {
        return Err(invalid_webhook("Stripe-Signature missing t="));
    };
    if signatures.is_empty() {
        return Err(invalid_webhook("Stripe-Signature missing v1="));
    }
    if (now_unix - t).abs() > tolerance_secs {
        return Err(invalid_webhook(format!(
            "webhook timestamp {t} is outside the {tolerance_secs}s tolerance"
        )));
    }

    let signed_payload = [t.to_string().as_bytes(), b".", payload].concat();
    let expected = to_hex(&hmac_sha256(secret.as_bytes(), &signed_payload));
    let matched = signatures
        .iter()
        .any(|sig| constant_time_eq(sig.as_bytes(), expected.as_bytes()));
    if !matched {
        return Err(invalid_webhook("Stripe-Signature does not match"));
    }
    Ok(())
}

// ---- billing mode -------------------------------------------------------

/// `off` (no secret key configured), `test` (`sk_test_...`) or `live`
/// (anything else -- Stripe's own convention, so a key that somehow
/// carries neither documented prefix is treated as live, the safer
/// default) -- AC4 / AC11 / `/healthz`'s `billing_mode`.
pub fn key_mode(secret_key: Option<&str>) -> &'static str {
    match secret_key {
        None => "off",
        Some(k) if k.starts_with("sk_test_") => "test",
        Some(_) => "live",
    }
}

/// `test`/`live` from a Stripe event's own `livemode` boolean -- distinct
/// from [`key_mode`] (the host's configured key) so a mismatch between the
/// two (AC10) is a comparison between two independently-derived values,
/// not the same function called twice.
pub fn event_mode(livemode: bool) -> &'static str {
    if livemode { "live" } else { "test" }
}

// ---- configuration --------------------------------------------------------

/// The four `MCPHOST_STRIPE_*` / `MCPHOST_PLANS_PATH` settings (requirement
/// "Configuration"), read once at startup. `None` fields are the
/// documented "billing absent" state (goal 4): quotas still enforce,
/// `billing.checkout` says `billing_unavailable`, `/healthz` says `off`.
/// PRD-mcphost-metered-overage: the meter event name every meter event this
/// crate emits carries, unless `$MCPHOST_STRIPE_METER_EVENT_NAME` overrides
/// it -- matches the live Stripe account's `mcphost_tool_calls` meter.
pub const DEFAULT_METER_EVENT_NAME: &str = "mcphost_tool_calls";

#[derive(Clone, Default)]
pub struct BillingConfig {
    pub secret_key: Option<String>,
    pub webhook_secret: Option<String>,
    pub price_pro: Option<String>,
    /// PRD-mcphost-metered-overage requirement 48: the graduated metered
    /// price (`price_1UCZQRIPqmv0aTON9zg3eZ38` in production) `billing.checkout`
    /// attaches alongside the base price, and whose presence is also what
    /// gates `/healthz`'s `meter_lag` field. `None` (absent
    /// `$MCPHOST_STRIPE_METERED_PRICE_ID`) reproduces v0.14.0 behavior
    /// exactly: a single-price checkout, no `meter_lag`.
    pub metered_price_id: Option<String>,
    /// `$MCPHOST_STRIPE_METER_EVENT_NAME`; `None` (the common case) means
    /// "use [`DEFAULT_METER_EVENT_NAME`]" -- see [`Self::meter_event_name`].
    pub meter_event_name: Option<String>,
}

impl BillingConfig {
    pub fn from_env() -> Self {
        Self {
            secret_key: std::env::var("MCPHOST_STRIPE_SECRET_KEY").ok(),
            webhook_secret: std::env::var("MCPHOST_STRIPE_WEBHOOK_SECRET").ok(),
            price_pro: std::env::var("MCPHOST_STRIPE_PRICE_PRO").ok(),
            metered_price_id: std::env::var("MCPHOST_STRIPE_METERED_PRICE_ID").ok(),
            meter_event_name: std::env::var("MCPHOST_STRIPE_METER_EVENT_NAME").ok(),
        }
    }

    pub fn billing_mode(&self) -> &'static str {
        key_mode(self.secret_key.as_deref())
    }

    /// The Stripe price id for `plan_name`. Only `pro` is wired to a price
    /// at P0 (`MCPHOST_STRIPE_PRICE_PRO`) -- P2 adds `team`
    /// (`MCPHOST_STRIPE_PRICE_TEAM`), out of scope here.
    pub fn price_for(&self, plan_name: &str) -> Option<&str> {
        match plan_name {
            "pro" => self.price_pro.as_deref(),
            _ => None,
        }
    }

    /// The event name `mcphost billing emit-meter` reports batches under:
    /// the configured override, or [`DEFAULT_METER_EVENT_NAME`].
    pub fn meter_event_name(&self) -> &str {
        self.meter_event_name
            .as_deref()
            .unwrap_or(DEFAULT_METER_EVENT_NAME)
    }
}

// ---- outbound: Stripe Checkout -------------------------------------------

/// What `billing.checkout` asks the processor to create.
pub struct CheckoutSessionRequest {
    pub price_id: String,
    /// PRD-mcphost-metered-overage AC3: `Some` (the base price plus this
    /// metered price, no quantity on the second line item) whenever
    /// `BillingConfig::metered_price_id` is configured; `None` reproduces
    /// v0.14.0's single-price shape exactly.
    pub metered_price_id: Option<String>,
    pub client_reference_id: String,
    pub tenant_namespace: String,
    pub success_url: String,
    pub cancel_url: String,
    /// AC3: `true` whenever billing is configured at all (Stripe Tax is
    /// active on the live account, defaults exclusive, products carry
    /// `txcd_10103001`) -- `checkout()` only builds a request once billing
    /// is confirmed configured, so this is unconditionally `true` at every
    /// call site today, but it's a field (not a hardcoded `"true"` at the
    /// HTTP layer) so [`FakeBillingClient`] can assert on it directly.
    pub automatic_tax: bool,
}

/// What the processor (real or fake) hands back: enough to build
/// `billing.checkout`'s response (AC5).
#[derive(Debug, Clone)]
pub struct CheckoutSessionResponse {
    pub id: String,
    pub url: String,
    /// Unix seconds -- Stripe's own `expires_at` shape, about 24h ahead of
    /// creation for a `subscription`-mode session.
    pub expires_at: i64,
}

/// One Stripe meter event `mcphost billing emit-meter` (`src/metering.rs`)
/// asks a [`BillingClient`] to record: one per pro tenant's pending span
/// (requirement 51's "groups per tenant").
#[derive(Debug, Clone)]
pub struct MeterEventRequest {
    pub event_name: String,
    /// `mcphost-<tenant_id>-<first_call_id>-<last_call_id>` -- Stripe-side
    /// dedup key (requirement 51); identical across a crash-and-rerun of
    /// the same unadvanced span (AC5).
    pub identifier: String,
    pub stripe_customer_id: String,
    /// The call count this event reports (`payload[value]`).
    pub value: i64,
}

/// The outbound half of billing: creating a Checkout Session, and (PRD-
/// mcphost-metered-overage) emitting Stripe meter events. A trait
/// (technical considerations: "Tests never reach the network") so
/// `AppState` can hold a [`FakeBillingClient`] in every test and a
/// [`StripeClient`] in production, with `billing.checkout`'s own logic
/// none the wiser which one it's calling.
#[async_trait::async_trait]
pub trait BillingClient: Send + Sync {
    async fn create_checkout_session(
        &self,
        req: &CheckoutSessionRequest,
    ) -> Result<CheckoutSessionResponse, AppError>;

    /// POST one meter event per entry in `events` (real Stripe's
    /// `/v1/billing/meter_events` creates exactly one event per call; the
    /// caller in `src/metering.rs` is what caps `events.len()` at 100 per
    /// invocation, per requirement 51). Any single event's rejection fails
    /// the whole call -- `src/metering.rs` treats that as "this chunk did
    /// not go through," ledgering and advancing state for nothing in it.
    async fn emit_meter_events(&self, events: &[MeterEventRequest]) -> Result<(), AppError>;

    /// PRD-mcphost-metered-overage P1 requirement 60 / AC12: Stripe's own
    /// accepted-usage total for `stripe_customer_id` on the
    /// `meter_event_name` meter, summed from `period_start` (unix seconds,
    /// the current UTC month's start) to now. This is deliberately a
    /// separate read from [`Self::emit_meter_events`] -- it reports what
    /// Stripe has *accepted and aggregated* server-side, which can lag or
    /// (after a dedup) undercount raw emitted events, and is exactly the
    /// number `billing.status` labels "Stripe-reported" rather than
    /// reusing the ledger's own `emitted_this_month` count.
    async fn accepted_usage(
        &self,
        stripe_customer_id: &str,
        meter_event_name: &str,
        period_start: i64,
    ) -> Result<i64, AppError>;
}

/// The real implementation: `POST https://api.stripe.com/v1/checkout/sessions`,
/// form-encoded, bearer-authenticated with the secret key -- the one
/// outbound endpoint P0 needs (technical considerations: "no Stripe SDK
/// crate"). Never constructed by a test; see [`FakeBillingClient`].
pub struct StripeClient {
    http: reqwest::Client,
    secret_key: String,
}

impl StripeClient {
    pub fn new(http: reqwest::Client, secret_key: String) -> Self {
        Self { http, secret_key }
    }
}

#[async_trait::async_trait]
impl BillingClient for StripeClient {
    async fn create_checkout_session(
        &self,
        req: &CheckoutSessionRequest,
    ) -> Result<CheckoutSessionResponse, AppError> {
        let mut params: Vec<(&str, &str)> = vec![
            ("mode", "subscription"),
            ("line_items[0][price]", req.price_id.as_str()),
            ("line_items[0][quantity]", "1"),
            ("client_reference_id", req.client_reference_id.as_str()),
            (
                "metadata[tenant_namespace]",
                req.tenant_namespace.as_str(),
            ),
            ("success_url", req.success_url.as_str()),
            ("cancel_url", req.cancel_url.as_str()),
        ];
        // AC3: the metered price rides alongside the base price with no
        // `quantity` field -- Stripe reports usage for it via meter events,
        // not a line-item quantity.
        if let Some(metered_price_id) = req.metered_price_id.as_deref() {
            params.push(("line_items[1][price]", metered_price_id));
        }
        // AC3: Stripe Tax is active on the account whenever billing is
        // configured at all.
        if req.automatic_tax {
            params.push(("automatic_tax[enabled]", "true"));
        }
        let resp = self
            .http
            .post("https://api.stripe.com/v1/checkout/sessions")
            .bearer_auth(&self.secret_key)
            .form(&params)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Stripe checkout session request: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Stripe checkout session response: {e}")))?;
        if !status.is_success() {
            return Err(AppError::Internal(format!(
                "Stripe rejected the checkout session request: HTTP {status}: {body}"
            )));
        }
        Ok(CheckoutSessionResponse {
            id: body
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            url: body
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            expires_at: body.get("expires_at").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// PRD-mcphost-metered-overage: one real POST per event -- Stripe's
    /// `/v1/billing/meter_events` has no bulk-create shape, so
    /// `src/metering.rs`'s "no more than 100 events per request" (AC7)
    /// caps how many of these this call issues per invocation, not a
    /// single request's payload size.
    async fn emit_meter_events(&self, events: &[MeterEventRequest]) -> Result<(), AppError> {
        for event in events {
            let value = event.value.to_string();
            let params = [
                ("event_name", event.event_name.as_str()),
                (
                    "payload[stripe_customer_id]",
                    event.stripe_customer_id.as_str(),
                ),
                ("payload[value]", value.as_str()),
                ("identifier", event.identifier.as_str()),
            ];
            let resp = self
                .http
                .post("https://api.stripe.com/v1/billing/meter_events")
                .bearer_auth(&self.secret_key)
                .form(&params)
                .send()
                .await
                .map_err(|e| AppError::Internal(format!("Stripe meter event request: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                return Err(AppError::Internal(format!(
                    "Stripe rejected the meter event: HTTP {status}: {body}"
                )));
            }
        }
        Ok(())
    }

    /// Two real reads: `GET /v1/billing/meters` to resolve `event_name` to
    /// the meter id Stripe's usage-summary endpoint actually keys on, then
    /// `GET /v1/billing/meters/{id}/event_summaries` for the customer's
    /// aggregated value over the period. Summed across whatever windows
    /// the summary endpoint returns -- `billing.status` only wants one
    /// number for the period, not Stripe's own bucketing.
    async fn accepted_usage(
        &self,
        stripe_customer_id: &str,
        meter_event_name: &str,
        period_start: i64,
    ) -> Result<i64, AppError> {
        let list_resp = self
            .http
            .get("https://api.stripe.com/v1/billing/meters")
            .bearer_auth(&self.secret_key)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Stripe meter list request: {e}")))?;
        let list_status = list_resp.status();
        let list_body: Value = list_resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Stripe meter list response: {e}")))?;
        if !list_status.is_success() {
            return Err(AppError::Internal(format!(
                "Stripe rejected the meter list request: HTTP {list_status}: {list_body}"
            )));
        }
        let meter_id = list_body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|m| m.get("event_name").and_then(Value::as_str) == Some(meter_event_name))
            .and_then(|m| m.get("id").and_then(Value::as_str))
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "no Stripe meter found with event_name '{meter_event_name}'"
                ))
            })?
            .to_string();

        let now = crate::state::now_unix().to_string();
        let start = period_start.to_string();
        let summary_resp = self
            .http
            .get(format!(
                "https://api.stripe.com/v1/billing/meters/{meter_id}/event_summaries"
            ))
            .bearer_auth(&self.secret_key)
            .query(&[
                ("customer", stripe_customer_id),
                ("start_time", start.as_str()),
                ("end_time", now.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Stripe meter summary request: {e}")))?;
        let summary_status = summary_resp.status();
        let summary_body: Value = summary_resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Stripe meter summary response: {e}")))?;
        if !summary_status.is_success() {
            return Err(AppError::Internal(format!(
                "Stripe rejected the meter summary request: HTTP {summary_status}: {summary_body}"
            )));
        }
        let total = summary_body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("aggregated_value").and_then(Value::as_i64))
            .sum();
        Ok(total)
    }
}

/// The test double every `billing_ac*.rs` test injects instead of
/// [`StripeClient`] (technical considerations: "Tests never reach the
/// network"). Records the last request it received (AC5's "the fake
/// received mode=subscription, the pro price id, and client_reference_id
/// equal to the tenant namespace") and returns a canned session with a
/// deterministic id/url and an `expires_at` 24h ahead of construction.
#[cfg(any(test, feature = "test-support"))]
pub struct FakeBillingClient {
    pub last_request: std::sync::Mutex<Option<CheckoutSessionRequestSnapshot>>,
    pub calls: std::sync::atomic::AtomicUsize,
    now_unix: i64,
    /// PRD-mcphost-metered-overage: every `emit_meter_events` invocation
    /// this fake received, in order -- one `Vec<MeterEventRequest>` per
    /// call, so a test can assert both "how many requests" (AC7's
    /// 100-per-request cap) and "what each one carried" (AC4/AC5's
    /// identifiers).
    pub meter_batches: std::sync::Mutex<Vec<Vec<MeterEventRequest>>>,
    /// AC6: when set, every `emit_meter_events` call fails as if Stripe
    /// returned 500 -- set via [`Self::set_fail_meter_events`].
    fail_meter_events: std::sync::atomic::AtomicBool,
    /// AC12: what [`BillingClient::accepted_usage`] returns -- `None`
    /// (the default) reports `0`, same as "Stripe has accepted nothing
    /// yet"; a test sets this via [`Self::set_accepted_usage`] to assert
    /// `billing.status` surfaces it labeled Stripe-reported.
    accepted_usage: std::sync::Mutex<Option<i64>>,
}

/// An owned, `Clone`/inspectable copy of a [`CheckoutSessionRequest`] (the
/// original borrows nothing but is not itself `Clone`d for; a snapshot
/// lets a test read back what the fake received after the async call has
/// already consumed the original).
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone)]
pub struct CheckoutSessionRequestSnapshot {
    pub price_id: String,
    pub metered_price_id: Option<String>,
    pub client_reference_id: String,
    pub tenant_namespace: String,
    pub success_url: String,
    pub cancel_url: String,
    pub automatic_tax: bool,
}

/// A test-only signer, sharing the exact same HMAC construction
/// [`verify_stripe_signature`] checks against -- `billing_ac*.rs`'s webhook
/// tests use this to build a `Stripe-Signature` header for a canned event
/// payload (technical considerations: "webhook test vectors signed with a
/// known secret", never a real network round trip to Stripe).
#[cfg(any(test, feature = "test-support"))]
pub fn sign_for_test(payload: &[u8], secret: &str, timestamp: i64) -> String {
    let signed_payload = [timestamp.to_string().as_bytes(), b".", payload].concat();
    let sig = to_hex(&hmac_sha256(secret.as_bytes(), &signed_payload));
    format!("t={timestamp},v1={sig}")
}

#[cfg(any(test, feature = "test-support"))]
impl FakeBillingClient {
    pub fn new(now_unix: i64) -> Self {
        Self {
            last_request: std::sync::Mutex::new(None),
            calls: std::sync::atomic::AtomicUsize::new(0),
            now_unix,
            meter_batches: std::sync::Mutex::new(Vec::new()),
            fail_meter_events: std::sync::atomic::AtomicBool::new(false),
            accepted_usage: std::sync::Mutex::new(None),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// AC6: make the next (and every subsequent) `emit_meter_events` call
    /// fail as if Stripe returned 500.
    pub fn set_fail_meter_events(&self, fail: bool) {
        self.fail_meter_events
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// How many separate `emit_meter_events` calls ("requests", AC7) this
    /// fake has received.
    pub fn meter_request_count(&self) -> usize {
        self.meter_batches.lock().map(|b| b.len()).unwrap_or(0)
    }

    /// Every event across every `emit_meter_events` call, flattened -- for
    /// assertions that don't care about request boundaries (AC4's total
    /// count, AC5's identifier comparison).
    pub fn all_meter_events(&self) -> Vec<MeterEventRequest> {
        self.meter_batches
            .lock()
            .map(|b| b.iter().flatten().cloned().collect())
            .unwrap_or_default()
    }

    /// AC12: make the next (and every subsequent) `accepted_usage` call
    /// return `value`, as if Stripe had accepted and aggregated that many
    /// events for the queried customer/meter/period.
    pub fn set_accepted_usage(&self, value: i64) {
        if let Ok(mut guard) = self.accepted_usage.lock() {
            *guard = Some(value);
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl BillingClient for FakeBillingClient {
    async fn create_checkout_session(
        &self,
        req: &CheckoutSessionRequest,
    ) -> Result<CheckoutSessionResponse, AppError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = self.last_request.lock() {
            *guard = Some(CheckoutSessionRequestSnapshot {
                price_id: req.price_id.clone(),
                metered_price_id: req.metered_price_id.clone(),
                client_reference_id: req.client_reference_id.clone(),
                tenant_namespace: req.tenant_namespace.clone(),
                success_url: req.success_url.clone(),
                cancel_url: req.cancel_url.clone(),
                automatic_tax: req.automatic_tax,
            });
        }
        let n = self.call_count();
        Ok(CheckoutSessionResponse {
            id: format!("cs_test_fake_{n}"),
            url: format!("https://checkout.stripe.com/c/pay/cs_test_fake_{n}"),
            expires_at: self.now_unix + 24 * 3600,
        })
    }

    async fn emit_meter_events(&self, events: &[MeterEventRequest]) -> Result<(), AppError> {
        if self
            .fail_meter_events
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::Internal(
                "Stripe rejected the meter event: HTTP 500: {}".to_string(),
            ));
        }
        if let Ok(mut guard) = self.meter_batches.lock() {
            guard.push(events.to_vec());
        }
        Ok(())
    }

    async fn accepted_usage(
        &self,
        _stripe_customer_id: &str,
        _meter_event_name: &str,
        _period_start: i64,
    ) -> Result<i64, AppError> {
        Ok(self
            .accepted_usage
            .lock()
            .map(|g| g.unwrap_or(0))
            .unwrap_or(0))
    }
}

// ---- quota vocabulary -----------------------------------------------------

/// The shape every quota rejection carries (requirement "Enforcement" /
/// AC2/AC3): `code: quota_exceeded`, `plan`, `limit: {name, value}`,
/// `used`, an optional `resets_at` (only meaningful for `calls_per_day`),
/// and `next: "billing.checkout"` so an agent always knows what to do
/// instead of retrying.
pub fn quota_exceeded(
    plan: &str,
    limit_name: &'static str,
    limit_value: i64,
    used: i64,
    resets_at: Option<String>,
) -> AppError {
    let mut data = json!({
        "plan": plan,
        "limit": {"name": limit_name, "value": limit_value},
        "used": used,
        "next": "billing.checkout",
    });
    if let (Some(resets_at), Some(obj)) = (resets_at, data.as_object_mut()) {
        obj.insert("resets_at".to_string(), json!(resets_at));
    }
    AppError::Structured {
        code: "quota_exceeded",
        message: format!("plan '{plan}' quota exceeded: {limit_name} (limit {limit_value})"),
        data,
    }
}

/// AC4: `billing.checkout` (and, transitively, anything that needs a live
/// Stripe config but doesn't have one) with no secret key configured.
pub fn billing_unavailable(reason: impl Into<String>) -> AppError {
    AppError::Structured {
        code: "billing_unavailable",
        message: reason.into(),
        data: json!({"billing_mode": "off"}),
    }
}

// ---- billing.* tool handlers -----------------------------------------------

/// `billing.plans` (anonymous and tenant, AC1): the catalog plus
/// `billing_mode`. Never fails -- there is always at least a default
/// catalog loaded at startup.
pub fn plans(state: &AppState) -> Value {
    let plans: Vec<Value> = state
        .plans
        .plans
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "price_usd_month": p.price_usd_month,
                "tools_max": p.tools_max,
                "calls_per_day": p.calls_per_day,
                "secrets_max": p.secrets_max,
                "description": p.description,
            })
        })
        .collect();
    json!({
        "plans": plans,
        "billing_mode": state.billing_config.billing_mode(),
    })
}

/// `billing.status` (tenant): plan, usage against each quota, and
/// `resets_at` for the daily counter. PRD-mcphost-metered-overage requirement
/// 53: a pro tenant's response also carries `metered_usage.emitted_this_month`
/// -- the ledgered call count `mcphost billing emit-meter` has already sent
/// to Stripe this UTC month, so what a pro tenant owes past the included
/// volume is inspectable by tool call, not just by invoice. Absent for every
/// other plan (nothing has ever emitted a meter event for them).
pub async fn status(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let tools_used = state.db.count_tools(tenant.id).await?;
    let secrets_used = state.db.count_secrets(tenant.id).await?;
    let now = crate::state::now_unix();
    let midnight = crate::state::utc_midnight_unix(now);
    let calls_used = state.db.count_calls_since(tenant.id, midnight, true).await?;
    let resets_at = crate::state::rfc3339_from_unix(midnight + 86_400);
    let mut result = json!({
        "plan": tenant.plan,
        "plan_since": tenant.plan_since,
        "usage": {
            "tools_max": {"used": tools_used, "limit": plan.tools_max},
            "secrets_max": {"used": secrets_used, "limit": plan.secrets_max},
            "calls_per_day": {"used": calls_used, "limit": plan.calls_per_day, "resets_at": resets_at},
        },
    });
    if tenant.plan == "pro" {
        let month_start = crate::state::utc_month_start_unix(now);
        let emitted = state
            .db
            .emitted_call_count_for_tenant(tenant.id, month_start)
            .await?;
        let mut metered_usage = json!({"emitted_this_month": emitted});
        // P1 requirement 60 / AC12: Stripe's own accepted-usage total,
        // labeled distinctly from the ledger's `emitted_this_month` --
        // only reachable with a configured key and a customer id on file
        // (nothing to query Stripe about otherwise).
        if let (Some(_), Some(stripe_customer_id)) = (
            state.billing_config.secret_key.as_deref(),
            tenant.stripe_customer_id.as_deref(),
        ) {
            let cached = state
                .accepted_usage_cache
                .lock()
                .ok()
                .and_then(|cache| cache.get(&tenant.id).copied())
                .filter(|c| now - c.fetched_at < crate::billing::ACCEPTED_USAGE_CACHE_TTL_SECS);
            let stripe_reported = match cached {
                Some(c) => c.value,
                None => {
                    let value = state
                        .billing_client
                        .accepted_usage(
                            stripe_customer_id,
                            state.billing_config.meter_event_name(),
                            month_start,
                        )
                        .await?;
                    if let Ok(mut cache) = state.accepted_usage_cache.lock() {
                        cache.insert(
                            tenant.id,
                            CachedAcceptedUsage {
                                value,
                                fetched_at: now,
                            },
                        );
                    }
                    value
                }
            };
            if let Some(obj) = metered_usage.as_object_mut() {
                obj.insert("stripe_reported".to_string(), json!(stripe_reported));
            }
        }
        if let Some(obj) = result.as_object_mut() {
            obj.insert("metered_usage".to_string(), metered_usage);
        }
    }
    Ok(result)
}

fn arg_str(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// An open, not-yet-expired Checkout Session `billing.checkout` created for
/// a `(tenant, plan)` pair, cached in [`AppState::checkout_sessions`] so a
/// second call before it expires returns the same URL instead of asking
/// the processor to mint another one (P1 AC13). Held in memory only --
/// scoped to this process's lifetime, same as every other in-flight-request
/// cache in this crate; a restart simply lets the next call create a fresh
/// session, which is a safe (if slightly wasteful) outcome, not a
/// correctness one.
#[derive(Debug, Clone)]
pub struct OpenCheckoutSession {
    pub url: String,
    pub expires_at: i64,
}

/// The type [`AppState::checkout_sessions`](crate::state::AppState::checkout_sessions)
/// holds -- named here (rather than spelled out inline) to keep
/// clippy's `type_complexity` lint happy.
pub type CheckoutSessionCache =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(i64, String), OpenCheckoutSession>>>;

/// P1 requirement 60 / AC12: how long a `billing.status` call trusts a
/// prior [`BillingClient::accepted_usage`] read for the same tenant before
/// asking Stripe again. Chosen so a burst of `billing.status` polling
/// (an agent checking its own overage) does not turn into a burst of
/// Stripe reads, while staying well inside "the current period" -- a
/// cached number is at most this many seconds stale.
pub const ACCEPTED_USAGE_CACHE_TTL_SECS: i64 = 60;

/// One cached [`BillingClient::accepted_usage`] read: the value and the
/// unix time it was fetched at.
#[derive(Debug, Clone, Copy)]
pub struct CachedAcceptedUsage {
    pub value: i64,
    pub fetched_at: i64,
}

/// The type [`AppState::accepted_usage_cache`](crate::state::AppState::accepted_usage_cache)
/// holds -- keyed by tenant id, same in-process/in-memory-only shape as
/// [`CheckoutSessionCache`].
pub type AcceptedUsageCache = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i64, CachedAcceptedUsage>>>;

/// `billing.checkout` (tenant, `plan` defaulting to `pro`, AC4/AC5): create
/// (or reuse -- P1 AC13) a Stripe Checkout Session and return its URL,
/// expiry, mode, and one paragraph of instructions.
pub async fn checkout(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let plan_name = arg_str(args, "plan").unwrap_or_else(|| "pro".to_string());
    let Some(secret_key) = state.billing_config.secret_key.as_deref() else {
        return Err(billing_unavailable(
            "billing is not configured on this host (no MCPHOST_STRIPE_SECRET_KEY)",
        ));
    };
    if state.plans.get(&plan_name).is_none() {
        return Err(AppError::InvalidParams(format!(
            "unknown plan '{plan_name}'"
        )));
    }
    let Some(price_id) = state.billing_config.price_for(&plan_name) else {
        return Err(billing_unavailable(format!(
            "no Stripe price is configured for plan '{plan_name}'"
        )));
    };
    let mode = key_mode(Some(secret_key));
    let cache_key = (tenant.id, plan_name.clone());
    let now = crate::state::now_unix();

    // P1 AC13: reuse an open, unexpired session for the same tenant and
    // plan instead of creating a second one. A poisoned mutex (a prior
    // panic while holding the lock) is treated as "no cache" rather than
    // propagated -- losing the reuse optimization is not worth failing a
    // paying tenant's checkout over.
    if let Ok(cache) = state.checkout_sessions.lock() {
        if let Some(open) = cache.get(&cache_key) {
            if open.expires_at > now {
                return Ok(json!({
                    "url": open.url,
                    "expires_at": open.expires_at,
                    "mode": mode,
                    "instructions": CHECKOUT_INSTRUCTIONS,
                }));
            }
        }
    }

    let success_url = format!("{}/billing/done", state.public_url.trim_end_matches('/'));
    let cancel_url = format!("{}/billing/cancel", state.public_url.trim_end_matches('/'));
    let req = CheckoutSessionRequest {
        price_id: price_id.to_string(),
        // AC3: rides alongside the base price whenever configured; absent
        // reproduces v0.14.0's single-price shape exactly.
        metered_price_id: state.billing_config.metered_price_id.clone(),
        client_reference_id: tenant.namespace.clone(),
        tenant_namespace: tenant.namespace.clone(),
        success_url,
        cancel_url,
        // AC3: this whole function only reaches here once billing is
        // confirmed configured (the `secret_key` check above), which is
        // exactly the condition Stripe Tax should be on for.
        automatic_tax: true,
    };
    let session = state.billing_client.create_checkout_session(&req).await?;

    if let Ok(mut cache) = state.checkout_sessions.lock() {
        cache.insert(
            cache_key,
            OpenCheckoutSession {
                url: session.url.clone(),
                expires_at: session.expires_at,
            },
        );
    }

    Ok(json!({
        "url": session.url,
        "expires_at": session.expires_at,
        "mode": mode,
        "instructions": CHECKOUT_INSTRUCTIONS,
    }))
}

// ---- inbound: webhook processing -------------------------------------------

/// A parsed Stripe event, just the fields this crate needs off of it.
struct StripeEvent {
    id: String,
    event_type: String,
    livemode: bool,
    object: Value,
}

impl StripeEvent {
    fn parse(raw: &Value) -> Result<Self, AppError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_webhook("event missing id"))?
            .to_string();
        let event_type = raw
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_webhook("event missing type"))?
            .to_string();
        let livemode = raw.get("livemode").and_then(Value::as_bool).unwrap_or(false);
        let object = raw
            .get("data")
            .and_then(|d| d.get("object"))
            .cloned()
            .unwrap_or(Value::Null);
        Ok(Self {
            id,
            event_type,
            livemode,
            object,
        })
    }
}

fn amount_and_currency(object: &Value) -> (Option<i64>, Option<String>) {
    let amount = object
        .get("amount_total")
        .or_else(|| object.get("amount_paid"))
        .and_then(Value::as_i64);
    let currency = object
        .get("currency")
        .and_then(Value::as_str)
        .map(str::to_string);
    (amount, currency)
}

/// `POST /billing/webhook`'s whole business logic: verify the signature
/// (AC7), parse the event, ledger it exactly once (idempotent on
/// `event_id`, AC6's second-POST-changes-nothing), and apply whatever plan
/// change it implies. `Ok` always means "respond 200"; `Err` (only ever
/// [`invalid_webhook`]'s `invalid_webhook_signature`) means "respond 400,
/// nothing was written" (AC7) -- `http.rs` maps the two outcomes onto
/// status codes, this function never touches one directly.
pub async fn process_webhook(
    state: &AppState,
    raw_body: &[u8],
    signature_header: &str,
) -> Result<Value, AppError> {
    let webhook_secret = state
        .billing_config
        .webhook_secret
        .as_deref()
        .ok_or_else(|| invalid_webhook("no webhook secret is configured on this host"))?;
    verify_stripe_signature(
        raw_body,
        signature_header,
        webhook_secret,
        300,
        crate::state::now_unix(),
    )?;

    let parsed: Value = serde_json::from_slice(raw_body)
        .map_err(|e| invalid_webhook(format!("invalid JSON payload: {e}")))?;
    let event = StripeEvent::parse(&parsed)?;
    let payload_sha256 = sha256_hex(raw_body);

    // AC6: idempotent on event_id -- a second delivery of the same event
    // changes nothing (and does not re-ledger it as a duplicate row).
    if state.db.billing_event_exists(event.id.clone()).await? {
        return Ok(json!({"received": true, "duplicate": true}));
    }

    let host_mode = state.billing_config.billing_mode();
    let ev_mode = event_mode(event.livemode);

    // AC10: a mismatch between the host's configured key and the event's
    // own livemode flag is ledgered under a `.mode_mismatch`-suffixed
    // event_type and NOT applied -- checked before any event-type
    // dispatch, so every handler below can assume the two already agree.
    if host_mode != "off" && host_mode != ev_mode {
        state
            .db
            .insert_billing_event(BillingEventInsert {
                event_id: event.id.clone(),
                event_type: format!("{}.mode_mismatch", event.event_type),
                tenant_id: None,
                plan: None,
                amount_cents: None,
                currency: None,
                mode: ev_mode.to_string(),
                payload_sha256,
            })
            .await?;
        return Ok(json!({"received": true, "mode_mismatch": true}));
    }

    match event.event_type.as_str() {
        "checkout.session.completed" => {
            apply_checkout_completed(state, &event, ev_mode, payload_sha256).await
        }
        "invoice.paid" => apply_invoice_paid(state, &event, ev_mode, payload_sha256).await,
        "customer.subscription.deleted" | "invoice.payment_failed" => {
            apply_downgrade(state, &event, ev_mode, payload_sha256).await
        }
        // AC7's sibling case, "checkout.session.expired", and every event
        // type this crate has no opinion about: ledgered, never applied
        // (requirement: "unknown event types are ledgered ... and
        // ignored").
        _ => {
            let (amount_cents, currency) = amount_and_currency(&event.object);
            state
                .db
                .insert_billing_event(BillingEventInsert {
                    event_id: event.id.clone(),
                    event_type: event.event_type.clone(),
                    tenant_id: None,
                    plan: None,
                    amount_cents,
                    currency,
                    mode: ev_mode.to_string(),
                    payload_sha256,
                })
                .await?;
            Ok(json!({"received": true}))
        }
    }
}

async fn apply_checkout_completed(
    state: &AppState,
    event: &StripeEvent,
    mode: &'static str,
    payload_sha256: String,
) -> Result<Value, AppError> {
    let tenant_ns = event
        .object
        .get("client_reference_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let billing_ref = event
        .object
        .get("customer")
        .and_then(Value::as_str)
        .or_else(|| event.object.get("subscription").and_then(Value::as_str))
        .map(str::to_string);
    let plan_name = "pro".to_string();

    let tenant = match tenant_ns.as_deref() {
        Some(ns) => state.db.find_tenant_by_namespace(ns.to_string()).await?,
        None => None,
    };
    let (amount_cents, currency) = amount_and_currency(&event.object);
    let plan_since = crate::state::rfc3339_now();
    // PRD-mcphost-metered-overage AC2: the event's own `customer` field,
    // specifically -- never the `subscription` fallback `billing_ref`
    // (above) accepts, since meter events need a real customer id.
    let stripe_customer_id = event
        .object
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string);

    if let Some(tenant) = &tenant {
        state
            .db
            .upgrade_tenant_plan(
                tenant.id,
                plan_name.clone(),
                plan_since,
                billing_ref.clone(),
            )
            .await?;
        if let Some(customer_id) = stripe_customer_id {
            state.db.set_stripe_customer_id(tenant.id, customer_id).await?;
        }
    }

    state
        .db
        .insert_billing_event(BillingEventInsert {
            event_id: event.id.clone(),
            event_type: event.event_type.clone(),
            tenant_id: tenant.as_ref().map(|t| t.id),
            plan: Some(plan_name),
            amount_cents,
            currency,
            mode: mode.to_string(),
            payload_sha256,
        })
        .await?;
    Ok(json!({"received": true}))
}

async fn apply_invoice_paid(
    state: &AppState,
    event: &StripeEvent,
    mode: &'static str,
    payload_sha256: String,
) -> Result<Value, AppError> {
    let customer = event
        .object
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string);
    let tenant = match customer {
        Some(c) => state.db.find_tenant_by_billing_ref(c).await?,
        None => None,
    };
    let (amount_cents, currency) = amount_and_currency(&event.object);
    if let Some(tenant) = &tenant {
        state
            .db
            .refresh_plan_since(tenant.id, crate::state::rfc3339_now())
            .await?;
    }
    state
        .db
        .insert_billing_event(BillingEventInsert {
            event_id: event.id.clone(),
            event_type: event.event_type.clone(),
            tenant_id: tenant.as_ref().map(|t| t.id),
            plan: tenant.as_ref().map(|t| t.plan.clone()),
            amount_cents,
            currency,
            mode: mode.to_string(),
            payload_sha256,
        })
        .await?;
    Ok(json!({"received": true}))
}

async fn apply_downgrade(
    state: &AppState,
    event: &StripeEvent,
    mode: &'static str,
    payload_sha256: String,
) -> Result<Value, AppError> {
    let customer = event
        .object
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string);
    let tenant = match customer {
        Some(c) => state.db.find_tenant_by_billing_ref(c).await?,
        None => None,
    };
    if let Some(tenant) = &tenant {
        state.db.downgrade_tenant_plan(tenant.id).await?;
    }
    state
        .db
        .insert_billing_event(BillingEventInsert {
            event_id: event.id.clone(),
            event_type: event.event_type.clone(),
            tenant_id: tenant.as_ref().map(|t| t.id),
            plan: Some("free".to_string()),
            amount_cents: None,
            currency: None,
            mode: mode.to_string(),
            payload_sha256,
        })
        .await?;
    Ok(json!({"received": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mode_reads_prefix() {
        assert_eq!(key_mode(None), "off");
        assert_eq!(key_mode(Some("sk_test_abc")), "test");
        assert_eq!(key_mode(Some("sk_live_abc")), "live");
    }

    #[test]
    fn event_mode_reads_livemode() {
        assert_eq!(event_mode(true), "live");
        assert_eq!(event_mode(false), "test");
    }

    #[test]
    fn signature_round_trips() {
        let secret = "whsec_test_secret"; // allowlist: test-only fixture, not a real Stripe secret
        let payload = br#"{"id":"evt_1","type":"checkout.session.completed"}"#;
        let now = 1_700_000_000i64;
        let signed_payload = [now.to_string().as_bytes(), b".", payload.as_slice()].concat();
        let sig = to_hex(&hmac_sha256(secret.as_bytes(), &signed_payload));
        let header = format!("t={now},v1={sig}");
        assert!(verify_stripe_signature(payload, &header, secret, 300, now).is_ok());
    }

    #[test]
    fn signature_rejects_wrong_secret() {
        let payload = b"{}";
        let now = 1_700_000_000i64;
        let signed_payload = [now.to_string().as_bytes(), b".", payload.as_slice()].concat();
        let sig = to_hex(&hmac_sha256(b"whsec_right", &signed_payload));
        let header = format!("t={now},v1={sig}");
        assert!(verify_stripe_signature(payload, &header, "whsec_wrong", 300, now).is_err());
    }

    #[test]
    fn signature_rejects_stale_timestamp() {
        let secret = "whsec_test_secret"; // allowlist: test-only fixture, not a real Stripe secret
        let payload = b"{}";
        let old = 1_700_000_000i64;
        let now = old + 301;
        let signed_payload = [old.to_string().as_bytes(), b".", payload.as_slice()].concat();
        let sig = to_hex(&hmac_sha256(secret.as_bytes(), &signed_payload));
        let header = format!("t={old},v1={sig}");
        assert!(verify_stripe_signature(payload, &header, secret, 300, now).is_err());
    }

    #[test]
    fn signature_rejects_missing_header_fields() {
        assert!(verify_stripe_signature(b"{}", "garbage", "secret", 300, 0).is_err());
        assert!(verify_stripe_signature(b"{}", "t=123", "secret", 300, 123).is_err());
    }
}
