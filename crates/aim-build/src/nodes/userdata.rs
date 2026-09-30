//! `userdata/*` (after AOSP's data partition image): the data templates
//! in `target/aim/userdata` a new data directory starts from
//! (docs/first-boot.md). guest-init clones one (`--userdata`,
//! `aim_storage::data::template`) instead of making the data image:
//!
//! - `userdata/empty`: `empty.asif`, an empty data image: an instant APFS
//!   clone where making one costs about half a second of a first boot
//!   (#563).

use crate::graph::{Action, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use aim_storage::data::{self, EMPTY_TEMPLATE};
use std::fs;
use std::path::PathBuf;

pub fn out() -> PathBuf {
    aim_paths::userdata()
}

pub fn empty() -> Node {
    Node {
        name: "userdata/empty".into(),
        deps: Vec::new(),
        inputs: Vec::new(),
        outputs: vec![out().join(EMPTY_TEMPLATE)],
        tools: Vec::new(),
        recipe: 1,
        action: Action::EmptyUserdata,
        boot: true,
    }
}

pub fn run_empty(log: &mut Log) -> Result<(), String> {
    let out = out();
    let work = out.with_extension("work");
    let _ = force_remove(&work);
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let empty = work.join(EMPTY_TEMPLATE);
    data::create(&empty)?;
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    fs::rename(&empty, out.join(EMPTY_TEMPLATE)).map_err(|e| e.to_string())?;
    force_remove(&work).map_err(|e| e.to_string())?;
    log.line(&format!("data templates: {}", out.display()));
    Ok(())
}
