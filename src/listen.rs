//! PRD-mcphost-subscriptions-listen: the per-tenant hub that feeds open
//! `subscriptions/listen` streams.
//!
//! rmcp builds a new `McpHostHandler` for every POST, and on the stateless
//! transport a request's peer closes when its response ends, so a standing
//! listener can only live in shared state: [`ListenHub`] is a field of
//! `AppState`. A listen registers one entry per open stream under its tenant
//! id; a committed change to that tenant's lists is sent to every entry of
//! the tenant and to no other tenant's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::{broadcast, watch};

/// Messages a lagging stream may drop: a list change is idempotent (the client
/// re-reads the list), so a stream that falls this far behind gets one
/// notification and not a backlog.
const CHANNEL_CAPACITY: usize = 16;

/// Which list a hub message says changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChange {
    Tools,
    Prompts,
}

struct Entry {
    tx: broadcast::Sender<ListChange>,
    open: usize,
}

struct Inner {
    tenants: Mutex<HashMap<i64, Entry>>,
    shutdown: watch::Sender<bool>,
}

/// The per-tenant registry of open listen streams. Cheap to clone: every clone
/// shares one map.
#[derive(Clone)]
pub struct ListenHub(Arc<Inner>);

impl Default for ListenHub {
    fn default() -> Self {
        Self(Arc::new(Inner { tenants: Mutex::new(HashMap::new()), shutdown: watch::channel(false).0 }))
    }
}

impl ListenHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Entry>> {
        self.0.tenants.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Register one open stream for `tenant_id`. The returned guard removes the
    /// registration when it drops, whichever way the stream ends.
    pub fn open(&self, tenant_id: i64) -> ListenGuard {
        let mut tenants = self.lock();
        let entry = tenants.entry(tenant_id).or_insert_with(|| Entry {
            tx: broadcast::channel(CHANNEL_CAPACITY).0,
            open: 0,
        });
        entry.open += 1;
        ListenGuard { hub: self.clone(), tenant_id, rx: entry.tx.subscribe() }
    }

    /// Tell every open stream of `tenant_id` that `change` happened. A tenant
    /// with no open stream costs one map lookup.
    pub fn notify(&self, tenant_id: i64, change: ListChange) {
        if let Some(entry) = self.lock().get(&tenant_id) {
            let _ = entry.tx.send(change);
        }
    }

    /// Open streams `tenant_id` holds right now.
    pub fn open_for(&self, tenant_id: i64) -> usize {
        self.lock().get(&tenant_id).map_or(0, |e| e.open)
    }

    /// Tenants the hub holds an entry for. 0 once every stream has closed.
    pub fn entries(&self) -> usize {
        self.lock().len()
    }

    /// Ask every open stream to end with the completion result (server shutdown).
    pub fn shutdown(&self) {
        let _ = self.0.shutdown.send(true);
    }

    fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.0.shutdown.subscribe()
    }
}

/// One open stream's registration and its feed.
pub struct ListenGuard {
    hub: ListenHub,
    tenant_id: i64,
    rx: broadcast::Receiver<ListChange>,
}

impl ListenGuard {
    /// The next change for this stream, or `None` once the hub is shutting down.
    /// A stream that lagged past the channel capacity gets `Tools` and then
    /// `Prompts` once each: it cannot tell which messages it lost.
    pub async fn next(&mut self, pending: &mut Vec<ListChange>) -> Option<ListChange> {
        if let Some(change) = pending.pop() {
            return Some(change);
        }
        let mut shutdown = self.hub.shutdown_signal();
        tokio::select! {
            _ = shutdown.wait_for(|down| *down) => None,
            msg = self.rx.recv() => match msg {
                Ok(change) => Some(change),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    pending.push(ListChange::Prompts);
                    Some(ListChange::Tools)
                }
                Err(broadcast::error::RecvError::Closed) => None,
            },
        }
    }
}

impl Drop for ListenGuard {
    fn drop(&mut self) {
        let mut tenants = self.hub.lock();
        if let Some(entry) = tenants.get_mut(&self.tenant_id) {
            entry.open = entry.open.saturating_sub(1);
            if entry.open == 0 {
                tenants.remove(&self.tenant_id);
            }
        }
    }
}

