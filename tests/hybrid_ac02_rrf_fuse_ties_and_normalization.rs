//! PRD-mcphost-docs-hybrid-search
//! AC2 -- Given the fusion function with two lists, When fused at `k_rrf`
//! 60, Then a chunk present in both lists outranks a chunk at the same
//! best rank in only one, and the top score is 1.0.

use mcphost::docs_index::{K_RRF, rrf_fuse};

#[test]
fn present_in_both_lists_at_rank1_outranks_rank1_in_only_one_list() {
    // "both" is rank 1 in both lists; "dense_only" is rank 1 in dense
    // alone; "sparse_only" is rank 1 in sparse alone.
    let dense = vec![("both".to_string(), 0.9), ("dense_only".to_string(), 0.8)];
    let sparse = vec![("both".to_string(), 5.0), ("sparse_only".to_string(), 4.0)];

    let fused = rrf_fuse(&dense, &sparse, K_RRF);
    let score_of = |id: &str| fused.iter().find(|(i, _)| i == id).map(|(_, s)| *s).unwrap();

    assert!(
        score_of("both") > score_of("dense_only"),
        "a chunk in both lists must outrank one only in dense at the same rank: {fused:?}"
    );
    assert!(
        score_of("both") > score_of("sparse_only"),
        "a chunk in both lists must outrank one only in sparse at the same rank: {fused:?}"
    );

    // Top result renormalised to exactly 1.0.
    let top_score = fused.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
    assert!((top_score - 1.0).abs() < 1e-9, "top fused score must be 1.0: {fused:?}");
    assert_eq!(fused[0].0, "both", "the top-scored entry must be the chunk present in both lists: {fused:?}");
}

#[test]
fn ties_break_by_dense_rank() {
    // "in_dense" sits at dense rank 2 (only in dense); "not_in_dense" sits
    // at sparse rank 2 (only in sparse) -- same fused score
    // (1/(60+2) each), but "in_dense" must sort first.
    let dense = vec![("filler".to_string(), 1.0), ("in_dense".to_string(), 0.5)];
    let sparse = vec![("other_filler".to_string(), 1.0), ("not_in_dense".to_string(), 0.5)];

    let fused = rrf_fuse(&dense, &sparse, K_RRF);
    let pos = |id: &str| fused.iter().position(|(i, _)| i == id).unwrap();

    let score_in_dense = fused[pos("in_dense")].1;
    let score_not_in_dense = fused[pos("not_in_dense")].1;
    assert!(
        (score_in_dense - score_not_in_dense).abs() < 1e-9,
        "the two candidates must tie on fused score before tie-break: {fused:?}"
    );
    assert!(
        pos("in_dense") < pos("not_in_dense"),
        "a tied chunk present in dense must sort ahead of one absent from dense: {fused:?}"
    );
}

#[test]
fn a_chunk_absent_from_both_lists_never_appears() {
    let dense = vec![("a".to_string(), 1.0)];
    let sparse = vec![("b".to_string(), 1.0)];
    let fused = rrf_fuse(&dense, &sparse, K_RRF);
    assert_eq!(fused.len(), 2, "fusion only ever returns ids present in at least one list: {fused:?}");
    assert!(fused.iter().any(|(id, _)| id == "a"));
    assert!(fused.iter().any(|(id, _)| id == "b"));
}
