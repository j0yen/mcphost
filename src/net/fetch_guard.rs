//! PRD-mcphost-table-import-from-url R2: the SSRF-guarded, size-capped GET a
//! `source_url` import fetches with. https only; every hop (the first URL and
//! each of at most [`MAX_REDIRECTS`] redirects) is refused when its host is a
//! literal address in a loopback, private, link-local, CGNAT, multicast or
//! cloud-metadata range, and a DNS name is refused at connect when any address
//! it resolves to is (the vetted address is the one `reqwest` connects to, so
//! a rebinding answer cannot slip past). The body is counted while it streams
//! and the fetch stops one chunk past the cap, never buffering more.
//!
//! The range table is [`crate::kinds::http::is_disallowed_ip`] plus the ranges
//! an upstream `http` tool does not need to name (CGNAT, multicast, benchmark,
//! IPv4-mapped IPv6). A loopback-hosted instance (`url_policy`: local dev, the
//! test harness) may fetch loopback and plain http for the FIRST hop only; a
//! redirect is https-only everywhere.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{Url, redirect};

use crate::kinds::http::{HickoryLookup, NameLookup, is_disallowed_ip};

/// Redirects followed before the fetch is refused.
pub const MAX_REDIRECTS: usize = 3;
/// Wall-clock budget for the whole fetch, connect to last body byte.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// The byte cap when a plan names none.
pub const DEFAULT_CAP_BYTES: u64 = 25 * 1024 * 1024;

/// Who may be fetched. `loopback_dev` is true only for a loopback-hosted
/// instance (see `index::url_policy`); production never sets it.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchPolicy {
    pub loopback_dev: bool,
}

/// Why a fetch did not produce a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The URL (or a redirect hop) names a host the guard refuses.
    Forbidden { host: String, reason: String },
    /// The upstream could not be read: a connect, timeout, status or DNS failure.
    Failed { host: String, detail: String, status: Option<u16>, hint: Option<String> },
    /// The body passed `cap` bytes.
    TooLarge { host: String, cap: u64 },
}

/// A fetched body.
#[derive(Debug)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
    pub host: String,
}

/// The marker a redirect or resolver refusal raises so the `reqwest::Error`
/// chain can be walked back to a [`FetchError::Forbidden`].
#[derive(Debug, Clone)]
struct Refusal {
    host: String,
    reason: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.host, self.reason)
    }
}

impl std::error::Error for Refusal {}

/// Whether `ip` is in a range no import may reach.
pub fn ip_forbidden(ip: IpAddr, loopback_dev: bool) -> bool {
    if let IpAddr::V6(v6) = ip
        && let Some(v4) = v6.to_ipv4_mapped()
    {
        return ip_forbidden(IpAddr::V4(v4), loopback_dev);
    }
    if is_disallowed_ip(ip, loopback_dev) {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => extra_v4_forbidden(v4),
        IpAddr::V6(v6) => v6.is_loopback() && !loopback_dev,
    }
}

fn extra_v4_forbidden(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 0x40)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || o[0] >= 240
}

fn refusal_for(url: &Url, first_hop: bool, policy: FetchPolicy) -> Option<(String, String)> {
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']).to_string();
    let https = url.scheme() == "https";
    let http_ok = first_hop && policy.loopback_dev && url.scheme() == "http";
    if !https && !http_ok {
        return Some((host, format!("{} is not https", url.scheme())));
    }
    if host.is_empty() {
        return Some((host, "no host".to_string()));
    }
    // `Url` has already folded decimal/hex/octal IPv4 forms into dotted quads.
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip_forbidden(ip, policy.loopback_dev).then(|| (host, "private or reserved address".to_string()));
    }
    let d = host.trim_end_matches('.').to_ascii_lowercase();
    let local = !policy.loopback_dev && (d == "localhost" || d.ends_with(".localhost"));
    (local || d == "internal" || d.ends_with(".internal")).then(|| (host, "internal host name".to_string()))
}

/// The refusal for a redirect to `next` when `visited` URLs (the first one
/// included) were already fetched: more than [`MAX_REDIRECTS`] hops, plain
/// http, or a forbidden host. Redirects are https-only on every instance.
pub fn hop_refusal(next: &Url, visited: usize, policy: FetchPolicy) -> Option<(String, String)> {
    if visited > MAX_REDIRECTS {
        return Some((next.host_str().unwrap_or("").trim_matches(['[', ']']).to_string(), format!("more than {MAX_REDIRECTS} redirects")));
    }
    refusal_for(next, false, policy)
}

