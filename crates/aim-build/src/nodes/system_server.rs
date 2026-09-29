//! `system-server`: the derived image's `services.jar`, the original's
//! without SystemServer's start of each service `image/native-services`
//! lists (ADR 0013; `aim_android_image::system_server`). The `oat` node
//! compiles it.

use super::repo;
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use aim_android_image::system_server::{parse_native_services, patch_services_jar};
use std::fs;
use std::path::PathBuf;

const LIST: &str = "image/native-services";
const JAR: &str = "system/framework/services.jar";

pub fn out() -> PathBuf {
    aim_paths::out().join("system-server")
}

pub fn node() -> Node {
    Node {
        name: "system-server".into(),
        deps: vec![Dep::on("image")],
        inputs: vec![repo(LIST)],
        outputs: vec![out().join("services.jar")],
        tools: Vec::new(),
        recipe: 3,
        action: Action::SystemServer,
        boot: true,
    }
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let text = fs::read_to_string(repo(LIST)).map_err(|e| format!("{LIST}: {e}"))?;
    let classes: Vec<String> = parse_native_services(&text)
        .map_err(|e| format!("{LIST}: {e}"))?
        .into_iter()
        .map(|s| s.class)
        .collect();
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let out = out();
    let work = out.with_extension("work");
    let _ = force_remove(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    patch_services_jar(&image.join(JAR), &work.join("services.jar"), &classes)?;
    log.line(&format!("not started: {}", classes.join(", ")));
    let _ = force_remove(&out);
    fs::rename(&work, &out).map_err(|e| e.to_string())
}
