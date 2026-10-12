//! AC2 (PRD-mcphost-call-cli-from-contract) -- Given `mcphost call
//! host.tool.publish --name x --kind nope --spec-json '{}'`, When parsed,
//! Then clap exits 2 before any network with the schema's kind enum in the
//! message.

use std::net::TcpListener;
use std::process::Command;
use std::time::Duration;

#[test]
fn a_kind_outside_the_schema_enum_is_refused_by_clap_before_any_network() {
    // A listener that must never see a connection: the host URL points at it.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let url = format!("http://{}", listener.local_addr().expect("addr"));

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["call", "host.tool.publish", "--name", "x", "--kind", "nope", "--spec-json", "{}"])
        .args(["--url", &url])
        .env_remove("MCPHOST_KEY")
        .output()
        .expect("run mcphost call");

    assert_eq!(out.status.code(), Some(2), "clap usage errors exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--kind"), "message names the flag: {stderr}");
    assert!(stderr.contains("nope"), "message names the bad value: {stderr}");

    // The enum comes from the contract, not a hand list: every value of the
    // schema's `kind` enum is in the message.
    let contract: serde_json::Value =
        serde_json::from_slice(mcphost::call_cli::EMBEDDED_CONTRACT).expect("contract json");
    let tool = contract["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "host.tool.publish")
        .expect("host.tool.publish in the contract");
    let kinds = tool["input_schema"]["properties"]["kind"]["enum"].as_array().expect("kind enum");
    assert!(!kinds.is_empty());
    for kind in kinds {
        let kind = kind.as_str().expect("enum string");
        assert!(stderr.contains(kind), "kind enum value `{kind}` missing from: {stderr}");
    }

    std::thread::sleep(Duration::from_millis(100));
    assert!(
        matches!(listener.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "no connection may reach the host before clap refuses the call"
    );
}
