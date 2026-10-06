//! PRD-mcphost-human-claim-magic-link requirement 5: the outbound email
//! provider `claim::post_claim` sends a magic link through -- an HTTP JSON
//! API configured by `MCPHOST_EMAIL_API_URL`/`MCPHOST_EMAIL_API_KEY`/
//! `MCPHOST_EMAIL_FROM`, with a fake client in tests (same
//! trait-plus-real-plus-fake shape as [`crate::billing::BillingClient`] /
//! [`crate::billing::StripeClient`] / [`crate::billing::FakeBillingClient`]
//! -- technical considerations: "Tests never reach the network").

use serde_json::json;

use crate::errors::AppError;

/// PRD-mcphost-ownership-moment requirement 3 (AC3): `MCPHOST_EMAIL_PROVIDER`
/// selects the outbound request's own shape -- Resend's `{from, to,
/// subject, text}` plus a bearer `Authorization`, or Postmark's
/// `{From, To, Subject, TextBody}` plus an `X-Postmark-Server-Token`
/// header. Behind one enum, not a trait per provider (technical
/// considerations: "no new crate" -- there's no vendor SDK here to begin
/// with, just two JSON shapes [`HttpEmailClient::send`] switches on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EmailProvider {
    #[default]
    Resend,
    Postmark,
}

impl EmailProvider {
    /// Open question, decided 2026-10-05 (Joe): Resend is the default;
    /// `MCPHOST_EMAIL_PROVIDER=postmark` is the only other recognized
    /// value. Anything else (including unset) is Resend -- an operator
    /// typo must never silently stop mail from sending.
    pub fn from_env() -> Self {
        match std::env::var("MCPHOST_EMAIL_PROVIDER").ok().as_deref() {
            Some("postmark") => Self::Postmark,
            _ => Self::Resend,
        }
    }
}

/// One outbound email: the magic-link send, and nothing else today.
/// `from`/`reply_to` travel with the message (rather than living on the
/// client) so the display-name form of `$MCPHOST_EMAIL_FROM` (e.g.
/// `mcphost <hello@mcphost.dev>`) reaches the provider verbatim.
#[derive(Debug, Clone)]
pub struct EmailMessage {
    pub to: String,
    pub from: String,
    pub reply_to: String,
    pub subject: String,
    pub text_body: String,
}

/// requirement 2: the bare address a human can actually reply to --
/// `"mcphost <hello@mcphost.dev>"` -> `"hello@mcphost.dev"`; a `from`
/// with no `<...>` display-name form is already bare and comes back
/// unchanged.
pub fn reply_to_address(from: &str) -> String {
    match (from.find('<'), from.find('>')) {
        (Some(start), Some(end)) if end > start => from[start + 1..end].trim().to_string(),
        _ => from.trim().to_string(),
    }
}

/// `$MCPHOST_EMAIL_API_URL`/`$MCPHOST_EMAIL_API_KEY`/`$MCPHOST_EMAIL_FROM`;
/// `None` fields (in practice, an absent `api_url`) are requirement 5's
/// "email delivery is not configured" state -- [`Self::is_configured`] is
/// the one place that decision is made, so every call site (the claim
/// page, `/healthz`'s `claim_email_configured`) agrees.
#[derive(Clone, Default)]
pub struct EmailConfig {
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub from: Option<String>,
    /// PRD-mcphost-ownership-moment requirement 3 (AC3): resolved once,
    /// here, from `$MCPHOST_EMAIL_PROVIDER` -- every other field predates
    /// this PRD.
    pub provider: EmailProvider,
}

impl EmailConfig {
    pub fn from_env() -> Self {
        Self {
            api_url: std::env::var("MCPHOST_EMAIL_API_URL").ok(),
            api_key: std::env::var("MCPHOST_EMAIL_API_KEY").ok(),
            from: std::env::var("MCPHOST_EMAIL_FROM").ok(),
            provider: EmailProvider::from_env(),
        }
    }

    /// AC6: an absent `MCPHOST_EMAIL_API_URL` is the whole "not configured"
    /// signal -- `api_key`/`from` being unset too is the same host, so
    /// they aren't consulted separately.
    pub fn is_configured(&self) -> bool {
        self.api_url.is_some()
    }
}

