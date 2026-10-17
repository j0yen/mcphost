//! `tenant_clients` + `join_codes` (PRD-mcphost-same-user-second-agent): a
//! second client of the same user joins the tenant with a credential of its
//! own -- no key, no claim code, no email. The tenant's pre-existing
//! credentials (key, `/u/` secret) are client 0 (`c0-<tenant id>`); a
//! sibling gets its own `/u/<secret>/mcp` secret through `/j/<code>/mcp`.
//! Same shape as `invites.rs`/`org.rs`: `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out; `handler.rs` owns the wire.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::auth::{generate_url_secret, hash_key};
use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// R3: a tenant's join code allows this many first-calls ...
pub const DEFAULT_JOIN_MAX_USES: i64 = 5;
/// ... and lives this long (7 days).
pub const JOIN_CODE_TTL_SECS: i64 = 7 * 86_400;

tokio::task_local! {
    /// The calling client's id for the tenant verbs that need it
    /// (`host.whoami`, `host.key.rotate`), set by `handler::call_tool_inner`
    /// around dispatch -- a task-local rather than a parameter threaded
    /// through three dispatch layers.
    pub static CALL_CLIENT: Option<String>;
}

/// The id [`CALL_CLIENT`] holds, `None` outside a scoped call (client 0).
pub fn current_client() -> Option<String> {
    CALL_CLIENT.try_with(|c| c.clone()).ok().flatten()
}

/// Client 0's id: the tenant's pre-existing key / `/u/` secret.
pub fn first_client_id(tenant_id: i64) -> String {
    format!("c0-{tenant_id}")
}

/// R7: `<surface id>-<ordinal>` for a client whose `clientInfo.name` is in
/// [`crate::install_links::CLIENT_INFO_NAMES`], `client-<ordinal>` otherwise.
/// The surface id comes from the one table (`install_links::surface_id`), so
/// a new client cannot drift from the label.
pub fn derive_label(client_name: Option<&str>, ordinal: i64) -> String {
    let surface = crate::install_links::surface_id(client_name);
    let stem = if surface == "other" { "client" } else { surface };
    format!("{stem}-{ordinal}")
}

pub fn join_url(state: &AppState, code: &str) -> String {
    format!("{}/j/{code}/mcp", state.public_url.trim_end_matches('/'))
}

fn client_url(state: &AppState, secret: &str) -> String {
    format!("{}/u/{secret}/mcp", state.public_url.trim_end_matches('/'))
}

/// The `<code>` segment of a `/j/{code}/mcp` request.
pub fn join_path_code(parts: &http::request::Parts) -> Option<&str> {
    parts.uri.path().strip_prefix("/j/")?.strip_suffix("/mcp")
}

#[derive(Debug, Clone)]
pub struct ClientRow {
    pub id: String,
    pub tenant_id: i64,
    pub secret_hash: Option<String>,
    pub label: String,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub created_unix: i64,
    pub last_seen_unix: Option<i64>,
    pub revoked_unix: Option<i64>,
}

const CLIENT_COLUMNS: &str = "id, tenant_id, secret_hash, label, client_name, client_version, \
    created_unix, last_seen_unix, revoked_unix";

fn client_from_row(r: &rusqlite::Row) -> rusqlite::Result<ClientRow> {
    Ok(ClientRow {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        secret_hash: r.get(2)?,
        label: r.get(3)?,
        client_name: r.get(4)?,
        client_version: r.get(5)?,
        created_unix: r.get(6)?,
        last_seen_unix: r.get(7)?,
        revoked_unix: r.get(8)?,
    })
}

/// A live join code as `host.whoami` prints it.
#[derive(Debug, Clone)]
pub struct JoinCode {
    pub code_plain: String,
    pub max_uses: i64,
    pub uses: i64,
    pub expires_unix: i64,
}

/// What a successful join produced.
#[derive(Debug, Clone)]
pub struct Joined {
    pub client: ClientRow,
    pub siblings: i64,
}

/// Every tenant has >= 1 row (R1): client 0 is backfilled by the migration
/// for existing tenants and inserted here, idempotently, for newer ones.
fn ensure_first_sync(conn: &Connection, tenant_id: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO tenant_clients (id, tenant_id, secret_hash, label, created_unix) \
         VALUES (?1, ?2, NULL, 'first', COALESCE((SELECT created_unix FROM tenants WHERE id = ?2), ?3))",
        params![first_client_id(tenant_id), tenant_id, now],
    )?;
    Ok(())
}

