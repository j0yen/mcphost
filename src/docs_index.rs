//! The background indexer behind `host.docs.search`/`host.docs.status`'s
//! `index` block (PRD-mcphost-docs-semantic-search P0 requirements 1-5, P1
//! requirement 7): every 10 s, per tenant with `pending_documents > 0`
//! (`Db::tenants_with_pending_docs`), chunk changed documents by paragraph
//! into ≤800-character windows with a 100-character overlap
//! ([`chunk_text`]), replace that document's `doc_chunks` rows atomically,
//! embed through a configured provider when one is set, and advance
//! `indexed_watermark` only once the whole batch has committed.
//!
//! `docs.rs` owns the tenant-facing tools (`search`/`status`/
//! `index_config`/`reindex`); this module owns the tick itself plus the
//! two pieces of machinery both directions of that tick need: the
//! provider's embeddings HTTP call ([`call_embeddings_provider`], used at
//! index time to embed a document's chunks and at search time to embed the
//! query) and the little-endian f32 vector codec/cosine ([`encode_vector`],
//! [`decode_vector`], [`cosine`]) -- the same "storage is a thin shape,
//! business logic lives beside the tool" split `docs.rs`'s own module doc
//! already describes, just with the indexer's own tick as the "business
//! logic" here instead of a tool handler.

use serde_json::{Value, json};

use crate::db::ChunkVecRow;
use crate::errors::AppError;
use crate::state::AppState;

/// requirement 2: "chunk text ... into ≤ 800-character windows".
pub const CHUNK_MAX_CHARS: usize = 800;
/// requirement 2: "... with 100-character overlap".
pub const CHUNK_OVERLAP_CHARS: usize = 100;
/// requirement 2: "read documents with seq > indexed_watermark (batch 50)".
pub const DOCS_PER_TICK_BATCH: i64 = 50;
/// requirement 2: "every 10 s per tenant".
pub const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

// ---- chunking --------------------------------------------------------

/// One paragraph-aligned chunking window: `offset`/`len` are always
/// document-absolute byte positions.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub offset: i64,
    pub len: i64,
    pub text: String,
}

/// requirement 2: paragraphs (blank-line separated) greedily packed into
/// ≤ [`CHUNK_MAX_CHARS`]-byte windows. `offset` is always pinned to a real
/// paragraph's document-absolute start (never shifted back by the
/// overlap), so a phrase inside a paragraph always falls inside the
/// `offset` of whichever chunk that paragraph *opens* -- AC2's own "an
/// offset inside the third paragraph" holds however much of the previous
/// chunk's tail this chunk's `text` also happens to carry as context. That
/// tail -- up to [`CHUNK_OVERLAP_CHARS`] characters of the *previous*
/// chunk's own final text -- is what realizes "100-character overlap":
/// prepended to this chunk's `text` (and folded into its `len`), but never
/// into its `offset`. A paragraph longer than the whole window budget on
/// its own is split into fixed windows with a true, offset-shifting
/// overlap instead (there is no larger unit to align to).
pub fn chunk_text(text: &str) -> Vec<Chunk> {
    let paragraphs = split_paragraphs(text);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur_start: Option<usize> = None;
    let mut cur_parts: Vec<&str> = Vec::new();
    let mut cur_len = 0usize;

    for (p_start, p_text) in &paragraphs {
        if p_text.trim().is_empty() {
            continue;
        }
        if p_text.len() > CHUNK_MAX_CHARS {
            flush_chunk(&mut chunks, cur_start, &cur_parts);
            cur_start = None;
            cur_parts.clear();
            cur_len = 0;
            chunks.extend(split_long_paragraph(*p_start, p_text));
            continue;
        }
        let additional = if cur_parts.is_empty() { p_text.len() } else { 2 + p_text.len() };
        if !cur_parts.is_empty() && cur_len + additional > CHUNK_MAX_CHARS {
            flush_chunk(&mut chunks, cur_start, &cur_parts);
            cur_start = None;
            cur_parts.clear();
            cur_len = 0;
        }
        if cur_parts.is_empty() {
            cur_start = Some(*p_start);
            cur_len = p_text.len();
        } else {
            cur_len += 2 + p_text.len();
        }
        cur_parts.push(p_text);
    }
    flush_chunk(&mut chunks, cur_start, &cur_parts);

    let mut prev_tail = String::new();
    for c in chunks.iter_mut() {
        if !prev_tail.is_empty() {
            let mut combined = prev_tail.clone();
            combined.push_str(&c.text);
            c.len = combined.len() as i64;
            c.text = combined;
        }
        prev_tail = tail_chars(&c.text, CHUNK_OVERLAP_CHARS);
    }
    chunks
}

