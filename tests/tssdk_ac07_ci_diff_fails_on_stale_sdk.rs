//! PRD-mcphost-typescript-sdk-generated AC7 (P0) -- Given a PR that edits one
//! tool description without regenerating, When CI's `sdk-ts` job runs, Then
//! it fails on the `diff -r` step naming the file.

use std::path::Path;
use std::process::{Command, Output};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// The `run:` text of the `sdk-ts` job's regenerate-and-diff step.
fn diff_step() -> String {
    let ci = std::fs::read_to_string(Path::new(ROOT).join(".github/workflows/ci.yml")).unwrap();
    let job = ci.split("\n  sdk-ts:\n").nth(1).expect("ci.yml has an sdk-ts job");
    let job = job.split("\n  # ").next().unwrap();
    let step = job.split("- name: ").find(|s| s.contains("diff -r")).expect("sdk-ts job has a diff -r step");
    // the diff step must come before anything that would mask it
    assert!(job.find("diff -r").unwrap() < job.find("npm ci").unwrap());
    assert!(job.find("diff -r").unwrap() < job.find("npm test").unwrap());
    step.to_string()
}

fn mcphost_gen(out: &Path, contract: Option<&Path>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.args(["gen", "sdk", "ts", "--out"]).arg(out).current_dir(ROOT);
    if let Some(c) = contract {
        cmd.arg("--contract").arg(c);
    }
    let o = cmd.output().expect("run mcphost");
    assert!(o.status.success(), "gen sdk ts failed: {}", text(&o));
}

fn diff_r(generated: &Path) -> (bool, String) {
    let o = Command::new("diff")
        .args(["-r"])
        .arg(generated.join("src"))
        .arg(Path::new(ROOT).join("sdk/ts/src"))
        .output()
        .expect("run diff");
    (o.status.success(), text(&o))
}

#[test]
fn tssdk_ac07_ci_regenerates_and_diffs_and_a_stale_description_fails_naming_the_file() {
    let step = diff_step();
    assert!(step.contains("gen sdk ts --out /tmp/x"), "step does not regenerate: {step}");
    assert!(step.contains("diff -r /tmp/x/src sdk/ts/src"), "step does not diff: {step}");

    let tmp = std::env::temp_dir().join(format!("tssdk_ac07_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // the committed tree is what the generator renders right now
    let fresh = tmp.join("fresh");
    mcphost_gen(&fresh, None);
    let (same, out) = diff_r(&fresh);
    assert!(same, "committed sdk/ts/src is stale; run `mcphost gen sdk ts`:\n{out}");

    // a PR edits one tool description and does not rerun the generator
    let mut contract: serde_json::Value = serde_json::from_slice(mcphost::call_cli::EMBEDDED_CONTRACT).unwrap();
    let tool = contract["tools"].as_array_mut().unwrap().iter_mut().find(|t| t.get("alias_of").is_none()).unwrap();
    let edited = tool["name"].as_str().unwrap().to_string();
    let desc = tool["description"].as_str().unwrap().to_string();
    tool["description"] = serde_json::json!(format!("{desc} (edited in a PR)"));
    let edited_contract = tmp.join("contract.json");
    std::fs::write(&edited_contract, serde_json::to_vec_pretty(&contract).unwrap()).unwrap();

    let stale = tmp.join("stale");
    mcphost_gen(&stale, Some(&edited_contract));
    let (same, out) = diff_r(&stale);
    assert!(!same, "diff -r must fail when the description changed ({edited})");
    assert!(out.contains("client.ts"), "diff output must name the stale file:\n{out}");
    assert!(out.contains("(edited in a PR)"), "diff output must show the edit:\n{out}");
    let _ = std::fs::remove_dir_all(&tmp);
}