impl Db {
    pub async fn ensure_first_client(&self, tenant_id: i64) -> Result<String, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            ensure_first_sync(conn, tenant_id, now)?;
            Ok(first_client_id(tenant_id))
        })
        .await
    }

    /// R3: the tenant's live join code, minted on first read. A code that is
    /// revoked or expired is replaced; one that is merely used up is still
    /// the live code (its `uses`/`max_uses` tell the story).
    pub async fn live_join_code(&self, tenant_id: i64) -> Result<JoinCode, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let out = (|| {
                let found = conn
                    .query_row(
                        "SELECT code_plain, max_uses, uses, expires_unix FROM join_codes \
                         WHERE tenant_id = ?1 AND revoked_unix IS NULL AND expires_unix > ?2 \
                         ORDER BY id DESC LIMIT 1",
                        params![tenant_id, now],
                        |r| {
                            Ok(JoinCode {
                                code_plain: r.get(0)?,
                                max_uses: r.get(1)?,
                                uses: r.get(2)?,
                                expires_unix: r.get(3)?,
                            })
                        },
                    )
                    .optional()?;
                match found {
                    Some(code) => Ok(code),
                    None => mint_join_code_sync(conn, tenant_id, now),
                }
            })();
            finish_txn(conn, out)
        })
        .await
    }

    /// Revoke every live join code of the tenant and mint a fresh one (R5:
    /// `host.key.rotate {all: true}` rotates the join code).
    pub async fn rotate_join_code(&self, tenant_id: i64) -> Result<JoinCode, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let out = (|| {
                conn.execute(
                    "UPDATE join_codes SET revoked_unix = ?1 WHERE tenant_id = ?2 AND revoked_unix IS NULL",
                    params![now, tenant_id],
                )?;
                mint_join_code_sync(conn, tenant_id, now)
            })();
            finish_txn(conn, out)
        })
        .await
    }

    /// Re-limit the tenant's live join code (operator / test seam; the
    /// default is [`DEFAULT_JOIN_MAX_USES`]).
    pub async fn set_join_code_max_uses(&self, tenant_id: i64, max_uses: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE join_codes SET max_uses = ?1 WHERE tenant_id = ?2 AND revoked_unix IS NULL",
                params![max_uses, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// `/j/{code}/mcp` connection-time check: revoked or expired only (an
    /// exhausted code is decided by [`Self::claim_join`], the one place that
    /// consumes a slot), same split as `invites`.
    pub async fn join_code_connectable(&self, code_hash: String) -> Result<bool, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT 1 FROM join_codes WHERE code_hash = ?1 AND revoked_unix IS NULL AND expires_unix > ?2",
                params![code_hash, now],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(AppError::from)
        })
        .await
    }

    /// R3: the whole join, atomically -- validate the code, consume a slot,
    /// insert the sibling with its own secret hash. One transaction, so ten
    /// concurrent first calls on `max_uses: 5` land exactly five rows (AC7).
    pub async fn claim_join(
        &self,
        code_hash: String,
        secret_hash: String,
        client_name: Option<String>,
        client_version: Option<String>,
    ) -> Result<(i64, Joined), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let out = (|| {
                let (code_id, tenant_id, max_uses, uses, expires, revoked): (i64, i64, i64, i64, i64, Option<i64>) = conn
                    .query_row(
                        "SELECT id, tenant_id, max_uses, uses, expires_unix, revoked_unix FROM join_codes \
                         WHERE code_hash = ?1",
                        params![code_hash],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                    )
                    .optional()?
                    .ok_or_else(AppError::join_code_invalid)?;
                let disabled: bool = conn
                    .query_row("SELECT disabled FROM tenants WHERE id = ?1", params![tenant_id], |r| {
                        r.get::<_, i64>(0).map(|d| d != 0)
                    })
                    .optional()?
                    .unwrap_or(true);
                if revoked.is_some() || expires <= now || uses >= max_uses || disabled {
                    return Err(AppError::join_code_invalid());
                }
                ensure_first_sync(conn, tenant_id, now)?;
                conn.execute("UPDATE join_codes SET uses = uses + 1 WHERE id = ?1", params![code_id])?;
                let existing: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM tenant_clients WHERE tenant_id = ?1",
                    params![tenant_id],
                    |r| r.get(0),
                )?;
                let label = derive_label(client_name.as_deref(), existing + 1);
                let id = crate::state::new_ulid();
                conn.execute(
                    "INSERT INTO tenant_clients (id, tenant_id, secret_hash, label, client_name, \
                     client_version, created_unix, last_seen_unix) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    params![id, tenant_id, secret_hash, label, client_name, client_version, now],
                )?;
                let sql = format!("SELECT {CLIENT_COLUMNS} FROM tenant_clients WHERE id = ?1");
                let client = conn.query_row(&sql, params![id], client_from_row)?;
                let siblings = live_count_sync(conn, tenant_id)?;
                Ok((tenant_id, Joined { client, siblings }))
            })();
            finish_txn(conn, out)
        })
        .await
    }

    /// `/u/{secret}/mcp` resolution for a sibling (R2): the client whose
    /// secret this is, live or revoked alike.
    pub async fn find_client_by_secret_hash(&self, secret_hash: String) -> Result<Option<ClientRow>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {CLIENT_COLUMNS} FROM tenant_clients WHERE secret_hash = ?1");
            conn.query_row(&sql, params![secret_hash], client_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    /// Whether `secret_hash` is a secret some client held before a rotation:
    /// that URL answers `tenant_key_invalid`, not an anonymous 404.
    pub async fn is_retired_client_secret(&self, secret_hash: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT 1 FROM tenant_clients WHERE retired_secret_hash = ?1",
                params![secret_hash],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(AppError::from)
        })
        .await
    }

    /// R6: remember that a browser consented to `tenant_id` (30 days).
    pub async fn insert_consent_cookie(&self, token_hash: String, tenant_id: i64, ttl_secs: i64) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO consent_cookies (token_hash, tenant_id, created_unix, expires_unix) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![token_hash, tenant_id, now, now + ttl_secs],
            )?;
            Ok(())
        })
        .await
    }

    /// The tenant a live consent cookie names, if any.
    pub async fn consent_cookie_tenant_id(&self, token_hash: String) -> Result<Option<i64>, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT tenant_id FROM consent_cookies WHERE token_hash = ?1 AND expires_unix > ?2",
                params![token_hash, now],
                |r| r.get(0),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// R6: a client row for a browser grant that chose "Join <workspace>" --
    /// no secret of its own (the grant's token is its credential).
    pub async fn add_oauth_client(&self, tenant_id: i64, client_name: Option<String>) -> Result<ClientRow, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let out = (|| {
                ensure_first_sync(conn, tenant_id, now)?;
                let existing: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM tenant_clients WHERE tenant_id = ?1",
                    params![tenant_id],
                    |r| r.get(0),
                )?;
                let label = derive_label(client_name.as_deref(), existing + 1);
                let id = crate::state::new_ulid();
                conn.execute(
                    "INSERT INTO tenant_clients (id, tenant_id, secret_hash, label, client_name, created_unix, \
                     last_seen_unix) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?5)",
                    params![id, tenant_id, label, client_name, now],
                )?;
                let sql = format!("SELECT {CLIENT_COLUMNS} FROM tenant_clients WHERE id = ?1");
                Ok(conn.query_row(&sql, params![id], client_from_row)?)
            })();
            finish_txn(conn, out)
        })
        .await
    }

    pub async fn client_is_revoked(&self, client_id: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT revoked_unix IS NOT NULL FROM tenant_clients WHERE id = ?1",
                params![client_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|r| r.unwrap_or(0) != 0)
            .map_err(AppError::from)
        })
        .await
    }

    pub async fn touch_client(&self, client_id: String) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenant_clients SET last_seen_unix = ?1 WHERE id = ?2",
                params![now, client_id],
            )?;
            Ok(())
        })
        .await
    }

    /// R4: non-revoked rows, oldest first. Ensures client 0 exists.
    pub async fn list_live_clients(&self, tenant_id: i64) -> Result<Vec<ClientRow>, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            ensure_first_sync(conn, tenant_id, now)?;
            let sql = format!(
                "SELECT {CLIENT_COLUMNS} FROM tenant_clients WHERE tenant_id = ?1 AND revoked_unix IS NULL \
                 ORDER BY created_unix, rowid"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![tenant_id], client_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    /// Whether the tenant has ever had a sibling (more than the one client 0
    /// row): decides whether a rotated client-0 URL leaves a tombstone.
    pub async fn tenant_has_siblings(&self, tenant_id: i64) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tenant_clients WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )?;
            Ok(n > 1)
        })
        .await
    }

    /// Client 0's retired-secret tombstone (the URL it held before a
    /// rotation).
    pub async fn retire_first_client_secret(&self, tenant_id: i64, old_secret_hash: String) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenant_clients SET retired_secret_hash = ?1 WHERE id = ?2",
                params![old_secret_hash, first_client_id(tenant_id)],
            )?;
            Ok(())
        })
        .await
    }

    /// R5: replace a sibling's secret, keeping the old one as a tombstone.
    pub async fn rotate_client_secret(&self, client_id: String, new_secret_hash: String) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenant_clients SET retired_secret_hash = secret_hash, secret_hash = ?1 WHERE id = ?2",
                params![new_secret_hash, client_id],
            )?;
            Ok(())
        })
        .await
    }

    /// R4: revoke one live client of `tenant_id`. The last live client is
    /// refused (`last_client`).
    pub async fn revoke_client(&self, tenant_id: i64, client_id: String) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let out = (|| {
                ensure_first_sync(conn, tenant_id, now)?;
                let live: Option<i64> = conn
                    .query_row(
                        "SELECT 1 FROM tenant_clients WHERE id = ?1 AND tenant_id = ?2 AND revoked_unix IS NULL",
                        params![client_id, tenant_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if live.is_none() {
                    return Err(AppError::client_not_found(&client_id));
                }
                if live_count_sync(conn, tenant_id)? <= 1 {
                    return Err(AppError::last_client());
                }
                conn.execute(
                    "UPDATE tenant_clients SET revoked_unix = ?1 WHERE id = ?2",
                    params![now, client_id],
                )?;
                Ok(())
            })();
            finish_txn(conn, out)
        })
        .await
    }

    /// `admin.funnel` / `/healthz`: client rows created in the window.
    pub async fn count_tenant_clients_since(&self, since_unix: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tenant_clients WHERE created_unix >= ?1 AND id NOT LIKE 'c0-%'",
                params![since_unix],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }
}