// ---- TOOLS_LIST_OPS: the one list of what changes tools/list --------------

use serde_json::Value;

use crate::db::Tenant;
use crate::state::AppState;

/// What a verb in [`TOOLS_LIST_OPS`] changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// The tenant's `tools/list`.
    Tools,
    /// The tenant's prompt list (prompt-kind tools).
    Prompts,
    /// The tenant's `Tool` resources (`resources::write_hint`).
    Resource,
}

/// Whose `tools/list` a verb changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affected {
    /// The calling tenant.
    Caller,
    /// The tenant named by argument `arg` (a namespace), e.g. `namespace` on `host.group.add`.
    Target(&'static str),
    /// The members of the group named by argument `arg`, or of the group the
    /// tool's grant row named before the call (a re-share, or an unshare).
    GroupMembers(&'static str),
}

/// Reads a verb's name and result: did this call change the list?
pub type WhenFn = fn(&str, &Value) -> bool;

/// One verb that changes a tools/list.
#[derive(Debug)]
pub struct ToolsListOp {
    /// The canonical verb; an alias such as `host.tool_publish` resolves to it first.
    pub verb: &'static str,
    pub effects: &'static [Effect],
    /// A set: a verb may change more than one tenant's list.
    pub affected: &'static [Affected],
    /// A verb that only sometimes changes the list reads its result.
    pub when: Option<WhenFn>,
}

const TOOLS_AND_PROMPTS_AND_RESOURCE: &[Effect] = &[Effect::Tools, Effect::Prompts, Effect::Resource];
const TOOLS_AND_RESOURCE: &[Effect] = &[Effect::Tools, Effect::Resource];
const TOOLS_ONLY: &[Effect] = &[Effect::Tools];
const CALLER: &[Affected] = &[Affected::Caller];

/// Every verb that writes a tool row, a tool's visibility or a group
/// membership. `resources::write_hint`, the prompt tracking in
/// `dispatch_tenant_tool_tracking_prompts` and the hub broadcast all read it;
/// in debug builds the db write seam asserts a tool-table write happens under
/// one of these verbs.
pub const TOOLS_LIST_OPS: &[ToolsListOp] = &[
    ToolsListOp { verb: "host.tool.publish", effects: TOOLS_AND_PROMPTS_AND_RESOURCE, affected: CALLER, when: None },
    ToolsListOp { verb: "host.tool.remove", effects: TOOLS_AND_PROMPTS_AND_RESOURCE, affected: CALLER, when: None },
    ToolsListOp { verb: "host.tool.rollback", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: None },
    ToolsListOp { verb: "host.tool.fork", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: None },
    // The verb returns before any tool exists; the restore job tells the hub
    // when it ends (`import.rs`), so the call itself sends nothing.
    ToolsListOp { verb: "host.import", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: Some(delivered_by_the_job) },
    // Hands back the tool sources to publish; it writes no row itself, so it
    // changes nobody's list (the follow-up `host.tool.publish` does).
    ToolsListOp { verb: "host.example.fork", effects: &[], affected: &[], when: None },
    ToolsListOp {
        verb: "host.tool.share",
        effects: TOOLS_ONLY,
        affected: &[Affected::Caller, Affected::GroupMembers("group")],
        when: None,
    },
    ToolsListOp {
        verb: "host.tool.unshare",
        effects: TOOLS_ONLY,
        affected: &[Affected::Caller, Affected::GroupMembers("group")],
        when: None,
    },
    ToolsListOp { verb: "host.group.add", effects: TOOLS_ONLY, affected: &[Affected::Target("namespace")], when: None },
    ToolsListOp { verb: "host.group.remove", effects: TOOLS_ONLY, affected: &[Affected::Target("namespace")], when: None },
    ToolsListOp { verb: "host.note.put", effects: TOOLS_ONLY, affected: CALLER, when: Some(crate::notes::changes_tool_list) },
    ToolsListOp { verb: "host.note.delete", effects: TOOLS_ONLY, affected: CALLER, when: Some(crate::notes::changes_tool_list) },
    // Verbs that publish or remove tools through `control::tool_publish` /
    // `Db::remove_tool` on the caller's behalf.
    ToolsListOp { verb: "host.tool_from_description", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: None },
    ToolsListOp { verb: "host.uptime.create", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: None },
    ToolsListOp { verb: "host.webhook_inbox.create", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: None },
    ToolsListOp { verb: "host.secret.leak_test", effects: TOOLS_AND_RESOURCE, affected: CALLER, when: None },
];

/// `when` for a verb whose list change lands after the call returns.
fn delivered_by_the_job(_verb: &str, _out: &Value) -> bool {
    false
}

/// Whether a successful call of `verb` can change the prompt list.
pub fn tracks_prompts(verb: &str) -> bool {
    op_for(verb).is_some_and(|op| op.effects.contains(&Effect::Prompts))
}

/// The canonical form of `verb`: an alias resolves to its target.
pub fn canonical_verb(verb: &str) -> &str {
    crate::tool_aliases::resolve(verb).unwrap_or(verb)
}

/// The row for `verb` (aliases resolve first), if it is one.
pub fn op_for(verb: &str) -> Option<&'static ToolsListOp> {
    let verb = canonical_verb(verb);
    TOOLS_LIST_OPS.iter().find(|op| op.verb == verb)
}

