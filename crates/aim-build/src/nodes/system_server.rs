//! `system-server`: the derived image's `services.jar`, the original's
//! without SystemServer's start of each service `image/native-services`
//! lists (ADR 0013; `aim_android_image::system_server`).

use super::repo;
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::system_server::{parse_native_services, patch_services_jar};
use std::fs;
use std::path::PathBuf;

const LIST: &str = "image/native-services";
const JAR: &str = "system/framework/services.jar";

pub fn out() -> PathBuf {
    aim_paths::out().join("system-server/services.jar")
}

pub fn node() -> Node {
    Node {
        name: "system-server".into(),
        deps: vec![Dep::on("image")],
        inputs: vec![repo(LIST)],
        outputs: vec![out()],
        tools: Vec::new(),
        recipe: 1,
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
    let original = aim_paths::original_image().join(JAR);
    let out = out();
    fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    patch_services_jar(&original, &out, &classes)?;
    log.line(&format!(
        "{}: not started: {}",
        out.display(),
        classes.join(", ")
    ));
    Ok(())
}