fn live_count_sync(conn: &Connection, tenant_id: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM tenant_clients WHERE tenant_id = ?1 AND revoked_unix IS NULL",
        params![tenant_id],
        |r| r.get(0),
    )
}

fn mint_join_code_sync(conn: &Connection, tenant_id: i64, now: i64) -> Result<JoinCode, AppError> {
    let code = generate_url_secret();
    let expires = now + JOIN_CODE_TTL_SECS;
    conn.execute(
        "INSERT INTO join_codes (code_hash, code_plain, tenant_id, max_uses, uses, expires_unix, created_unix) \
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
        params![hash_key(&code), code, tenant_id, DEFAULT_JOIN_MAX_USES, expires, now],
    )?;
    Ok(JoinCode { code_plain: code, max_uses: DEFAULT_JOIN_MAX_USES, uses: 0, expires_unix: expires })
}

fn finish_txn<T>(conn: &Connection, out: Result<T, AppError>) -> Result<T, AppError> {
    match out {
        Ok(v) => {
            conn.execute("COMMIT", [])?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(e)
        }
    }
}

/// R3: the join side effect for an anonymous first `host.*` call on
/// `/j/{code}/mcp`: consume a slot, mint the sibling's own `/u/` URL, bind
/// the session (tenant AND client), and return the tenant plus the
/// `onboarding` object the call's response carries.
pub async fn join_on_first_call(
    state: &AppState,
    code: &str,
    session_id: Option<&str>,
    client_info: Option<(String, String)>,
) -> Result<(Tenant, ClientRow, Value), AppError> {
    let secret = generate_url_secret();
    let (name, version) = match client_info {
        Some((n, v)) => (Some(n), Some(v)),
        None => (None, None),
    };
    let (tenant_id, joined) = state.db.claim_join(hash_key(code), hash_key(&secret), name, version).await?;
    let tenant = state
        .db
        .find_tenant_by_id(tenant_id)
        .await?
        .ok_or_else(AppError::join_code_invalid)?;
    if let Some(session_id) = session_id {
        let now = now_unix();
        state.session_bindings.bind(session_id, tenant.id, now);
        state.session_bindings.bind_client(session_id, tenant.id, &joined.client.id);
    }
    let onboarding = json!({
        "url": client_url(state, &secret),
        "joined": tenant.namespace,
        "client_id": joined.client.id,
        "label": joined.client.label,
        "siblings": joined.siblings,
        "text": format!(
            "you joined tenant {} as client {}; your own URL is the url field -- use it from now on",
            tenant.namespace, joined.client.label
        ),
    });
    Ok((tenant, joined.client, onboarding))
}

