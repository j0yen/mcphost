//! PRD-mcphost-sync-pull-with-change-detection: the offline fixture server
//! behind `sypul_ac*`. A real axum server on an ephemeral loopback port that
//! answers every path with one configurable [`Reply`] -- a status, a body, an
//! optional `ETag` (a matching `If-None-Match` earns a 304) and an optional
//! `Retry-After` -- and counts the requests it served.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub etag: bool,
    pub retry_after: Option<u64>,
    pub content_type: &'static str,
}

struct Inner {
    reply: Mutex<Reply>,
    hits: AtomicU64,
}

#[derive(Clone)]
pub struct Fixture {
    pub base: String,
    inner: Arc<Inner>,
}

fn etag_of(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    format!("\"{}\"", digest.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>())
}

async fn serve(State(inner): State<Arc<Inner>>, headers: HeaderMap) -> Response {
    inner.hits.fetch_add(1, Ordering::SeqCst);
    let reply = inner.reply.lock().expect("fixture reply").clone();
    let status = StatusCode::from_u16(reply.status).expect("status");
    let mut out_headers = HeaderMap::new();
    if let Some(s) = reply.retry_after {
        out_headers.insert(header::RETRY_AFTER, HeaderValue::from_str(&s.to_string()).expect("retry-after"));
    }
    if reply.etag && status == StatusCode::OK {
        let etag = etag_of(&reply.body);
        out_headers.insert(header::ETAG, HeaderValue::from_str(&etag).expect("etag"));
        if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
            return (StatusCode::NOT_MODIFIED, out_headers).into_response();
        }
    }
    out_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(reply.content_type));
    (status, out_headers, reply.body).into_response()
}

impl Fixture {
    /// Serves `body` with 200; `etag` adds a content `ETag` and 304 support.
    pub async fn start(body: &str, etag: bool) -> Fixture {
        let inner = Arc::new(Inner {
            reply: Mutex::new(Reply {
                status: 200,
                body: body.to_string(),
                etag,
                retry_after: None,
                content_type: "text/csv",
            }),
            hits: AtomicU64::new(0),
        });
        let app = Router::new().fallback(serve).with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
        let addr = listener.local_addr().expect("fixture addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Fixture { base: format!("http://{addr}"), inner }
    }

    /// A url on this server (the path is ignored by the server).
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    pub fn csv_url(&self) -> String {
        self.url("data.csv")
    }

    pub fn set_body(&self, body: &str) {
        self.inner.reply.lock().expect("fixture reply").body = body.to_string();
    }

    pub fn set_status(&self, status: u16, retry_after: Option<u64>) {
        let mut reply = self.inner.reply.lock().expect("fixture reply");
        reply.status = status;
        reply.retry_after = retry_after;
    }

    pub fn set_content_type(&self, content_type: &'static str) {
        self.inner.reply.lock().expect("fixture reply").content_type = content_type;
    }

    /// Requests served so far.
    pub fn hits(&self) -> u64 {
        self.inner.hits.load(Ordering::SeqCst)
    }
}

/// `id,name,qty` with `rows` data lines: `id` is `1..=rows`.
pub fn orders_csv(rows: usize) -> String {
    let mut out = String::from("id,name,qty\n");
    for i in 1..=rows {
        out.push_str(&format!("{i},item {i},{}\n", i % 7));
    }
    out
}