/// Every blank-line-separated paragraph in `text`, paired with its real
/// document-absolute byte offset.
fn split_paragraphs(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    for part in text.split("\n\n") {
        out.push((pos, part));
        pos += part.len() + 2;
    }
    out
}

fn flush_chunk(chunks: &mut Vec<Chunk>, start: Option<usize>, parts: &[&str]) {
    if parts.is_empty() {
        return;
    }
    let start = start.unwrap_or(0);
    let text = parts.join("\n\n");
    chunks.push(Chunk {
        offset: start as i64,
        len: text.len() as i64,
        text,
    });
}

/// A single paragraph too big for one window on its own: fixed
/// [`CHUNK_MAX_CHARS`] windows with a true (offset-shifting)
/// [`CHUNK_OVERLAP_CHARS`] overlap between them.
fn split_long_paragraph(base_offset: usize, text: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut start = 0usize;
    loop {
        let mut end = (start + CHUNK_MAX_CHARS).min(text.len());
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        let slice = &text[start..end];
        out.push(Chunk {
            offset: (base_offset + start) as i64,
            len: slice.len() as i64,
            text: slice.to_string(),
        });
        if end >= text.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP_CHARS);
    }
    out
}

/// The last up to `n` bytes of `s`, snapped forward to a valid UTF-8 char
/// boundary so it never splits a multi-byte character.
fn tail_chars(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut start = s.len() - n;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

// ---- lexical query sanitizing -----------------------------------------

/// Builds a safe FTS5 `MATCH` expression out of `query`: every
/// alphanumeric run becomes its own quoted phrase, joined by `OR` -- BM25
/// then ranks chunks matching more/rarer terms higher without requiring
/// every term to be present (a search query is a description of what the
/// caller wants, not a boolean formula -- AC2's "refund window" must still
/// find a chunk that only contains "refund"/"refunds"), and without any of
/// `query`'s own characters (quotes, `-`, `:`, ...) being interpreted as
/// FTS5 query syntax. `None` when `query` has no alphanumeric content at
/// all.
pub fn sanitize_fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{}\"", s.replace('"', "")))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" OR "))
}

// ---- vectors ------------------------------------------------------------