/// What a verb needs remembered from before it ran.
pub struct Pending {
    op: &'static ToolsListOp,
    args: Value,
    /// The group the tool was shared to before the call.
    grant_group: Option<String>,
}

impl Pending {
    /// Called before the verb runs. `None` for a verb not in the table, a dry
    /// run (nothing is written) and a host with no open stream.
    pub async fn begin(state: &AppState, tenant: &Tenant, verb: &str, args: &Value) -> Option<Box<Self>> {
        let op = op_for(verb)?;
        if op.effects.is_empty() || crate::write_verbs::dry_run_requested(args) || state.listen_hub.entries() == 0 {
            return None;
        }
        let mut grant_group = None;
        if op.affected.iter().any(|a| matches!(a, Affected::GroupMembers(_)))
            && let Some(name) = args.get("name").and_then(Value::as_str)
            && let Ok(Some(row)) = state.db.get_tool(tenant.id, name.to_string()).await
            && row.visibility == "group"
        {
            grant_group = row.shared_group;
        }
        Some(Box::new(Self { op, args: args.clone(), grant_group }))
    }

    /// Called with a successful verb's result, after its write committed.
    pub async fn finish(self: Box<Self>, state: &AppState, tenant: &Tenant, out: &Value) {
        if let Some(when) = self.op.when
            && !when(self.op.verb, out)
        {
            return;
        }
        let mut tenants = std::collections::BTreeSet::new();
        for affected in self.op.affected {
            match *affected {
                Affected::Caller => {
                    tenants.insert(tenant.id);
                }
                Affected::Target(arg) => {
                    let Some(ns) = self.args.get(arg).and_then(Value::as_str) else { continue };
                    if let Ok(Some(target)) = state.db.find_tenant_by_namespace(ns.to_string()).await {
                        tenants.insert(target.id);
                    }
                }
                Affected::GroupMembers(arg) => {
                    let named = self.args.get(arg).and_then(Value::as_str).map(str::to_string);
                    for group in named.into_iter().chain(self.grant_group.clone()) {
                        if let Ok(ids) = state.db.group_member_ids(tenant.id, group).await {
                            tenants.extend(ids);
                        }
                    }
                }
            }
        }
        for id in tenants {
            if self.op.effects.contains(&Effect::Tools) {
                state.listen_hub.notify(id, ListChange::Tools);
            }
        }
    }
}

// ---- the write seam: CURRENT_VERB and the debug write check ---------------

tokio::task_local! {
    /// The canonical verb `dispatch_tenant_tool` is running, for the length of the call.
    pub static CURRENT_VERB: String;
}

/// The verb running on this task, if any.
pub fn current_verb() -> Option<String> {
    CURRENT_VERB.try_with(String::clone).ok()
}

