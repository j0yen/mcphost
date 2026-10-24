//! Guard: the numeric prefixes of `migrations/*.sql` are unique and sorted, so
//! two parallel branches cannot land the same migration number.

use std::fs;
use std::path::Path;

/// Numbers already duplicated on main before this guard existed (both files
/// are wired in `src/db.rs`). Never add to this list; renumber instead.
const GRANDFATHERED_DUPLICATES: &[u32] = &[113];

#[test]
fn migration_numbers_are_unique_and_sorted() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut nums: Vec<(u32, String)> = Vec::new();
    for entry in fs::read_dir(&dir).expect("read migrations/") {
        let name = entry.expect("dir entry").file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sql") {
            continue;
        }
        let prefix = name.get(..4).unwrap_or("");
        let n: u32 = prefix
            .parse()
            .unwrap_or_else(|_| panic!("migration {name} lacks a 4-digit numeric prefix"));
        nums.push((n, name));
    }
    nums.sort();
    for pair in nums.windows(2) {
        if pair[0].0 == pair[1].0 && GRANDFATHERED_DUPLICATES.contains(&pair[0].0) {
            continue;
        }
        assert!(
            pair[0].0 < pair[1].0,
            "migration numbers must be unique and sorted: {} vs {}",
            pair[0].1,
            pair[1].1
        );
    }
}
