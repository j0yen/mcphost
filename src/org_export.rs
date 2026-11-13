//! PRD-mcphost-org-export-and-offboard-v2 R1/R2/R7: `host.org.export` -- the
//! owner takes the whole team's data in one signed bundle. One job on the
//! `runs` ledger (its own `runs.tool_name`, [`crate::export::ORG_EXPORT_TOOL_NAME`])
//! runs the per-tenant archive builder once per live member, one member in
//! memory at a time, and nests each member's own archive as
//! `members/<namespace>.tar.gz` behind an `org_manifest.json`. The builder
//! takes a member's `Tenant` row and reads by tenant id; this module never
//! reads a member's credentials, and the one write-and-sign step is the
//! owner's, through `export::store_and_sign`.

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use tar::Builder;

use crate::billing::sha256_hex;
use crate::db::{StartExportRun, Tenant};
use crate::errors::AppError;
use crate::export::{EXPORT_DIR, ORG_EXPORT_TOOL_NAME};
use crate::state::AppState;

/// `org_manifest.json`'s `schema_version`.
pub const ORG_MANIFEST_VERSION: i64 = 1;

/// What one org export job works on.
struct Job<'a> {
    state: &'a AppState,
    owner: &'a Tenant,
    run_id: &'a str,
    org_id: i64,
    org_name: &'a str,
}

/// `host.org.export`: owner only. Starts (or, like `host.export`, rejoins)
/// the background job and returns `{run_id, status: running}`.
pub async fn export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let (org_id, org_name) = crate::org::owned_org(state, tenant).await?;
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let run_id = crate::state::new_ulid();
    let args_json = serde_json::to_string(args)
        .map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    let outcome = state
        .db
        .start_export_run(
            tenant.id,
            run_id.clone(),
            ORG_EXPORT_TOOL_NAME.to_string(),
            plan.job_max_s,
            args_json,
        )
        .await?;
    let run_id = match outcome {
        StartExportRun::AlreadyRunning(existing) => {
            return Ok(json!({"run_id": existing, "status": "running"}));
        }
        StartExportRun::Started(new_id) => new_id,
    };
    let spawn_state = state.clone();
    let spawn_owner = tenant.clone();
    let spawn_run_id = run_id.clone();
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let job = Job { state: &spawn_state, owner: &spawn_owner, run_id: &spawn_run_id, org_id, org_name: &org_name };
        let outcome = build_bundle(&job).await;
        crate::export::finish_run(&spawn_state, &spawn_owner, spawn_run_id.clone(), start, outcome).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

/// The `org_manifest.json` row of one member's finished part.
fn member_row(
    member: &crate::org::LiveMember,
    disabled_reason: Option<&str>,
    manifest_tools: &[Value],
    part_bytes: &[u8],
) -> Value {
    let tools: Vec<Value> = manifest_tools
        .iter()
        .map(|t| {
            let spec = serde_json::to_vec(&t["spec"]).unwrap_or_default();
            json!({"name": t["name"], "kind": t["kind"], "spec_sha256": sha256_hex(&spec)})
        })
        .collect();
    let mut row = json!({
        "namespace": member.namespace,
        "role": member.role,
        "joined_unix": member.joined_unix,
        "tools": tools,
        "part": format!("members/{}.tar.gz", member.namespace),
        "sha256": sha256_hex(part_bytes),
        "bytes": part_bytes.len(),
    });
    if let Some(reason) = disabled_reason {
        row["disabled_reason"] = json!(reason);
    }
    row
}

/// Builds every live member's part into a spool directory (one member in
/// memory at a time), writes the bundle with `org_manifest.json` first, and
/// stores and signs it for the owner. The spool directory is removed on
/// every path out.
async fn build_bundle(job: &Job<'_>) -> Result<Value, AppError> {
    let plan = job.state.plans.get(&job.owner.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            job.owner.plan
        ))
    })?;
    let spool = job.state.db.data_dir().join(EXPORT_DIR).join(format!("{}.parts", job.run_id));
    tokio::fs::create_dir_all(&spool)
        .await
        .map_err(|e| AppError::Storage(format!("create org export spool: {e}")))?;
    let outcome = assemble(job, plan.export_bytes_max, &spool).await;
    let _ = tokio::fs::remove_dir_all(&spool).await;
    outcome
}

async fn assemble(job: &Job<'_>, limit: i64, spool: &std::path::Path) -> Result<Value, AppError> {
    let Job { state, owner, run_id, org_id, org_name } = *job;
    // The member list is read once, here: a member who joins later is not in this bundle.
    let members = state.db.org_live_members(org_id).await?;
    let mut rows: Vec<Value> = Vec::with_capacity(members.len());
    let mut parts: Vec<(String, std::path::PathBuf)> = Vec::with_capacity(members.len());
    let mut total: usize = 0;
    for member in &members {
        let Some(row) = state.db.find_tenant_by_id(member.tenant_id).await? else {
            continue;
        };
        let archive = crate::export::build_tenant_archive(state, &row, None).await?;
        total += archive.bytes.len();
        if total as i64 > limit {
            return Err(crate::export::export_too_large_error(total, limit));
        }
        let path = spool.join(format!("{}.tar.gz", member.namespace));
        tokio::fs::write(&path, &archive.bytes)
            .await
            .map_err(|e| AppError::Storage(format!("spool org export part: {e}")))?;
        rows.push(member_row(member, row.disabled_reason.as_deref(), &archive.manifest, &archive.bytes));
        parts.push((format!("members/{}.tar.gz", member.namespace), path));
    }

    let org_manifest = json!({
        "schema_version": ORG_MANIFEST_VERSION,
        "org": org_name,
        "exported_unix": crate::state::now_unix(),
        "members": rows,
    });
    let manifest_bytes = crate::export::to_json_bytes(&org_manifest)?;

    let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    crate::export::add_entry(&mut builder, "org_manifest.json", &manifest_bytes)?;
    for (name, path) in &parts {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| AppError::Storage(format!("read org export part: {e}")))?;
        crate::export::add_entry(&mut builder, name, &bytes)?;
    }
    let bundle = builder
        .into_inner()
        .map_err(|e| AppError::Internal(format!("tar build: {e}")))?
        .finish()
        .map_err(|e| AppError::Internal(format!("gzip finish: {e}")))?;

    let mut result = crate::export::store_and_sign(state, owner, run_id, &bundle).await?;
    result["org_manifest"] = org_manifest;
    Ok(result)
}
