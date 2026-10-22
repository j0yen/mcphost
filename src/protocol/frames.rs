//! PRD-mcphost-sse-frame-contract-generated: the frames the host may emit per
//! request type, as one table.
//!
//! Under `rmcp`'s `json_response(true)` a response is plain `application/json`
//! unless a message precedes the result on the same request's channel, in
//! which case the one response upgrades to `text/event-stream` carrying every
//! frame in order. [`FRAME_SEQUENCES`] declares, per request type, which
//! frames may appear and in what order. `scripts/protocol-frames-check.sh`
//! renders `docs/protocol/frames.md`, `www/frames.json` and the
//! `www/llms.txt` Transport section from it; `tests/sfc_ac02_*` replays every
//! row against an in-process server. A notify call outside the table trips
//! [`emit`] in debug builds.
//!
//! `scripts/protocol-frames-check.sh` reads the table below as text, so keep
//! every row in the single `FrameRow { .. }` shape it parses.

use rmcp::{RoleServer, service::Peer};
use serde_json::Value;

/// The MCP protocol version the host negotiates and `frames.json` reports.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// The response header carrying the host's own handling time on `POST /mcp`.
/// The one place the name is spelled; `src/http.rs` and the generated docs
/// (`scripts/protocol-frames-check.sh`) read it from here.
pub const SERVER_TIMING_HEADER: &str = "Server-Timing";

/// The `Server-Timing` metric name: `app;dur=<ms>`. `dur` is the wall time
/// from the request reaching the `/mcp` route to the response head being
/// ready, in milliseconds (decimal, three places), so auth, rate-limit and
/// handler time are inside it and Caddy/TLS/network time is not. For a
/// `text/event-stream` response it is time to response head, not to the last
/// frame.
pub const SERVER_TIMING_METRIC: &str = "app";

/// A frame the host can put on a response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    ListChanged,
    /// `notifications/prompts/list_changed`: a prompt-kind tool was published
    /// or removed (PRD-mcphost-prompts-from-published-tools R4).
    PromptsListChanged,
    Progress,
    Result,
    Error,
}

impl FrameKind {
    /// The wire label of this frame: the JSON-RPC `method` for a
    /// notification, `result` / `error` for the terminal frame.
    pub const fn method(self) -> &'static str {
        match self {
            FrameKind::ListChanged => "notifications/tools/list_changed",
            FrameKind::PromptsListChanged => "notifications/prompts/list_changed",
            FrameKind::Progress => "notifications/progress",
            FrameKind::Result => "result",
            FrameKind::Error => "error",
        }
    }

    /// Classify one decoded JSON-RPC frame.
    pub fn classify(frame: &Value) -> Option<FrameKind> {
        if let Some(method) = frame.get("method").and_then(Value::as_str) {
            return [FrameKind::ListChanged, FrameKind::PromptsListChanged, FrameKind::Progress].into_iter().find(|k| k.method() == method);
        }
        if frame.get("result").is_some() {
            return Some(FrameKind::Result);
        }
        frame.get("error").map(|_| FrameKind::Error)
    }

    fn is_terminal(self) -> bool {
        matches!(self, FrameKind::Result | FrameKind::Error)
    }
}

/// How many times a frame kind may appear in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Card {
    ExactlyOne,
    AtMostOne,
    ZeroOrMore,
}

impl Card {
    pub const fn bounds(self) -> (usize, usize) {
        match self {
            Card::ExactlyOne => (1, 1),
            Card::AtMostOne => (0, 1),
            Card::ZeroOrMore => (0, usize::MAX),
        }
    }

    /// `1`, `0,1` or `0,n` -- the notation the rendered docs use.
    pub const fn notation(self) -> &'static str {
        match self {
            Card::ExactlyOne => "1",
            Card::AtMostOne => "0,1",
            Card::ZeroOrMore => "0,n",
        }
    }
}

/// How the response content type follows from the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentTypeRule {
    /// `application/json` when the body is one frame, `text/event-stream`
    /// when more than one frame precedes the end of the response.
    JsonWhenSingleFrame,
}