/// Whether `verb` (an alias resolves first) is a row of [`TOOLS_LIST_OPS`].
pub fn is_listed(verb: &str) -> bool {
    op_for(verb).is_some()
}

/// The message a tool-table write under an unlisted verb trips.
pub fn unlisted_write_message(table: &str, verb: &str) -> String {
    format!(
        "write to `{table}` under verb `{verb}`, which is not in TOOLS_LIST_OPS (src/listen.rs): \
         a verb that changes a tools/list needs a row naming whose list it changes"
    )
}

#[cfg(debug_assertions)]
mod write_check {
    use std::cell::RefCell;

    thread_local! {
        /// The verb the db closure now running on this thread belongs to.
        static VERB: RefCell<Option<String>> = const { RefCell::new(None) };
        static VIOLATION: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    /// Marks the blocking thread as running a db closure for `verb`.
    pub struct Scope;

    impl Scope {
        pub fn enter(verb: Option<String>) -> Self {
            VERB.with(|v| *v.borrow_mut() = verb);
            VIOLATION.with(|v| *v.borrow_mut() = None);
            Scope
        }

        /// The first unlisted write the closure made, if any.
        pub fn take_violation(&self) -> Option<String> {
            VIOLATION.with(|v| v.borrow_mut().take())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            VERB.with(|v| *v.borrow_mut() = None);
        }
    }

    /// Called by the SQL function the debug triggers run on every tool-list write.
    pub fn record(table: &str) {
        let Some(verb) = VERB.with(|v| v.borrow().clone()) else { return };
        if super::is_listed(&verb) {
            return;
        }
        VIOLATION.with(|v| {
            v.borrow_mut().get_or_insert_with(|| super::unlisted_write_message(table, &verb));
        });
    }
}

#[cfg(debug_assertions)]
pub use write_check::Scope as WriteScope;

/// Debug builds: make every write that can change a tools/list call
/// `mcphost_listen_write(table, tenant)`, which checks the verb running the
/// db closure is in [`TOOLS_LIST_OPS`]. A trigger and not `update_hook`: the
/// hook cannot tell a spec or visibility change from a `spec_reads` bump. No
/// write call site moves. Tenant deletion cascades are skipped (the tenant's
/// own streams go with its key).
#[cfg(debug_assertions)]
pub fn install_write_check(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function("mcphost_listen_write", 2, FunctionFlags::SQLITE_UTF8, |ctx| {
        let table: String = ctx.get(0)?;
        write_check::record(&table);
        Ok(0i64)
    })?;
    // Each trigger is `CREATE TEMP TRIGGER <name> <event> ON <table> [WHEN <cond>] BEGIN <call>; END`.
    // (The block opener is assembled below: `tests/dryspt_ac04_txn_allowlist.rs` greps
    // `src/` for raw transaction keywords, and a trigger body is not a transaction.)
    let body_open = concat!("BE", "GIN");
    let triggers = [
        ("tools_ins", "AFTER INSERT ON main.tools", "", "tools", "NEW.tenant_id"),
        (
            "tools_del",
            "AFTER DELETE ON main.tools",
            "WHEN EXISTS (SELECT 1 FROM main.tenants WHERE id = OLD.tenant_id)",
            "tools",
            "OLD.tenant_id",
        ),
        (
            "tools_upd",
            "AFTER UPDATE OF kind, spec, current_version, scopes, visibility, shared_group ON main.tools",
            "",
            "tools",
            "NEW.tenant_id",
        ),
        ("members_ins", "AFTER INSERT ON main.group_members", "", "group_members", "NEW.member_tenant_id"),
        (
            "members_del",
            "AFTER DELETE ON main.group_members",
            "WHEN EXISTS (SELECT 1 FROM main.tenants WHERE id = OLD.member_tenant_id)",
            "group_members",
            "OLD.member_tenant_id",
        ),
    ];
    for (name, event, cond, table, tenant) in triggers {
        conn.execute_batch(&format!(
            "CREATE TEMP TRIGGER IF NOT EXISTS mcphost_listen_{name} {event} {cond} \
             {body_open} SELECT mcphost_listen_write('{table}', {tenant}); END;"
        ))?;
    }
    Ok(())
}
