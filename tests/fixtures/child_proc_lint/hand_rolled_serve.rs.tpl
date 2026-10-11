use std::io::Read;
use std::process::{Command, Stdio};
#[test]
fn hand_rolled() {
    let mut child = Command::new(bin)
        .arg("serve")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut buf = String::new();
    child.stderr.take().unwrap().read_to_string(&mut buf).unwrap();
}
