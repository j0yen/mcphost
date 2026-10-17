//! PRD-mcphost-scope-registry-generated
//! AC4 (P0) — Given `mcphost llms-txt` output, When compared with
//! `www/llms.txt`, Then the "Scopes" table is byte-identical and its row
//! count equals `Scope::ALL.len()`.

use mcphost::kinds::KindRegistry;
use mcphost::llms_txt::{render_scopes_section, scopes_block};
use mcphost::scopes::Scope;

fn committed() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms.txt")).expect("read www/llms.txt")
}

#[test]
fn sreg_ac04_committed_scopes_block_is_byte_identical_to_the_generated_one() {
    let generated = render_scopes_section(&KindRegistry::with_builtin());
    let text = committed();
    let block = scopes_block(&text).expect("www/llms.txt carries the scopes block");
    assert_eq!(block, generated);
}

#[test]
fn sreg_ac04_table_has_one_row_per_registry_scope() {
    let generated = render_scopes_section(&KindRegistry::with_builtin());
    let rows: Vec<&str> = generated
        .lines()
        .filter(|l| l.starts_with("| "))
        .skip(1) // header
        .collect();
    assert_eq!(rows.len(), Scope::ALL.len(), "{rows:?}");
    for (row, scope) in rows.iter().zip(Scope::ALL) {
        assert!(row.starts_with(&format!("| {} |", scope.as_str())), "{row}");
    }
}

#[test]
fn sreg_ac04_scopes_section_sits_between_the_oauth_scoping_and_share_sections() {
    let text = committed();
    let oauth = text.find("## Scope your tools for OAuth clients").expect("oauth section");
    let scopes = text.find("## Scopes").expect("scopes section");
    let share = text.find("## Recipe calls: Share a tool, not a key").expect("share section");
    assert!(oauth < scopes && scopes < share);
}

#[test]
fn sreg_ac04_llms_txt_check_subcommand_reports_up_to_date() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["llms-txt", "--check"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mcphost llms-txt --check");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
