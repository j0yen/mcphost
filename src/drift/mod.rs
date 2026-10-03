//! PRD-mcphost-drift-review: when a table note, a table's inferred schema,
//! or a document changes, re-run the logged questions that depended on it
//! and show what moved. Ported from ~/projects/ai-stack @ 4ef6c22
//! aistack-observability's `store.rs` (`grounding_records`/
//! `definition_versions`/`drift_queue`/`emitted_reviews`), `drift.rs`
//! (`DriftDetector`/`QuestionDelta`), `review.rs` (`DriftReviewItem`), and
//! `alert.rs` (`AlertEvent`) -- `eval.rs` (the external-evaluator stub) is
//! deliberately not ported; [`rerun`] replaces it with deterministic
//! re-execution of the stored SQL or search instead of a model judgment.
//!
//! - [`versions`]: the three hooks (`host.table.model_set`, the table-model
//!   tick, `doc_put`) that record a [`crate::db::Db::insert_context_version`]
//!   row and enqueue the affected questions for re-run (requirement 1/3).
//! - [`queue`]: the enqueue/dedupe helper [`versions`] calls, plus
//!   `host.drift.check` (P1 requirement 8/AC10), the manual-trigger
//!   counterpart that enqueues a re-run with no version change.
//! - [`rerun`]: the queue-draining tick (requirement 3/4) -- dependency
//!   lookup, re-execution, delta computation, and review assembly.
//! - [`alert`]: requirement 6's `AlertEvent`, raised when a review's
//!   `regressed_count > 0`.
//! - [`review`]: `host.drift.reviews`/`.review`/`.resolve` (requirement 7).

pub mod alert;
pub mod queue;
pub mod rerun;
pub mod review;
pub mod versions;

pub use queue::check;
pub use review::{resolve, review as review_item, reviews};
