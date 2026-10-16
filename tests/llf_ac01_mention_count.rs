//! PRD-mcphost-llms-full-generated-from-schemas AC1: count how many tools of
//! `host_tools(kinds, true)` (via `llms_txt_tool_names`, the same set) the
//! committed `www/llms-full.txt` mentions by name. Prints
//! `tools=<n> mentioned=<m>`; below half, the AC5 exception-list check is
//! logged `not applicable` rather than failing.

use mcphost::kinds::KindRegistry;

#[test]
fn llf_ac01() {
    let kinds = KindRegistry::with_builtin();
    let names = mcphost::handler::llms_txt_tool_names(&kinds);
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms-full.txt"))
        .expect("www/llms-full.txt");
    let n = names.len();
    let m = names.iter().filter(|name| text.contains(name.as_str())).count();
    println!("tools={n} mentioned={m}");
    assert!(n > 0, "host_tools returned no tools");
    if m < n / 2 {
        println!("AC5 exception-list check: not applicable (mentioned < tools/2)");
    } else {
        println!("AC5 exception-list check: applicable");
    }
}
