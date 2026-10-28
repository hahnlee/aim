//! Pinned official CTS test inputs, fetched without a guest boot.
use super::repo;
use crate::graph::{Action, Node};
use crate::log::Log;
use crate::tools::Tool;
use std::path::PathBuf;
use std::process::Command;

fn outputs(lock: &str, tradefed: bool) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(repo(lock)).map_err(|e| format!("{lock}: {e}"))?;
    let root = aim_paths::fetched().join(if tradefed { "cts-tradefed" } else { "cts" });
    text.lines()
        .filter_map(|line| line.split('#').next().unwrap().trim().strip_prefix('"'))
        .map(|line| {
            let (name, _) = line
                .split_once('|')
                .ok_or_else(|| format!("{lock}: invalid entry"))?;
            Ok(root.join(if tradefed {
                name
            } else {
                name.rsplit('/').next().unwrap()
            }))
        })
        .collect()
}

pub fn nodes() -> Result<Vec<Node>, String> {
    [false, true]
        .into_iter()
        .map(|tradefed| {
            let lock = if tradefed {
                "upstream/cts-tradefed.lock"
            } else {
                "upstream/cts.lock"
            };
            let mut inputs = vec![repo("upstream/cts.lock"), repo("tools/cts-module.py")];
            if tradefed {
                inputs.push(repo(lock));
            }
            Ok(Node {
                name: if tradefed { "cts-tradefed" } else { "cts" }.into(),
                deps: Vec::new(),
                inputs,
                outputs: outputs(lock, tradefed)?,
                tools: vec![Tool::Python],
                recipe: 1,
                action: Action::Cts { tradefed },
                boot: false,
            })
        })
        .collect()
}

pub fn run(log: &mut Log, tradefed: bool) -> Result<(), String> {
    let mut command = Command::new("python3");
    command.arg(repo("tools/cts-module.py"));
    if tradefed {
        command.arg("--tradefed");
    }
    log.run(&mut command)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declares_each_pinned_entry_in_its_fetch_layout() {
        let nodes = nodes().unwrap();
        assert!(
            nodes
                .iter()
                .all(|node| !node.boot && !node.outputs.is_empty())
        );
        assert!(
            nodes[0]
                .outputs
                .contains(&aim_paths::fetched().join("cts/CtsPkgInstallTinyAppV1.apk"))
        );
        assert!(nodes[1].outputs.contains(&aim_paths::fetched().join(
            "cts-tradefed/android-cts/testcases/CtsCompilationTestCases/CtsCompilationTestCases.jar"
        )));
        assert!(
            nodes
                .iter()
                .all(|node| node.inputs.contains(&repo("upstream/cts.lock")))
        );
    }
}
