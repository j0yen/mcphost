//! PRD-mcphost-migration-refusal-contract-b AC2 (P0) -- Given every
//! `(step, compat)` pair in the remedy table, When rendered, Then no
//! `remedy` names the retired `.config`/`authorize` file path, and every
//! `breaking` remedy contains `mcphost-deploy backup` before
//! `MCPHOST_DEPLOY_AUTHORIZE`.

use mcphost::compat_report::{BYPASS_ENV, Bypass, Compat, STEPS, refusal_text};

#[test]
fn mrefub_ac02_every_rendered_remedy_obeys_the_contract() {
    let bypass = Bypass { env: BYPASS_ENV.to_string(), value: "0.106.0..0.107.0".to_string() };
    for step in STEPS {
        for compat in [Compat::Previous, Compat::Breaking] {
            let (_, remedy) = refusal_text(step, compat, &bypass);
            let tag = format!("({step}, {})", compat.as_str());
            assert!(!remedy.contains("/.config"), "{tag} names a .config path: {remedy}");
            assert!(!remedy.contains("authorize"), "{tag} names the retired authorize file: {remedy}");
            if compat == Compat::Breaking {
                assert!(remedy.contains("mcphost-deploy backup"), "{tag} lacks the backup step: {remedy}");
                if let Some(auth) = remedy.find("MCPHOST_DEPLOY_AUTHORIZE") {
                    let backup = remedy.find("mcphost-deploy backup").unwrap();
                    assert!(backup < auth, "{tag} names the bypass before the backup: {remedy}");
                }
            }
        }
    }
}