/// R4: `host.whoami`'s `client_id`, `clients` and `join_url`, for the
/// calling client `this_client` (`None` resolves to client 0).
pub async fn whoami_fields(
    state: &AppState,
    tenant: &Tenant,
    this_client: Option<&str>,
) -> Result<Value, AppError> {
    let this_id = match this_client {
        Some(id) => id.to_string(),
        None => state.db.ensure_first_client(tenant.id).await?,
    };
    let clients: Vec<Value> = state
        .db
        .list_live_clients(tenant.id)
        .await?
        .into_iter()
        .map(|c| {
            json!({
                "client_id": c.id,
                "label": c.label,
                "client_name": c.client_name,
                "last_seen": c.last_seen_unix,
                "this": c.id == this_id,
            })
        })
        .collect();
    let code = state.db.live_join_code(tenant.id).await?;
    Ok(json!({
        "client_id": this_id,
        "clients": clients,
        "join_url": join_url(state, &code.code_plain),
    }))
}

/// `host.client.revoke {client_id}` (R4). Revoking client 0 also retires the
/// tenant's key and `/u/` secret (replaced by unguessable values) so its old
/// credentials stop working without a per-call lookup on the hot path.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let client_id = args
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'client_id'".to_string()))?;
    state.db.revoke_client(tenant.id, client_id.clone()).await?;
    if client_id == first_client_id(tenant.id) {
        state.db.rotate_tenant_key(tenant.id, hash_key(&crate::auth::generate_key())).await?;
        if let Some(old) = &tenant.url_secret_hash {
            state.db.retire_first_client_secret(tenant.id, old.clone()).await?;
        }
        state
            .db
            .rotate_tenant_url_secret(tenant.id, hash_key(&generate_url_secret()))
            .await?;
    }
    let remaining = state.db.list_live_clients(tenant.id).await?.len();
    Ok(json!({"revoked": client_id, "clients_remaining": remaining}))
}

