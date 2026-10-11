//! PRD-mcphost-blob-virus-and-type-guard AC6 — minting a link with
//! `accept: ["image/x-made-up"]` is `args_invalid` listing every
//! `BLOB_TYPES` mime; the page's `accept=` attribute, the error's list and
//! `gendocs`'s table are the same set.
//!
//! Each surface is read from what ships: the mint goes through
//! `link_policy_from_args` (the `host.blob.upload_link` argument parser),
//! the page is `upload_page_html` for an all-types link, and the docs table
//! is the committed `docs/blob-types.md` that `gen-docs` writes.
use std::collections::BTreeSet;

use mcphost::blobs::{accept_attr_in_page, link_policy_from_args, mimes_in_accept_attr, mimes_listed_in_error, upload_page_html, BLOB_TYPES};
use mcphost::errors::AppError;
use serde_json::json;

#[test]
fn made_up_accept_is_args_invalid_and_three_surfaces_are_one_set() {
    let err = link_policy_from_args(&json!({"accept": ["image/x-made-up"]})).unwrap_err();
    assert_eq!(err.code(), "args_invalid");
    let AppError::InvalidArgs(msg) = &err else { panic!("expected InvalidArgs, got {err:?}") };

    let table: BTreeSet<String> = BLOB_TYPES.iter().map(|t| t.mime.to_string()).collect();
    let in_error: BTreeSet<String> = mimes_listed_in_error(msg).into_iter().collect();

    let all = link_policy_from_args(&json!({"allow_active": true})).unwrap();
    let page = upload_page_html(&all);
    let attr = accept_attr_in_page(&page).expect("page has an accept= attribute");
    let in_page: BTreeSet<String> = mimes_in_accept_attr(attr).into_iter().collect();

    let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/blob-types.md")).unwrap();
    let in_docs: BTreeSet<String> = docs
        .lines()
        .filter(|l| l.starts_with("| `"))
        .filter_map(|l| l.split('`').nth(1).map(str::to_string))
        .collect();

    assert_eq!(in_error, table, "error list == BLOB_TYPES");
    assert_eq!(in_page, table, "page accept= == BLOB_TYPES");
    assert_eq!(in_docs, table, "gendocs table == BLOB_TYPES");

    // R2 default: a link minted without `accept` takes every non-active
    // type, and its page says so.
    let default_page = upload_page_html(&link_policy_from_args(&json!({})).unwrap());
    let default_set: BTreeSet<String> = mimes_in_accept_attr(accept_attr_in_page(&default_page).unwrap()).into_iter().collect();
    let non_active: BTreeSet<String> = BLOB_TYPES.iter().filter(|t| !t.active_content).map(|t| t.mime.to_string()).collect();
    assert_eq!(default_set, non_active);

    // A valid narrow mint renders only that type.
    let csv = link_policy_from_args(&json!({"accept": ["text/csv"]})).unwrap();
    assert_eq!(mimes_in_accept_attr(accept_attr_in_page(&upload_page_html(&csv)).unwrap()), vec!["text/csv".to_string()]);
}
