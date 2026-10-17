//! PRD-mcphost-key-delegate-child-keys R2: the subset rule. A child key is
//! never wider than its parent in scopes, tools, lifetime, or depth, and
//! [`narrow`] is the one place that says so -- `host.key.delegate` and the
//! unit test below both call it.

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::scopes::{Scope, held_implies};

/// Deepest chain of delegated keys under the primary (depth 0).
pub const DEPTH_MAX: i64 = 3;

/// What a key may do: the primary's is `["mcp"]`, every tool, no expiry,
/// depth 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Authority {
    pub scopes: Vec<Scope>,
    /// `None` = any tool the tenant owns or has shared in.
    pub tools: Option<Vec<String>>,
    pub expires_at: Option<i64>,
    pub depth: i64,
    pub can_delegate: bool,
}

impl Authority {
    pub fn primary() -> Self {
        Authority { scopes: vec![Scope::Mcp], tools: None, expires_at: None, depth: 0, can_delegate: true }
    }
}

/// A validated `host.key.delegate` request.
#[derive(Clone, Debug)]
pub struct DelegateReq {
    pub scopes: Vec<Scope>,
    pub tools: Option<Vec<String>>,
    pub ttl_s: i64,
    pub now: i64,
    pub can_delegate: bool,
}

/// The first item of a request that exceeds its parent.
#[derive(Clone, Debug, PartialEq)]
pub struct Excess {
    pub kind: &'static str,
    pub value: String,
}

impl Excess {
    pub fn into_error(self) -> AppError {
        AppError::Structured {
            code: SCOPE_ESCALATION,
            message: format!(
                "the requested {} '{}' exceeds the delegating key's own authority; nothing was minted",
                self.kind, self.value
            ),
            data: json!({"excess": {"kind": self.kind, "value": self.value}}),
        }
    }
}

/// The child authority for `req` under `parent`, or the first excess item,
/// checked in the order scope, tool, ttl, depth.
pub fn narrow(parent: &Authority, req: &DelegateReq) -> Result<Authority, Excess> {
    for sc in &req.scopes {
        if !held_implies(&parent.scopes, *sc) {
            return Err(Excess { kind: "scope", value: sc.as_str().to_string() });
        }
    }
    if let (Some(want), Some(allowed)) = (&req.tools, &parent.tools)
        && let Some(extra) = want.iter().find(|t| !allowed.contains(t))
    {
        return Err(Excess { kind: "tool", value: extra.clone() });
    }
    let expires_at = req.now + req.ttl_s;
    if let Some(parent_exp) = parent.expires_at
        && expires_at > parent_exp
    {
        return Err(Excess { kind: "ttl", value: req.ttl_s.to_string() });
    }
    let depth = parent.depth + 1;
    if depth > DEPTH_MAX {
        return Err(Excess { kind: "depth", value: depth.to_string() });
    }
    Ok(Authority {
        scopes: req.scopes.clone(),
        tools: req.tools.clone().or_else(|| parent.tools.clone()),
        expires_at: Some(expires_at),
        depth,
        can_delegate: req.can_delegate,
    })
}

/// `scopes` array of a request/response as wire strings.
pub fn scopes_json(scopes: &[Scope]) -> Value {
    json!(scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>())
}

crate::err_code! {
    pub SCOPE_ESCALATION,
    code: "scope_escalation", family: Auth, class: AgentFault, retryable: false,
    remedy: "Request only scopes, tools and limits inside the delegating key's own authority; data.excess names the one over.",
}


#[cfg(test)]
mod tests {
    use super::*;

    fn req(scopes: Vec<Scope>) -> DelegateReq {
        DelegateReq { scopes, tools: None, ttl_s: 60, now: 1_000, can_delegate: false }
    }

    #[test]
    fn every_scope_pair_is_allowed_exactly_when_it_implies() {
        for &parent_scope in Scope::ALL {
            let parent = Authority { scopes: vec![parent_scope], ..Authority::primary() };
            for &want in Scope::ALL {
                let got = narrow(&parent, &req(vec![want]));
                assert_eq!(got.is_ok(), parent_scope.implies(want), "{parent_scope:?} -> {want:?}");
                if let Err(e) = got {
                    assert_eq!(e, Excess { kind: "scope", value: want.as_str().to_string() });
                }
            }
        }
    }

    #[test]
    fn ttl_depth_and_tool_excess_are_named() {
        let parent = Authority {
            scopes: vec![Scope::ToolsCall],
            tools: Some(vec!["a".into()]),
            expires_at: Some(1_600),
            depth: 3,
            can_delegate: true,
        };
        let mut r = req(vec![Scope::ToolsCall]);
        r.tools = Some(vec!["b".into()]);
        assert_eq!(narrow(&parent, &r).unwrap_err().kind, "tool");
        r.tools = Some(vec!["a".into()]);
        r.ttl_s = 1_200;
        assert_eq!(narrow(&parent, &r).unwrap_err().kind, "ttl");
        r.ttl_s = 300;
        assert_eq!(narrow(&parent, &r).unwrap_err().kind, "depth");
    }
}
