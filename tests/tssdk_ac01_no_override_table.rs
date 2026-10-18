//! PRD-mcphost-typescript-sdk-generated AC1 (P0) -- Given the contract at the
//! PR head, When `mcphost gen sdk ts` runs and `tsc --strict --noEmit`
//! passes, Then fewer than 5 canonical tools needed an override-table entry;
//! the test prints the table and fails the premise at 5.

use mcphost::r#gen::{schema_ir, sdk_ts};

use crate::tssdk;

#[test]
fn tssdk_ac01_every_canonical_tool_renders_with_fewer_than_five_overrides() {
    println!("generator override table ({} entries):", sdk_ts::OVERRIDES.len());
    for (tool, why) in sdk_ts::OVERRIDES {
        println!("  {tool}: {why}");
    }
    assert!(
        sdk_ts::OVERRIDES.len() < 5,
        "premise falsified: {} hand overrides needed, the SDK is maintained, not generated",
        sdk_ts::OVERRIDES.len()
    );

    let bytes = mcphost::call_cli::EMBEDDED_CONTRACT;
    let files = sdk_ts::render(bytes).expect("contract renders");
    let (client, schemas) = (&files["client.ts"], &files["schemas.ts"]);
    let names = schema_ir::canonical_names(bytes).expect("canonical names");
    assert!(names.len() >= 150, "expected the full canonical set, got {}", names.len());
    for name in &names {
        let base = sdk_ts::camel(name);
        assert!(
            client.contains(&format!("this.#c.call(\"{name}\", s.{base}Args, s.{base}Result, args)")),
            "{name} has no client method"
        );
        assert!(schemas.contains(&format!("export const {base}Args = z.object(")), "{name} has no Args schema");
        assert!(schemas.contains(&format!("export const {base}Result = z.object(")), "{name} has no Result schema");
    }

    // alias entries are skipped: one method per canonical tool, no more
    assert_eq!(client.matches("this.#c.call(").count(), names.len());

    // rendering is deterministic, so CI can diff a regenerated tree
    assert_eq!(files, sdk_ts::render(bytes).unwrap());

    // the committed tree is what the generator renders, and it type-checks strict
    for (name, body) in &files {
        let committed = std::fs::read_to_string(tssdk::sdk_dir().join("src").join(name)).unwrap_or_default();
        assert_eq!(&committed, body, "sdk/ts/src/{name} is stale; run `mcphost gen sdk ts`");
    }
    let tsc = tssdk::npm(&["exec", "--", "tsc", "--strict", "--noEmit"]);
    assert!(tsc.status.success(), "tsc --strict --noEmit failed:\n{}", tssdk::text(&tsc));
}
