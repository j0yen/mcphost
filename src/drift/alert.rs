//! PRD-mcphost-drift-review requirement 6: `AlertEvent {kind:
//! drift_regression, severity, detail}` -- ported from ~/projects/ai-stack
//! @ 4ef6c22 aistack-observability's `alert.rs`. Raised once per review
//! whose `regressed_count > 0`, severity `critical` at `regressed_count >=
//! 5` (per the requirement's own text), `warning` otherwise.

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::AppState;

const CRITICAL_AT_REGRESSED_COUNT: i64 = 5;

/// requirement 6: writes a `drift_regression` row to `drift_alerts` when
/// `regressed_count > 0`; a no-op (no row written) otherwise -- AC2's "no
/// alert is written" for a cosmetic (zero-regression) change.
pub(crate) async fn raise_if_regressed(
    state: &AppState,
    tenant_id: i64,
    item_id: &str,
    target: &str,
    regressed_count: i64,
) -> Result<(), AppError> {
    if regressed_count <= 0 {
        return Ok(());
    }
    let severity = if regressed_count >= CRITICAL_AT_REGRESSED_COUNT {
        "critical"
    } else {
        "warning"
    };
    let detail: Value = json!({"target": target, "regressed_count": regressed_count});
    state
        .db
        .insert_drift_alert(
            tenant_id,
            item_id.to_string(),
            "drift_regression".to_string(),
            severity.to_string(),
            detail.to_string(),
        )
        .await
}
