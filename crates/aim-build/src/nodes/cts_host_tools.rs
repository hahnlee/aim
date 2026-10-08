//! Darwin builds of pinned AOSP CTS extraction tools (#1166).
use super::repo;
use crate::graph::{Action, Node};
use crate::log::Log;
use crate::tools::Tool;
use std::path::PathBuf;
use std::process::Command;

pub fn out() -> PathBuf {
    aim_paths::out().join("cts-host-tools")
}

pub fn node() -> Node {
    Node {
        name: "cts-host-tools".into(),
        deps: Vec::new(),
        inputs: vec![repo("upstream/cts-host-tools.lock.json"), repo("tools/cts-host-tools.py")],
        outputs: ["deapexer", "debugfs_static", "fsck.erofs", "provenance.json"]
            .map(|name| out().join(name)).into(),
        tools: vec![Tool::Python],
        recipe: 1,
        action: Action::CtsHostTools,
        boot: false,
    }
}

pub fn run(log: &mut Log) -> Result<(), String> {
    log.run(Command::new("python3").arg(repo("tools/cts-host-tools.py")))
}
