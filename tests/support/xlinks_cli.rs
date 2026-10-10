//! Scratch-copy runner for `mcphost llms-txt` (xlinks AC4/AC5): copies the
//! committed README.md, www/llms.txt and docs/clients.toml into a scratch
//! dir so a test can edit them and run the real binary without touching
//! the working tree.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = std::env::temp_dir().join(format!(
            "mcphost-xlinks-cli-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (from, to) in [("README.md", "README.md"), ("www/llms.txt", "llms.txt"), ("docs/clients.toml", "clients.toml")] {
            std::fs::copy(root.join(from), dir.join(to)).unwrap();
        }
        Scratch { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap()
    }

    pub fn write(&self, name: &str, body: &str) {
        std::fs::write(self.path(name), body).unwrap();
    }

    /// `mcphost llms-txt [--check]` over the scratch files.
    pub fn llms_txt(&self, check: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
        cmd.arg("llms-txt");
        if check {
            cmd.arg("--check");
        }
        cmd.arg("--path").arg(self.path("llms.txt"));
        cmd.arg("--readme").arg(self.path("README.md"));
        cmd.arg("--clients").arg(self.path("clients.toml"));
        cmd.output().expect("run mcphost llms-txt")
    }
}