/// The refusal for `raw` as a first hop, if any (the pre-flight check).
pub fn check_url(raw: &str, policy: FetchPolicy) -> Result<Url, FetchError> {
    let url = Url::parse(raw).map_err(|e| FetchError::Forbidden { host: raw.chars().take(80).collect(), reason: format!("not a valid url: {e}") })?;
    match refusal_for(&url, true, policy) {
        Some((host, reason)) => Err(FetchError::Forbidden { host, reason }),
        None => Ok(url),
    }
}

struct GuardResolver {
    lookup: Arc<dyn NameLookup>,
}

impl Resolve for GuardResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let lookup = self.lookup.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            let ips = lookup.lookup(host.clone()).await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            if ips.iter().any(|ip| ip_forbidden(*ip, false)) {
                let refusal = Refusal { host, reason: "resolves to a private or reserved address".to_string() };
                return Err(Box::new(refusal) as Box<dyn std::error::Error + Send + Sync>);
            }
            let addrs: Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

fn build_client(policy: FetchPolicy, lookup: Option<Arc<dyn NameLookup>>) -> Result<reqwest::Client, String> {
    let hops = redirect::Policy::custom(move |attempt| match hop_refusal(attempt.url(), attempt.previous().len(), policy) {
        Some((host, reason)) => attempt.error(Refusal { host, reason }),
        None => attempt.follow(),
    });
    let mut builder = reqwest::Client::builder().redirect(hops).timeout(TOTAL_TIMEOUT).user_agent("mcphost-import/1");
    if !policy.loopback_dev {
        let lookup = match lookup {
            Some(l) => l,
            None => Arc::new(HickoryLookup::new().map_err(|e| format!("{e:?}"))?),
        };
        builder = builder.dns_resolver(Arc::new(GuardResolver { lookup }));
    }
    builder.build().map_err(|e| e.to_string())
}

/// The `export?format=csv` form of a Google Sheets URL, for `data.hint`.
pub fn sheets_export_hint(url: &Url) -> Option<String> {
    if url.host_str() != Some("docs.google.com") {
        return None;
    }
    let mut parts = url.path_segments()?;
    (parts.next()? == "spreadsheets" && parts.next()? == "d").then_some(())?;
    let id = parts.next().filter(|s| !s.is_empty())?;
    Some(format!("https://docs.google.com/spreadsheets/d/{id}/export?format=csv"))
}

fn classify(err: reqwest::Error, host: &str, hint: Option<String>) -> FetchError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&err);
    while let Some(e) = source {
        if let Some(r) = e.downcast_ref::<Refusal>() {
            return FetchError::Forbidden { host: r.host.clone(), reason: r.reason.clone() };
        }
        source = e.source();
    }
    let status = err.status().map(|s| s.as_u16());
    let detail = if err.is_timeout() { format!("timed out after {} s", TOTAL_TIMEOUT.as_secs()) } else { err.without_url().to_string() };
    FetchError::Failed { host: host.to_string(), detail, status, hint }
}

/// GETs `raw` with the guard, reading at most `cap` bytes. Every body byte read
/// is added to `received` as it arrives, so a caller records the true count
/// even when the fetch ends in an error.
pub async fn fetch_capped(
    raw: &str,
    cap: u64,
    policy: FetchPolicy,
    lookup: Option<Arc<dyn NameLookup>>,
    received: &AtomicU64,
) -> Result<Fetched, FetchError> {
    let url = check_url(raw, policy)?;
    let host = url.host_str().unwrap_or("").to_string();
    let hint = sheets_export_hint(&url);
    // A Sheets edit page is HTML behind a login: name the CSV export instead of fetching it.
    if let Some(h) = &hint
        && !url.path().ends_with("/export")
    {
        return Err(FetchError::Failed { host, detail: "a Google Sheets edit URL is not a CSV".to_string(), status: None, hint: Some(h.clone()) });
    }
    let client = build_client(policy, lookup).map_err(|detail| FetchError::Failed { host: host.clone(), detail, status: None, hint: None })?;
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/csv, application/x-ndjson, application/jsonl, application/json;q=0.9, text/plain;q=0.8, */*;q=0.1")
        .send()
        .await
        .map_err(|e| classify(e, &host, hint.clone()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(FetchError::Failed { host, detail: format!("upstream answered HTTP {}", status.as_u16()), status: Some(status.as_u16()), hint });
    }
    if resp.content_length().is_some_and(|n| n > cap) {
        return Err(FetchError::TooLarge { host, cap });
    }
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut bytes: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| classify(e, &host, hint.clone()))?;
        let total = received.fetch_add(chunk.len() as u64, Ordering::SeqCst) + chunk.len() as u64;
        if total > cap {
            return Err(FetchError::TooLarge { host, cap });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched { bytes, content_type, host })
}