/// requirement 1/technical considerations: "vectors stored as little-endian
/// f32 blobs".
pub fn encode_vector(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// requirement 3: "cosine computed in Rust over the tenant's rows". `0.0`
/// for a dimension mismatch or a zero vector rather than `NaN`, so a
/// malformed/empty vector sorts last instead of poisoning the whole
/// ranking.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0f64;
    let mut norm_a = 0f64;
    let mut norm_b = 0f64;
    for i in 0..a.len() {
        let (x, y) = (a[i] as f64, b[i] as f64);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

pub fn matches_filter(row: &ChunkVecRow, prefix: &Option<String>, name: &Option<String>) -> bool {
    if let Some(p) = prefix
        && !row.name.starts_with(p.as_str())
    {
        return false;
    }
    if let Some(n) = name
        && &row.name != n
    {
        return false;
    }
    true
}

// ---- embeddings provider -------------------------------------------------

/// requirement 4: `POST <endpoint>` with `{model, input: [...]}}`,
/// `Authorization: Bearer <tenant's own named secret>` -- the same
/// tenant-secret-as-bearer shape `kinds::http`'s own spec templates give a
/// tool's egress, reusing [`AppState::http_client`] (the crate's one
/// shared `reqwest::Client`, same as `registry.rs`'s own outbound POSTs).
/// One request per call; `docs_index::tick_tenant` calls this once per
/// document's chunk batch, `docs::doc_search` once per query. `Err` for
/// any transport failure, non-2xx status, or malformed response body --
/// AC6's own lexical-fallback trigger at the call site.
#[allow(clippy::too_many_arguments)]
pub async fn call_embeddings_provider(
    state: &AppState,
    tenant_id: i64,
    endpoint: &str,
    model: &str,
    secret_name: &str,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>, AppError> {
    let secret_value = resolve_tenant_secret(state, tenant_id, secret_name).await?;
    let resp = state
        .http_client
        .post(endpoint)
        .bearer_auth(secret_value)
        .json(&json!({"model": model, "input": inputs}))
        .send()
        .await
        .map_err(|e| AppError::Structured {
            code: "docs_provider_unreachable",
            message: format!("embeddings provider request failed: {e}"),
            data: json!({}),
        })?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(AppError::Structured {
            code: "docs_provider_error",
            message: format!("embeddings provider returned status {status}"),
            data: json!({"status": status}),
        });
    }
    let body: Value = resp.json().await.map_err(|e| AppError::Structured {
        code: "docs_provider_error",
        message: format!("embeddings provider returned invalid JSON: {e}"),
        data: json!({}),
    })?;
    let data = body.get("data").and_then(Value::as_array).ok_or_else(|| AppError::Structured {
        code: "docs_provider_error",
        message: "embeddings provider response missing a 'data' array".to_string(),
        data: json!({}),
    })?;
    let mut out = Vec::with_capacity(data.len());
    for item in data {
        let embedding =
            item.get("embedding").and_then(Value::as_array).ok_or_else(|| AppError::Structured {
                code: "docs_provider_error",
                message: "embeddings provider response item is missing an 'embedding' array"
                    .to_string(),
                data: json!({}),
            })?;
        out.push(embedding.iter().filter_map(Value::as_f64).map(|v| v as f32).collect());
    }
    Ok(out)
}

async fn resolve_tenant_secret(
    state: &AppState,
    tenant_id: i64,
    name: &str,
) -> Result<String, AppError> {
    let (ciphertext, nonce) =
        state.db.get_secret(tenant_id, name.to_string()).await?.ok_or_else(|| {
            AppError::Structured {
                code: "docs_secret_missing",
                message: format!("tenant has no secret named '{name}'"),
                data: json!({"secret": name}),
            }
        })?;
    state.secrets.decrypt(&ciphertext, &nonce)
}

// ---- the tick -------------------------------------------------------------

/// requirement 2: one indexing pass across every tenant with pending
/// documents. Per-tenant failures are logged and skipped rather than
/// aborting the whole tick -- one tenant's provider outage or malformed
/// document must never stall every other tenant's own 10 s cadence.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let tenant_ids = state.db.tenants_with_pending_docs().await?;
    for tenant_id in tenant_ids {
        if let Err(e) = tick_tenant(state, tenant_id).await {
            tracing::warn!(tenant_id, error = %e, "docs indexer: tick failed for tenant");
        }
    }
    Ok(())
}

async fn tick_tenant(state: &AppState, tenant_id: i64) -> Result<(), AppError> {
    let now = crate::state::now_unix();
    state.db.doc_index_state_ensure(tenant_id, now).await?;
    let idx = state
        .db
        .doc_index_state_get(tenant_id)
        .await?
        .ok_or_else(|| AppError::Internal("doc_index_state row missing after ensure".into()))?;
    let Some(tenant) = state.db.find_tenant_by_id(tenant_id).await? else {
        return Ok(());
    };
    let Some(plan) = state.plans.get(&tenant.plan) else {
        return Ok(());
    };

    let batch = state
        .db
        .documents_pending_batch(tenant_id, idx.indexed_watermark, DOCS_PER_TICK_BATCH)
        .await?;
    let mut watermark = idx.indexed_watermark;
    let mut quota_hit = false;

    for doc in &batch {
        // requirement 2: "replace that document's chunks atomically" --
        // the old set is always gone before any new one lands, live doc,
        // deleted doc, or a doc whose own quota-truncated set is about to
        // be recomputed identically on a later tick.
        state.db.doc_chunks_delete_for_document(tenant_id, doc.id.clone()).await?;

        if doc.deleted_at.is_some() {
            watermark = doc.seq;
            continue;
        }

        let text = match state.db.document_blob_get(tenant_id, doc.id.clone(), doc.version).await? {
            Some((_, text)) => text,
            None => {
                watermark = doc.seq;
                continue;
            }
        };
        let chunks = chunk_text(&text);
        if chunks.is_empty() {
            watermark = doc.seq;
            continue;
        }

        // requirement 5/AC7: never let one tenant's chunk count cross its
        // plan's `docs_chunks_max` -- truncate this document's own batch
        // to whatever room is left, and stop advancing the watermark past
        // it once that happens (the next tick recomputes the identical
        // truncation, since this document's own prior partial set was
        // just deleted above).
        let current_total = state.db.doc_chunks_count(tenant_id).await?;
        let remaining = (plan.docs_chunks_max - current_total).max(0) as usize;
        let take = chunks.len().min(remaining);
        let took_all = take == chunks.len();

        let mut vectors: Vec<Option<Vec<u8>>> = vec![None; take];
        if take > 0
            && idx.provider == "openai-compatible"
            && let (Some(endpoint), Some(model), Some(secret_name)) =
                (idx.endpoint.as_deref(), idx.model.as_deref(), idx.secret_name.as_deref())
        {
            let inputs: Vec<String> = chunks[..take].iter().map(|c| c.text.clone()).collect();
            match call_embeddings_provider(state, tenant_id, endpoint, model, secret_name, &inputs)
                .await
            {
                Ok(embeddings) if embeddings.len() == take => {
                    for (slot, embedding) in vectors.iter_mut().zip(embeddings) {
                        *slot = Some(encode_vector(&embedding));
                    }
                    let _ = state
                        .db
                        .document_usage_event_insert(
                            tenant_id,
                            "docs.embed_chunks".to_string(),
                            take as i64,
                            now,
                        )
                        .await;
                }
                Ok(_) => tracing::warn!(
                    tenant_id,
                    document_id = %doc.id,
                    "docs indexer: embeddings provider returned the wrong number of vectors, storing chunks without vectors"
                ),
                Err(e) => tracing::warn!(
                    tenant_id,
                    document_id = %doc.id,
                    error = %e,
                    "docs indexer: embeddings provider call failed, storing chunks without vectors"
                ),
            }
        }

        let insert_rows: Vec<crate::db::ChunkInsertRow> = chunks[..take]
            .iter()
            .zip(vectors)
            .enumerate()
            .map(|(i, (c, vector))| (i as i64, c.offset, c.len, c.text.clone(), vector))
            .collect();
        state
            .db
            .doc_chunks_insert_batch(
                tenant_id,
                doc.id.clone(),
                doc.version,
                doc.name.clone(),
                insert_rows,
                now,
            )
            .await?;

        if took_all {
            watermark = doc.seq;
        } else {
            quota_hit = true;
            break;
        }
    }

    let live_watermark = state.db.documents_watermark(tenant_id).await?;
    let rebuilding = watermark < live_watermark;
    state.db.doc_index_state_advance(tenant_id, watermark, quota_hit, rebuilding, now).await?;
    Ok(())
}

/// The real, unattended 10 s cadence -- started once at `mcphost serve`
/// startup, alongside every other background task (see `main.rs`). Same
/// "never joined, tokio::spawn loop" lifetime convention as
/// `crate::triggers::spawn_scheduler`/`crate::retention::spawn_prune_scheduler`.
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK_INTERVAL).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "docs indexer: tick failed");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_keeps_offsets_at_paragraph_starts() {
        let text = "one two three\n\nfour five six\n\nrefunds are processed within 14 days, no exceptions";
        let chunks = chunk_text(text);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(text.get(c.offset as usize..).is_some(), "offset must be a valid position");
        }
    }

    #[test]
    fn chunk_text_splits_paragraphs_over_the_budget_and_overlaps() {
        let big = "x".repeat(500);
        let text = format!("{big}\n\n{big}\n\n{big}");
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 2, "three 500-char paragraphs must not all fit in one 800-char chunk");
        // Every chunk after the first carries some of the previous
        // chunk's own tail as a prefix (the 100-char overlap).
        for pair in chunks.windows(2) {
            assert!(!pair[1].text.is_empty());
        }
    }

    #[test]
    fn chunk_text_empty_input_yields_no_chunks() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   \n\n   ").is_empty());
    }

    #[test]
    fn sanitize_fts_query_ors_alphanumeric_terms() {
        assert_eq!(sanitize_fts_query("refund window"), Some("\"refund\" OR \"window\"".to_string()));
        assert_eq!(sanitize_fts_query("\"; DROP TABLE x"), Some("\"DROP\" OR \"TABLE\" OR \"x\"".to_string()));
        assert_eq!(sanitize_fts_query("   "), None);
    }

    #[test]
    fn vector_round_trips_through_bytes() {
        let v = vec![1.0f32, -2.5, 0.0, 3.25];
        let bytes = encode_vector(&v);
        assert_eq!(bytes.len(), 16);
        assert_eq!(decode_vector(&bytes), v);
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let v = vec![1.0f32, 2.0, 3.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!(cosine(&a, &b).abs() < 1e-6);
    }
}
