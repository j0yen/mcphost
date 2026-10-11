//! Shared helpers for the `blobguard_ac0N_*` tests (PRD-mcphost-blob-virus-and-type-guard).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

pub const EICAR: &[u8] = b"X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

pub fn fresh_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("blobguard-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A stored-method zip whose central directory declares `uncompressed` for
/// each entry (lets a 1 KB file claim 10 GB without writing 10 GB).
/// (name, data, declared uncompressed size override)
pub type ZipSpec<'a> = (&'a str, &'a [u8], Option<u32>);

pub fn zip(entries: &[ZipSpec<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, declared) in entries {
        let off = out.len() as u32;
        let size = data.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&declared.unwrap_or(size).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 12]);
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

pub fn png_bytes(total: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    v.resize(total.max(v.len()), 0xAB);
    v
}

pub const PE: &[u8] = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00PE\x00\x00 padding \x00\x01\x02";

/// Fake clamd on a Unix socket: answers `stream: OK`, or
/// `stream: Eicar-Test-Signature FOUND` when the streamed bytes contain EICAR.
pub fn fake_clamd(dir: &std::path::Path) -> PathBuf {
    let sock = dir.join("clamd.ctl");
    let listener = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { continue };
            let mut cmd = [0u8; 10];
            if c.read_exact(&mut cmd).is_err() || &cmd != b"zINSTREAM\0" {
                continue;
            }
            let mut data = Vec::new();
            loop {
                let mut len = [0u8; 4];
                if c.read_exact(&mut len).is_err() {
                    break;
                }
                let n = u32::from_be_bytes(len) as usize;
                if n == 0 {
                    break;
                }
                let mut chunk = vec![0u8; n];
                if c.read_exact(&mut chunk).is_err() {
                    break;
                }
                data.extend_from_slice(&chunk);
            }
            let found = data.windows(EICAR.len()).any(|w| w == EICAR);
            let reply: &[u8] = if found { b"stream: Eicar-Test-Signature FOUND\0" } else { b"stream: OK\0" };
            let _ = c.write_all(reply);
        }
    });
    sock
}

/// Env var that tells a re-executed child test where its parent's scratch
/// dir is; the child is a no-op without it.
pub const CHILD_DIR_ENV: &str = "BLOBGUARD_CHILD_DIR";

/// Re-runs this test binary on exactly `test` with `envs` set (`None` =
/// removed), so a test can exercise a real process environment without
/// mutating the shared one. Asserts the child ran its one test and passed.
pub fn run_child(test: &str, envs: &[(&str, Option<&std::ffi::OsStr>)]) -> String {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", test, "--nocapture", "--test-threads=1"]);
    for (k, v) in envs {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    let out = cmd.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("1 passed"), "child {test} failed:\n{text}");
    text
}