/// The outbound half of email: send one message. A trait so `AppState` can
/// hold a [`FakeEmailClient`] in every test and an [`HttpEmailClient`] in
/// production, with `claim::post_claim`'s own logic none the wiser which
/// one it's calling.
#[async_trait::async_trait]
pub trait EmailClient: Send + Sync {
    async fn send(&self, message: &EmailMessage) -> Result<(), AppError>;
}

/// The real implementation: `POST $MCPHOST_EMAIL_API_URL`, JSON body,
/// bearer-authenticated with `$MCPHOST_EMAIL_API_KEY` -- generic enough to
/// front Resend, Postmark, or any similarly-shaped transactional-email API
/// (Open question: "Resend or Postmark" is still undecided at build time;
/// this is a plain `{from, to, subject, text}` POST rather than a
/// vendor-specific SDK, same "no SDK crate" choice `billing::StripeClient`
/// already made for Stripe). Never constructed by a test; see
/// [`FakeEmailClient`].
pub struct HttpEmailClient {
    http: reqwest::Client,
    api_url: String,
    api_key: Option<String>,
    provider: EmailProvider,
}

impl HttpEmailClient {
    pub fn new(
        http: reqwest::Client,
        api_url: String,
        api_key: Option<String>,
        provider: EmailProvider,
    ) -> Self {
        Self { http, api_url, api_key, provider }
    }
}

#[async_trait::async_trait]
impl EmailClient for HttpEmailClient {
    async fn send(&self, message: &EmailMessage) -> Result<(), AppError> {
        // AC3: the request shape (body keys and auth header) is selected
        // by `self.provider` alone -- `api_url`/`api_key` stay the same
        // two env-driven values either way; `from`/`reply_to` travel on
        // the message (requirement 2).
        let req = match self.provider {
            EmailProvider::Resend => {
                let mut req = self.http.post(&self.api_url).json(&json!({
                    "from": message.from,
                    "to": message.to,
                    "reply_to": message.reply_to,
                    "subject": message.subject,
                    "text": message.text_body,
                }));
                if let Some(key) = self.api_key.as_deref() {
                    req = req.bearer_auth(key);
                }
                req
            }
            EmailProvider::Postmark => {
                let mut req = self.http.post(&self.api_url).json(&json!({
                    "From": message.from,
                    "To": message.to,
                    "ReplyTo": message.reply_to,
                    "Subject": message.subject,
                    "TextBody": message.text_body,
                }));
                if let Some(key) = self.api_key.as_deref() {
                    req = req.header("X-Postmark-Server-Token", key);
                }
                req
            }
        };
        let resp = req.send().await.map_err(|_| AppError::claim_email_failed(None))?;
        if !resp.status().is_success() {
            return Err(AppError::claim_email_failed(Some(resp.status().as_u16())));
        }
        Ok(())
    }
}

/// Test-only: never reaches the network (technical considerations).
/// `sends` records every message this fake actually accepted (a failed
/// attempt, per [`Self::fail_next`], is never pushed here) -- AC2/AC10's
/// "exactly one / one send is ultimately recorded" read this.
#[cfg(any(test, feature = "test-support"))]
pub struct FakeEmailClient {
    sends: std::sync::Mutex<Vec<EmailMessage>>,
    fail_next: std::sync::atomic::AtomicUsize,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeEmailClient {
    pub fn new() -> Self {
        Self {
            sends: std::sync::Mutex::new(Vec::new()),
            fail_next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// AC10: the next `n` `send` calls fail (as if the provider returned
    /// HTTP 500) before the following one succeeds -- `claim::send_with_retry`'s
    /// one retry is enough to recover from `n == 1`.
    pub fn fail_next(&self, n: usize) {
        self.fail_next.store(n, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn send_count(&self) -> usize {
        self.sends.lock().map(|s| s.len()).unwrap_or(0)
    }

    pub fn sends(&self) -> Vec<EmailMessage> {
        self.sends.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for FakeEmailClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl EmailClient for FakeEmailClient {
    async fn send(&self, message: &EmailMessage) -> Result<(), AppError> {
        let remaining = self.fail_next.load(std::sync::atomic::Ordering::SeqCst);
        if remaining > 0 {
            self.fail_next
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return Err(AppError::claim_email_failed(Some(500)));
        }
        if let Ok(mut guard) = self.sends.lock() {
            guard.push(message.clone());
        }
        Ok(())
    }
}