impl ContentTypeRule {
    pub const fn name(self) -> &'static str {
        match self {
            ContentTypeRule::JsonWhenSingleFrame => "json_when_single_frame",
        }
    }

    /// The content type a body of `frame_count` frames must carry.
    pub const fn content_type_for(self, frame_count: usize) -> &'static str {
        match self {
            ContentTypeRule::JsonWhenSingleFrame => {
                if frame_count <= 1 {
                    "application/json"
                } else {
                    "text/event-stream"
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameRow {
    pub request_type: &'static str,
    pub frames: &'static [(FrameKind, Card)],
    pub content_type_rule: ContentTypeRule,
}

/// The request types the host serves and the ordered frames each may emit.
/// `ping` is a request type the 2026-07-28 stateless transport does not
/// serve: the drift test observes a `-32601` error frame, so that is the row.
/// `tools/call:keyless_signup` is both the explicit `signup` / `host.redeem`
/// path and the implicit signup of a first key-less call: both bind the
/// connection and announce the new tool list before their own result.
pub const FRAME_SEQUENCES: &[FrameRow] = &[
    FrameRow {
        request_type: "initialize",
        frames: &[(FrameKind::Result, Card::ExactlyOne)],
        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,
    },
    FrameRow {
        request_type: "tools/list",
        frames: &[(FrameKind::Result, Card::ExactlyOne)],
        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,
    },
    FrameRow {
        request_type: "tools/call:keyed",
        frames: &[
            (FrameKind::ListChanged, Card::AtMostOne),
            (FrameKind::PromptsListChanged, Card::AtMostOne),
            (FrameKind::Result, Card::ExactlyOne),
        ],
        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,
    },
    FrameRow {
        request_type: "tools/call:keyless_signup",
        frames: &[(FrameKind::ListChanged, Card::AtMostOne), (FrameKind::Result, Card::ExactlyOne)],
        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,
    },
    FrameRow {
        request_type: "tools/call:error",
        frames: &[(FrameKind::Error, Card::ExactlyOne)],
        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,
    },
    FrameRow {
        request_type: "ping",
        frames: &[(FrameKind::Error, Card::ExactlyOne)],
        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,
    },
];

/// Request type of the signup / redeem / implicit-signup notify sites.
pub const REQ_KEYLESS_SIGNUP: &str = "tools/call:keyless_signup";

pub fn row_for(request_type: &str) -> Option<&'static FrameRow> {
    FRAME_SEQUENCES.iter().find(|r| r.request_type == request_type)
}

/// Does the observed frame-kind sequence satisfy `row`? With
/// `terminal_interchangeable`, a `result` and an `error` count as the same
/// slot (a caller that does not yet know whether its call succeeds).
fn sequence_matches(row: &FrameRow, observed: &[Option<FrameKind>], terminal_interchangeable: bool) -> bool {
    let same = |want: FrameKind, got: FrameKind| want == got || (terminal_interchangeable && want.is_terminal() && got.is_terminal());
    let mut at = 0;
    for (kind, card) in row.frames {
        let (min, max) = card.bounds();
        let mut n = 0;
        while n < max && at < observed.len() && observed[at].is_some_and(|got| same(*kind, got)) {
            n += 1;
            at += 1;
        }
        if n < min {
            return false;
        }
    }
    at == observed.len()
}

/// Wire labels of a decoded body, for messages: the `method`, else
/// `result` / `error`, else `?`.
pub fn observed_methods(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .map(|f| FrameKind::classify(f).map(|k| k.method().to_string()).or_else(|| f.get("method").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| "?".to_string()))
        .collect()
}

/// Does the decoded body match the row for `request_type` exactly?
pub fn body_matches_row(row: &FrameRow, frames: &[Value]) -> bool {
    let kinds: Vec<_> = frames.iter().map(FrameKind::classify).collect();
    sequence_matches(row, &kinds, false)
}

/// Split an SSE body into its decoded JSON-RPC frames: `\n\n`-separated
/// blocks, the first `data:` line of each.
pub fn split_sse_frames(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter_map(|block| block.lines().find_map(|line| line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:"))))
        .filter(|data| !data.is_empty())
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .collect()
}

/// A body whose frame sequence the table does not allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameContractViolation {
    pub request_type: String,
    pub observed: Vec<String>,
}

impl FrameContractViolation {
    pub const CODE: &'static str = "frame_contract_violation";
}

impl std::fmt::Display for FrameContractViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: request type {} observed frames [{}]", Self::CODE, self.request_type, self.observed.join(", "))
    }
}

impl std::error::Error for FrameContractViolation {}

/// Debug builds panic, release builds log, when `kind` is not allowed on a
/// response to `request_type`.
pub fn assert_allowed(kind: FrameKind, request_type: &str) {
    let allowed = row_for(request_type).is_some_and(|row| row.frames.iter().any(|(k, _)| *k == kind));
    if allowed {
        return;
    }
    let message = format!("frame contract: request type {request_type} does not allow frame {kind:?} ({})", kind.method());
    if cfg!(debug_assertions) {
        panic!("{message}");
    }
    tracing::error!(request_type, frame = kind.method(), "{message}");
}

/// Emit `kind` on the current request's channel, after checking the table.
/// Best-effort like the notify calls it replaces: a send failure is the
/// caller's to log.
pub async fn emit(peer: &Peer<RoleServer>, kind: FrameKind, request_type: &str) -> Result<(), String> {
    assert_allowed(kind, request_type);
    match kind {
        FrameKind::ListChanged => peer.notify_tool_list_changed().await.map_err(|e| e.to_string()),
        FrameKind::PromptsListChanged => peer.notify_prompt_list_changed().await.map_err(|e| e.to_string()),
        other => Err(format!("frames::emit has no sender for {other:?}")),
    }
}

/// The request type a client names for a call it is about to make: `method`,
/// whether it is `tools/call` with a bearer (`keyed`) or without (the
/// implicit or explicit signup path, `keyless_signup`).
pub fn request_type_of(method: &str, has_bearer: bool) -> &str {
    match (method, has_bearer) {
        ("tools/call", true) => "tools/call:keyed",
        ("tools/call", false) => REQ_KEYLESS_SIGNUP,
        (other, _) => other,
    }
}

/// The `result` / `error` frame of a decoded body, after checking the body's
/// frame sequence against the row for `request_type`. A body the row does not
/// allow is `frame_contract_violation` naming the observed methods. A request
/// type with no row has no contract to check: its last `result` / `error`
/// frame is returned.
pub fn select_result<'a>(frames: &'a [Value], request_type: &str) -> Result<&'a Value, FrameContractViolation> {
    let violation = || FrameContractViolation { request_type: request_type.to_string(), observed: observed_methods(frames) };
    if let Some(row) = row_for(request_type) {
        let kinds: Vec<_> = frames.iter().map(FrameKind::classify).collect();
        if !sequence_matches(row, &kinds, true) {
            return Err(violation());
        }
    }
    frames.iter().rev().find(|f| FrameKind::classify(f).is_some_and(FrameKind::is_terminal)).ok_or_else(violation)
}
