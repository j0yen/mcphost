//! PRD-mcphost-client-profile-registry AC2 — Given `SURFACE_IDS` and
//! `CLIENT_INFO_NAMES`, When the drift test runs, Then profile ids equal
//! `SURFACE_IDS` in order and every `CLIENT_INFO_NAMES` id appears in exactly
//! one row's `clientinfo_prefixes`.

use mcphost::client_profiles::CLIENT_PROFILES;
use mcphost::install_links::{CLIENT_INFO_NAMES, SURFACE_IDS};

#[test]
fn profile_ids_equal_surface_ids_in_order() {
    let ids: Vec<&str> = CLIENT_PROFILES.iter().map(|p| p.id).collect();
    assert_eq!(ids, SURFACE_IDS.to_vec());
}

#[test]
fn every_client_info_name_lands_in_exactly_one_row() {
    for (prefix, id) in CLIENT_INFO_NAMES {
        let owners: Vec<&str> = CLIENT_PROFILES
            .iter()
            .filter(|p| p.clientinfo_prefixes.contains(prefix))
            .map(|p| p.id)
            .collect();
        assert_eq!(owners, vec![*id], "CLIENT_INFO_NAMES entry {prefix:?} -> {id:?} must sit in exactly that row");
    }
    let total: usize = CLIENT_PROFILES.iter().map(|p| p.clientinfo_prefixes.len()).sum();
    assert_eq!(total, CLIENT_INFO_NAMES.len(), "rows carry no prefix CLIENT_INFO_NAMES lacks");
}
