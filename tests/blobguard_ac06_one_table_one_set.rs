//! PRD-mcphost-blob-virus-and-type-guard AC6 — minting a link with
//! `accept: ["image/x-made-up"]` is `args_invalid` listing every
//! `BLOB_TYPES` mime; the page's `accept=` attribute, the error's list and
//! `gendocs`'s table are the same set.
use std::collections::BTreeSet;

use mcphost::blobs::{accept_attr, all_mimes, default_accept, mimes_in_accept_attr, mimes_listed_in_error, validate_accept, BLOB_TYPES};
use mcphost::errors::AppError;
use mcphost::gendocs::render_blob_types_table;

#[test]
fn made_up_accept_is_args_invalid_and_three_surfaces_are_one_set() {
    let err = validate_accept(&["image/x-made-up".to_string()]).unwrap_err();
    assert_eq!(err.code(), "args_invalid");
    let AppError::InvalidArgs(msg) = &err else { panic!("expected InvalidArgs, got {err:?}") };

    let table: BTreeSet<String> = BLOB_TYPES.iter().map(|t| t.mime.to_string()).collect();
    let in_error: BTreeSet<String> = mimes_listed_in_error(msg).into_iter().collect();
    let in_page: BTreeSet<String> = mimes_in_accept_attr(&accept_attr(&all_mimes().iter().map(|m| m.to_string()).collect::<Vec<_>>())).into_iter().collect();
    let in_docs: BTreeSet<String> = render_blob_types_table()
        .lines()
        .skip(2)
        .filter_map(|l| l.split('`').nth(1).map(str::to_string))
        .collect();

    assert_eq!(in_error, table, "error list == BLOB_TYPES");
    assert_eq!(in_page, table, "page accept= == BLOB_TYPES");
    assert_eq!(in_docs, table, "gendocs table == BLOB_TYPES");

    // Default accept = every non-active type (R2); every valid entry validates.
    assert!(validate_accept(&default_accept(true)).is_ok());
    assert!(!default_accept(false).contains(&"image/svg+xml".to_string()));
    assert!(default_accept(false).contains(&"text/csv".to_string()));
}
