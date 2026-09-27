//! PRD-mcphost-oauth-conformance-harness
//! AC2 (P0) — Given a fake authorization server in the test (implemented
//! in test support, not the host) that advertises CIMD and `"none"`, When
//! the simulator registers, Then it chooses CIMD; When the fake advertises
//! neither CIMD nor `registration_endpoint`, Then the `register` step
//! reads `unsupported` with the reason.

use crate::fake_as;
use crate::oauthclient;
use fake_as::FakeAsOptions;
use oauthclient::{RegMethod, Verdict};

#[tokio::test]
async fn cimd_is_chosen_when_advertised_with_none_auth_method() {
    let http = reqwest::Client::new();
    let as_server = fake_as::start(FakeAsOptions { cimd_supported: true, none_auth_method: true, registration_endpoint: false }).await;

    let (meta_record, meta) = oauthclient::read_as_metadata(&http, &as_server.base_url).await;
    assert_eq!(meta_record.verdict, Verdict::Pass, "AS metadata must be readable: {meta_record:?}");
    let meta = meta.expect("AS metadata document");

    let cimd_url = "https://simulator.example/client-metadata.json";
    let (record, registration) = oauthclient::register(&http, &meta, cimd_url, "native").await;
    assert_eq!(record.verdict, Verdict::Pass, "register must pass: {record:?}");
    let registration = registration.expect("a Registration on success");
    assert_eq!(registration.method, RegMethod::Cimd, "CIMD must be chosen over DCR");
    assert_eq!(registration.client_id, cimd_url, "CIMD's client_id is the metadata document's own URL");
}

#[tokio::test]
async fn register_is_unsupported_with_a_reason_when_neither_cimd_nor_registration_endpoint_is_advertised() {
    let http = reqwest::Client::new();
    let as_server =
        fake_as::start(FakeAsOptions { cimd_supported: false, none_auth_method: false, registration_endpoint: false }).await;

    let (meta_record, meta) = oauthclient::read_as_metadata(&http, &as_server.base_url).await;
    assert_eq!(meta_record.verdict, Verdict::Pass, "AS metadata must be readable: {meta_record:?}");
    let meta = meta.expect("AS metadata document");
    assert!(!meta.client_id_metadata_document_supported);
    assert!(meta.registration_endpoint.is_none());

    let (record, registration) = oauthclient::register(&http, &meta, "https://simulator.example/client-metadata.json", "native").await;
    assert_eq!(record.verdict, Verdict::Unsupported, "register must read unsupported: {record:?}");
    assert!(record.reason.is_some(), "an unsupported register must carry a reason");
    assert!(registration.is_none());
}
