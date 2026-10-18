//! PRD-mcphost-python-sdk-generated AC1 (P0) -- Given the contract at the PR
//! head, When `mcphost gen sdk python` runs, Then every canonical tool renders
//! without an entry in the generator's override table, or the table has < 5
//! entries; the test prints the table and fails the premise at 5.

use mcphost::sdk_python;

#[test]
fn pysdk_ac01_every_canonical_tool_renders_with_fewer_than_five_overrides() {
    println!("generator override table ({} entries):", sdk_python::OVERRIDES.len());
    for (tool, why) in sdk_python::OVERRIDES {
        println!("  {tool}: {why}");
    }
    assert!(
        sdk_python::OVERRIDES.len() < 5,
        "premise falsified: {} hand overrides needed, the SDK is maintained, not generated",
        sdk_python::OVERRIDES.len()
    );

    let bytes = mcphost::call_cli::EMBEDDED_CONTRACT;
    let files = sdk_python::render(bytes).expect("contract renders");
    let (client, models) = (&files["client.py"], &files["models.py"]);
    let names = sdk_python::canonical_names(bytes).expect("canonical names");
    assert!(names.len() >= 150, "expected the full canonical set, got {}", names.len());
    for name in &names {
        let base = sdk_python::camel(name);
        assert!(
            client.contains(&format!("self._c._call(\"{name}\", {base}Args, {base}Result, raw)")),
            "{name} has no client method"
        );
        assert!(models.contains(&format!("class {base}Args(BaseModel)")), "{name} has no Args model");
        assert!(models.contains(&format!("class {base}Result(BaseModel)")), "{name} has no Result model");
    }

    // alias entries are skipped: one method per canonical tool, no more
    assert_eq!(client.matches("self._c._call(").count(), names.len());

    // rendering is deterministic, so CI can diff a regenerated tree
    assert_eq!(files, sdk_python::render(bytes).unwrap());
}
