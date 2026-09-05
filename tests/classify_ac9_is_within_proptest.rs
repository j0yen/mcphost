//! PRD-mcphost-classify-precision P2/AC9: `is_within`'s path-prefix
//! invariants, as an integration-test `proptest!` (the audit's
//! `check_proptest_density` detector only counts `proptest!` invocations
//! under `tests/`, so this is a second home for the same property `src/
//! sandbox.rs`'s own `is_within_proptests` module already exercises
//! in-crate -- both are kept: the in-crate copy runs against the private
//! `classify_stderr` sibling too and stays close to what it tests, this one
//! moves the tests/ density metric).

use mcphost::sandbox::is_within;
use proptest::prelude::*;
use std::path::PathBuf;

proptest! {
    /// A root is always within itself.
    #[test]
    fn root_is_within_itself(segs in prop::collection::vec("[a-zA-Z0-9_-]{1,12}", 0..4)) {
        let mut root = PathBuf::from("/tmp/mcphost-classify-ac9-root");
        for seg in &segs {
            root.push(seg);
        }
        prop_assert!(is_within(&root, &root));
    }

    /// Any single child segment appended to root stays within root.
    #[test]
    fn child_of_root_is_within_root(child in "[a-zA-Z0-9_-]{1,32}") {
        let root = PathBuf::from("/tmp/mcphost-classify-ac9-root");
        let path = root.join(&child);
        prop_assert!(is_within(&root, &path));
    }

    /// A path under a disjoint sibling root is never within root.
    #[test]
    fn disjoint_sibling_is_not_within_root(child in "[a-zA-Z0-9_-]{1,32}") {
        let root = PathBuf::from("/tmp/mcphost-classify-ac9-root-a");
        let other = PathBuf::from("/tmp/mcphost-classify-ac9-root-b").join(&child);
        prop_assert!(!is_within(&root, &other));
    }
}