/// `host.key.rotate` for a sibling or with `all: true` (R5).
pub async fn rotate(
    state: &AppState,
    tenant: &Tenant,
    this_client: &str,
    all: bool,
) -> Result<Value, AppError> {
    let first = first_client_id(tenant.id);
    let mut changed: Vec<Value> = Vec::new();
    let mut out = json!({"tenant": tenant.namespace});
    let siblings_exist = state.db.tenant_has_siblings(tenant.id).await?;

    let rotate_first = all || this_client == first;
    if rotate_first {
        let key = crate::auth::generate_key();
        state.db.rotate_tenant_key(tenant.id, hash_key(&key)).await?;
        let secret = generate_url_secret();
        if siblings_exist && let Some(old) = &tenant.url_secret_hash {
            state.db.retire_first_client_secret(tenant.id, old.clone()).await?;
        }
        state.db.rotate_tenant_url_secret(tenant.id, hash_key(&secret)).await?;
        let url = client_url(state, &secret);
        changed.push(json!({"client_id": first, "key": true, "url": url}));
        if let Some(obj) = out.as_object_mut() {
            obj.insert("key".into(), json!(key));
            obj.insert("url".into(), json!(url));
        }
    }
    let sibling_ids: Vec<String> = if all {
        state
            .db
            .list_live_clients(tenant.id)
            .await?
            .into_iter()
            .filter(|c| c.id != first)
            .map(|c| c.id)
            .collect()
    } else if this_client != first {
        vec![this_client.to_string()]
    } else {
        Vec::new()
    };
    for id in sibling_ids {
        let secret = generate_url_secret();
        state.db.rotate_client_secret(id.clone(), hash_key(&secret)).await?;
        let url = client_url(state, &secret);
        if !rotate_first && let Some(obj) = out.as_object_mut() {
            obj.insert("url".into(), json!(url));
        }
        changed.push(json!({"client_id": id, "url": url}));
    }
    if all {
        let code = state.db.rotate_join_code(tenant.id).await?;
        let url = join_url(state, &code.code_plain);
        changed.push(json!({"join_url": url}));
        if let Some(obj) = out.as_object_mut() {
            obj.insert("join_url".into(), json!(url));
        }
    }
    if let Some(obj) = out.as_object_mut() {
        obj.insert("client_id".into(), json!(this_client));
        obj.insert("changed".into(), json!(changed));
        obj.insert(
            "usage".into(),
            json!(
                "Each changed client's old URL now answers tenant_key_invalid; use the new url in \
                 changed (or the url field for your own client) from this call on. Without all: true \
                 only the calling client's credential changed."
            ),
        );
    }
    Ok(out)
}
