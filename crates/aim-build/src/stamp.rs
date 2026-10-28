//! A node's stamp (`target/aim-cache/<node>.stamp`): the key it was last
//! built with and what went into it, so a check needs no rebuild and
//! `cargo aim status` can say what changed.
//!
//! ```text
//! aim-stamp 1
//! key <hex>
//! recipe <n>
//! tool <name> <sha256 of its version>
//! dep <node> <key>
//! input <sha256> <size> <mtime ns> <path>
//! output <sha256> <size> <mtime ns> <path>
//! found <path>
//! ```
//!
//! Paths inside the repository are relative to its root. `found` lists the
//! inputs the node's tools reported (cargo dep-info, n2's deps log); every
//! input, found or declared, has an `input` line.

use crate::hash::FileState;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const HEADER: &str = "aim-stamp 1";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub key: String,
    pub recipe: u32,
    pub tools: BTreeMap<String, String>,
    pub deps: BTreeMap<String, String>,
    pub inputs: BTreeMap<PathBuf, FileState>,
    pub outputs: BTreeMap<PathBuf, FileState>,
    pub found: Vec<PathBuf>,
}

/// `path` as a stamp records it.
pub fn display(path: &Path) -> PathBuf {
    path.strip_prefix(aim_paths::root())
        .unwrap_or(path)
        .to_path_buf()
}

fn resolve(path: &str) -> PathBuf {
    aim_paths::root().join(path)
}

pub fn file(node: &str) -> PathBuf {
    aim_paths::cache().join(format!("{}.stamp", node.replace('/', "~")))
}

impl Stamp {
    pub fn read(node: &str) -> Option<Stamp> {
        let text = fs::read_to_string(file(node)).ok()?;
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut stamp = Stamp::default();
        for line in lines {
            let (tag, rest) = line.split_once(' ')?;
            match tag {
                "key" => stamp.key = rest.into(),
                "recipe" => stamp.recipe = rest.parse().ok()?,
                "tool" => {
                    let (name, version) = rest.split_once(' ')?;
                    stamp.tools.insert(name.into(), version.into());
                }
                "dep" => {
                    let (name, key) = rest.split_once(' ')?;
                    stamp.deps.insert(name.into(), key.into());
                }
                "input" | "output" => {
                    let mut fields = rest.splitn(4, ' ');
                    let hash = fields.next()?.to_string();
                    let size = fields.next()?.parse().ok()?;
                    let mtime_ns = fields.next()?.parse().ok()?;
                    let path = resolve(fields.next()?);
                    let states = if tag == "input" {
                        &mut stamp.inputs
                    } else {
                        &mut stamp.outputs
                    };
                    states.insert(
                        path,
                        FileState {
                            size,
                            mtime_ns,
                            hash,
                        },
                    );
                }
                "found" => stamp.found.push(resolve(rest)),
                _ => return None,
            }
        }
        Some(stamp)
    }

    pub fn write(&self, node: &str) -> Result<(), String> {
        let mut text = format!("{HEADER}\nkey {}\nrecipe {}\n", self.key, self.recipe);
        for (name, version) in &self.tools {
            text.push_str(&format!("tool {name} {version}\n"));
        }
        for (name, key) in &self.deps {
            text.push_str(&format!("dep {name} {key}\n"));
        }
        for (tag, states) in [("input", &self.inputs), ("output", &self.outputs)] {
            for (path, state) in states {
                text.push_str(&format!(
                    "{tag} {} {} {} {}\n",
                    state.hash,
                    state.size,
                    state.mtime_ns,
                    display(path).display()
                ));
            }
        }
        for path in &self.found {
            text.push_str(&format!("found {}\n", display(path).display()));
        }
        let path = file(node);
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let partial = path.with_extension("partial");
        fs::write(&partial, text).map_err(|e| format!("{}: {e}", partial.display()))?;
        fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn remove(node: &str) {
        let _ = fs::remove_file(file(node));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_fingerprints_survive_a_stamp_round_trip() {
        let name = format!("test-output-stamp-{}", std::process::id());
        let state = FileState {
            hash: "digest".into(),
            size: 9,
            mtime_ns: 123,
        };
        let stamp = Stamp {
            inputs: BTreeMap::from([(aim_paths::root().join("input with spaces"), state.clone())]),
            outputs: BTreeMap::from([(aim_paths::root().join("output with spaces"), state)]),
            ..Stamp::default()
        };
        stamp.write(&name).unwrap();
        assert_eq!(Stamp::read(&name), Some(stamp));
        fs::remove_file(file(&name)).unwrap();
    }
}
