//! PRD-mcphost-kind-ask-routing
//! AC5 (P1) -- Given the sandbox python module, When `help(mcphost.docs)` is
//! read, Then it names `host.docs.put` as the write path and states the
//! module is read-only.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn help_mcphost_docs_names_the_write_path_and_read_only() {
    let mut script = mcphost::kinds::python::runner_script_module_prelude().to_string();
    script.push_str(
        "\nimport pydoc\nm = sys.modules['mcphost.docs']\n\
         print(pydoc.render_doc(m, renderer=pydoc.plaintext))\n\
         print('GET:', m.get.__doc__)\nprint('SEARCH:', m.search.__doc__)\n",
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "python3 failed: {}", String::from_utf8_lossy(&out.stderr));
    let help = String::from_utf8_lossy(&out.stdout);

    assert!(help.contains("host.docs.put"), "help must name the write path:\n{help}");
    assert!(help.contains("read-only"), "help must state the module is read-only:\n{help}");
    // `search`/`get` docstrings carry it too.
    let tail = help.split("GET:").nth(1).expect("get docstring printed");
    assert!(tail.matches("host.docs.put").count() >= 2, "docstrings:\n{tail}");
    assert!(tail.matches("read-only").count() >= 2, "docstrings:\n{tail}");
}
