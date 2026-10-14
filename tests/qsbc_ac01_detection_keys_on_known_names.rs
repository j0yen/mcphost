//! AC1 (PRD-mcphost-quickstart-by-client) — every `CLIENT_INFO_NAMES` and
//! `USER_AGENT_NAMES` entry detects to its own surface id, and every id in
//! `SURFACE_IDS` has exactly one `SURFACE_CAVEATS` row (and no other ids).

use mcphost::install_links::{CLIENT_INFO_NAMES, SURFACE_CAVEATS, SURFACE_IDS, USER_AGENT_NAMES, detect};

#[test]
fn every_client_info_name_detects_to_its_surface() {
    for (name, id) in CLIENT_INFO_NAMES {
        assert_eq!(detect(Some(name), None), Some((*id, "clientInfo")), "{name}");
        assert_eq!(detect(Some(&name.to_uppercase()), None), Some((*id, "clientInfo")), "{name} upper");
    }
}

#[test]
fn every_user_agent_name_detects_to_its_surface() {
    for (name, id) in USER_AGENT_NAMES {
        let ua = format!("{name}/1.2.3");
        assert_eq!(detect(None, Some(&ua)), Some((*id, "user_agent")), "{name}");
    }
}

#[test]
fn client_info_wins_over_user_agent() {
    assert_eq!(detect(Some("cursor"), Some("codex-cli/1.2")), Some(("cursor", "clientInfo")));
    assert_eq!(detect(Some("mystery"), Some("codex-cli/1.2")), Some(("codex_cli", "user_agent")));
    assert_eq!(detect(Some("mystery"), Some("curl/8")), None);
}

#[test]
fn table_ids_are_surfaces_and_caveats_cover_every_surface_once() {
    for (_, id) in CLIENT_INFO_NAMES.iter().chain(USER_AGENT_NAMES.iter()) {
        assert!(SURFACE_IDS.contains(id), "{id} not in SURFACE_IDS");
    }
    for id in SURFACE_IDS {
        let rows = SURFACE_CAVEATS.iter().filter(|(row_id, _, _)| *row_id == id).count();
        assert_eq!(rows, 1, "{id} must have exactly one SURFACE_CAVEATS row");
    }
    for (id, caveats, next) in SURFACE_CAVEATS {
        assert!(SURFACE_IDS.contains(id), "caveats row for unknown surface {id}");
        assert!(!caveats.is_empty() && next.starts_with("host."), "{id}");
    }
}
