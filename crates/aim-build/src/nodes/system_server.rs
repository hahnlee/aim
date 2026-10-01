//! `system-server`: the derived image's `services.jar`, the original's
//! without SystemServer's start of each service `image/native-services`
//! lists, and with each call site `image/system-server-redirects` lists
//! calling its static method of the `device-services` node's
//! `aim-services.jar` (ADR 0013; `aim_android_image::system_server`,
//! `aim_android_image::redirect`). Its dex files are verified as ART opens
//! them; the `oat` node compiles it.

use super::java::Toolchain;
use super::{device_services, repo};
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use aim_android_image::redirect;
use aim_android_image::system_server::{dex_files, parse_native_services, patch_services_jar};
use std::fs;
use std::path::PathBuf;

const LIST: &str = "image/native-services";
const REDIRECTS: &str = "image/system-server-redirects";
const JAR: &str = "system/framework/services.jar";

pub fn out() -> PathBuf {
    aim_paths::out().join("system-server")
}

pub fn node() -> Node {
    Node {
        name: "system-server".into(),
        deps: vec![Dep::on("image"), Dep::on("device-services")],
        inputs: vec![repo(LIST), repo(REDIRECTS)],
        outputs: vec![out().join("services.jar")],
        tools: Vec::new(),
        recipe: 4,
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
    let text = fs::read_to_string(repo(REDIRECTS)).map_err(|e| format!("{REDIRECTS}: {e}"))?;
    let redirects = redirect::parse(&text).map_err(|e| format!("{REDIRECTS}: {e}"))?;
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let out = out();
    let work = out.with_extension("work");
    let _ = force_remove(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let jar = work.join("services.jar");
    patch_services_jar(&image.join(JAR), &jar, &classes)?;
    let targets = fs::read(device_services::jar()).map_err(|e| e.to_string())?;
    let patched = fs::read(&jar).map_err(|e| e.to_string())?;
    let redirected = redirect::redirect_jar(&patched, &redirects, &dex_files(&targets)?)
        .map_err(|e| format!("{REDIRECTS}: {e}"))?;
    fs::write(&jar, redirected).map_err(|e| e.to_string())?;
    Toolchain::fetch(log)?.verify_dex(log, &jar)?;
    log.line(&format!("not started: {}", classes.join(", ")));
    for r in &redirects {
        log.line(&format!(
            "redirected: {} in {} to {}",
            r.call, r.caller, r.to
        ));
    }
    let _ = force_remove(&out);
    fs::rename(&work, &out).map_err(|e| e.to_string())
}
