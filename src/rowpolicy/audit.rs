//! The hash-chained audit record -- requirement 6/7 (AC7/AC8). Ported
//! (shape) from ai-stack's `audit.rs` `RetrievalAuditRecord`/`AuditChain`;
//! see `policy.rs`'s module doc for the port-scope note.

use serde_json::json;

/// technical considerations: "Chain per tenant. Genesis hash constant per
/// tenant chain." -- a fixed seed (not computed per tenant) every tenant's
/// first audit record links `prior_record_hash` to.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// requirement 6: `RetrievalAuditRecord {subject, tenant_id, plane,
/// policy_hash, applied, returned_count, withheld_count, timestamp,
/// request_id, prior_record_hash}`, plus the `id` and `record_hash` its
/// storage row carries.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalAuditRecord {
    pub id: i64,
    pub subject: String,
    pub tenant_id: i64,
    pub plane: String,
    pub policy_hash: String,
    pub applied: String,
    pub returned_count: i64,
    pub withheld_count: Option<i64>,
    pub timestamp: i64,
    pub request_id: String,
    pub prior_record_hash: String,
    pub record_hash: String,
}

impl From<crate::db::AuditRecordRow> for RetrievalAuditRecord {
    fn from(r: crate::db::AuditRecordRow) -> Self {
        RetrievalAuditRecord {
            id: r.id,
            subject: r.subject,
            tenant_id: r.tenant_id,
            plane: r.plane,
            policy_hash: r.policy_hash,
            applied: r.applied,
            returned_count: r.returned_count,
            withheld_count: r.withheld_count,
            timestamp: r.timestamp_unix,
            request_id: r.request_id,
            prior_record_hash: r.prior_record_hash,
            record_hash: r.record_hash,
        }
    }
}

/// requirement 6: the hash one record commits to -- every field except its
/// own `record_hash`. Altering any stored field, including `applied`
/// (AC7's tamper case), changes this on recompute.
#[allow(clippy::too_many_arguments)]
pub fn compute_record_hash(
    subject: &str,
    tenant_id: i64,
    plane: &str,
    policy_hash: &str,
    applied: &str,
    returned_count: i64,
    withheld_count: Option<i64>,
    timestamp: i64,
    request_id: &str,
    prior_record_hash: &str,
) -> String {
    let canonical = json!({
        "subject": subject,
        "tenant_id": tenant_id,
        "plane": plane,
        "policy_hash": policy_hash,
        "applied": applied,
        "returned_count": returned_count,
        "withheld_count": withheld_count,
        "timestamp": timestamp,
        "request_id": request_id,
        "prior_record_hash": prior_record_hash,
    });
    crate::billing::sha256_hex(
        &serde_json::to_vec(&canonical).expect("record hash input is always serializable"),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainVerifyResult {
    pub intact: bool,
    pub first_broken_id: Option<i64>,
}

/// requirement 7 (AC7): recomputes each record's hash from its own stored
/// fields, comparing against what's stored, and (from the second record
/// onward) checks that its `prior_record_hash` matches the previous
/// record's stored `record_hash` -- either mismatch reports that record's
/// id as the first broken one. `records` must be ordered oldest-first.
pub fn verify_chain(records: &[RetrievalAuditRecord]) -> ChainVerifyResult {
    let mut prior: Option<String> = None;
    for record in records {
        if let Some(expected_prior) = &prior
            && record.prior_record_hash != *expected_prior
        {
            return ChainVerifyResult { intact: false, first_broken_id: Some(record.id) };
        }
        let recomputed = compute_record_hash(
            &record.subject,
            record.tenant_id,
            &record.plane,
            &record.policy_hash,
            &record.applied,
            record.returned_count,
            record.withheld_count,
            record.timestamp,
            &record.request_id,
            &record.prior_record_hash,
        );
        if recomputed != record.record_hash {
            return ChainVerifyResult { intact: false, first_broken_id: Some(record.id) };
        }
        prior = Some(record.record_hash.clone());
    }
    ChainVerifyResult { intact: true, first_broken_id: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: i64, applied: &str, prior: &str) -> RetrievalAuditRecord {
        let record_hash = compute_record_hash("alice", 1, "sql", "hash1", applied, 5, Some(0), 1000, "req1", prior);
        RetrievalAuditRecord {
            id,
            subject: "alice".to_string(),
            tenant_id: 1,
            plane: "sql".to_string(),
            policy_hash: "hash1".to_string(),
            applied: applied.to_string(),
            returned_count: 5,
            withheld_count: Some(0),
            timestamp: 1000,
            request_id: "req1".to_string(),
            prior_record_hash: prior.to_string(),
            record_hash,
        }
    }

    #[test]
    fn an_intact_chain_verifies() {
        let r1 = record(1, "region = 'EU'", GENESIS_HASH);
        let r2 = record(2, "region = 'EU'", &r1.record_hash);
        let result = verify_chain(&[r1, r2]);
        assert!(result.intact);
        assert_eq!(result.first_broken_id, None);
    }

    #[test]
    fn tampering_with_applied_breaks_verification_at_that_record() {
        let r1 = record(1, "region = 'EU'", GENESIS_HASH);
        let mut r2 = record(2, "region = 'EU'", &r1.record_hash);
        r2.applied = "1 = 1".to_string();
        let result = verify_chain(&[r1, r2]);
        assert!(!result.intact);
        assert_eq!(result.first_broken_id, Some(2));
    }
}
