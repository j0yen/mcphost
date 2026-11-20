//! Shared helper for tests that execute a file the test itself wrote,
//! copied or chmod'd (PRD-less fix: mcphost tests never fail on ETXTBSY).
//! `fs::copy` / `File::create` hold a write fd open for the duration of the
//! write; a `fork` in another test thread during that window inherits the fd,
//! and until that child execs the kernel refuses to exec the file ("Text file
//! busy", os error 26). `set_permissions` opens no fd, so the window is the
//! write itself. Running the file through its shebang interpreter means it is
//! only ever read, never exec'd, so the race cannot occur.
#![allow(dead_code)]

use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const ETXTBSY: i32 = 26;
const RETRIES: u32 = 50;
const RETRY_SLEEP: Duration = Duration::from_millis(20);

/// The interpreter named by `path`'s `#!` line (`#!/usr/bin/env bash` -> `bash`,
/// `#!/bin/sh` -> `/bin/sh`), plus its inline arguments. Errors, naming the
/// file, when it has no shebang.
pub fn shebang_interpreter(path: &Path) -> io::Result<(String, Vec<String>)> {
    let text = std::fs::read(path)?;
    let first = text.split(|b| *b == b'\n').next().unwrap_or(&[]);
    let line = String::from_utf8_lossy(first);
    let Some(rest) = line.strip_prefix("#!") else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{}: no #! shebang line; cannot run it through an interpreter",
                path.display()
            ),
        ));
    };
    let mut words = rest.split_whitespace().map(str::to_string);
    let mut prog = words.next().unwrap_or_default();
    let mut args: Vec<String> = words.collect();
    if prog.ends_with("/env") && !args.is_empty() {
        prog = args.remove(0);
    }
    if prog.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: empty #! interpreter", path.display()),
        ));
    }
    Ok((prog, args))
}

/// A `Command` that runs the script at `path` via its shebang interpreter
/// (`<interp> [inline args] <path>`); the caller adds args, cwd and env as
/// for any `Command`. Use this for every file a test wrote or copied.
pub fn script_command(path: &Path) -> io::Result<Command> {
    let (prog, inline) = shebang_interpreter(path)?;
    let mut cmd = Command::new(prog);
    cmd.args(inline).arg(path);
    Ok(cmd)
}

/// Like `script_command` but panics (naming the file) when it cannot.
pub fn script(path: &Path) -> Command {
    script_command(path).unwrap_or_else(|e| panic!("{e}"))
}

/// Retry path for a written file that is a real non-script binary: re-run
/// `build()` while spawning fails with ETXTBSY, up to 50 times with 20 ms
/// sleeps, then return the last error. Prints the retry count to stderr after
/// any retry that ended in success. tests/ has no such caller today.
pub fn output_retrying_etxtbsy(
    mut build: impl FnMut() -> Command,
) -> io::Result<std::process::Output> {
    let mut retries = 0;
    loop {
        match build().output() {
            Ok(out) => {
                if retries > 0 {
                    eprintln!("execw: spawn succeeded after {retries} ETXTBSY retries");
                }
                return Ok(out);
            }
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && retries < RETRIES => {
                retries += 1;
                std::thread::sleep(RETRY_SLEEP);
            }
            Err(e) => return Err(e),
        }
    }
}
