//! `system-server`: the derived image's `services.jar`, the original's
//! without SystemServer's start of each service `image/native-services`
//! lists (ADR 0013; `aim_android_image::system_server`), and its oat
//! files.
//!
//! The image's `services.odex` and `.vdex` name the jar's entries by their
//! CRCs, so they no longer match the edited jar, and system_server would
//! verify services.jar's classes at run time (a boot that completes, then
//! ANRs everywhere). They are compiled again, as the image's are
//! effective under the ART exception: the original odex was compiled for
//! the original boot image and runtime, so only its vdex (verification)
//! was used. dex2oat64 runs as the boot-image node runs it, with the
//! `verify` filter against the regenerated boot image.

use super::boot_image::{Dex2oat, oat_key};
use super::repo;
use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use aim_android_image::system_server::{parse_native_services, patch_services_jar};
use std::fs;
use std::path::PathBuf;

const LIST: &str = "image/native-services";
const JAR: &str = "/system/framework/services.jar";
const ODEX: &str = "/system/framework/oat/arm64/services.odex";

pub fn out() -> PathBuf {
    aim_paths::out().join("system-server")
}

pub fn node() -> Node {
    Node {
        name: "system-server".into(),
        deps: vec![
            Dep::on("image"),
            Dep::on("boot-image"),
            Dep::order_only("host/linux-run"),
        ],
        inputs: vec![repo(LIST)],
        outputs: vec![
            out().join("services.jar"),
            out().join("oat/arm64/services.odex"),
            out().join("oat/arm64/services.vdex"),
        ],
        tools: Vec::new(),
        recipe: 2,
        action: Action::SystemServer,
        boot: true,
    }
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
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
    for dir in ["data/boot/arm64", "data/out", "tmp"] {
        fs::create_dir_all(work.join(dir)).map_err(|e| e.to_string())?;
    }
    let jar = work.join("services.jar");
    patch_services_jar(&image.join(&JAR[1..]), &jar, &classes)?;
    log.line(&format!("not started: {}", classes.join(", ")));

    // The boot image to compile against, laid out as dex2oat finds it.
    let boot = aim_paths::boot_image().join("framework");
    for dir in [boot.join("arm64"), boot.clone()] {
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let from = entry.map_err(|e| e.to_string())?.path();
            if from.is_file() {
                let to = work.join("data/boot/arm64").join(from.file_name().unwrap());
                fs::copy(&from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
            }
        }
    }
    // The class path the image's own services.odex was compiled with.
    let original_odex = image.join(&ODEX[1..]);
    let bcp = oat_key(&original_odex, "bootclasspath")?;
    let dex2oat = Dex2oat::new(ctx, &image, &work, &[(JAR, jar.clone())])?;
    dex2oat.run(
        log,
        &[
            format!("--dex-file={JAR}"),
            format!("--dex-location={JAR}"),
            "--class-loader-context=PCL[/system/framework/com.android.location.provider.jar]"
                .into(),
            "--compiler-filter=verify".into(),
            "--runtime-arg".into(),
            format!("-Xbootclasspath:{bcp}"),
            "--runtime-arg".into(),
            format!("-Xbootclasspath-locations:{bcp}"),
            "--boot-image=/data/boot/boot.art".into(),
            "--oat-file=/data/out/services.odex".into(),
            format!("--oat-location={ODEX}"),
        ],
    )?;

    let staged = work.join("staged");
    fs::create_dir_all(staged.join("oat/arm64")).map_err(|e| e.to_string())?;
    fs::rename(&jar, staged.join("services.jar")).map_err(|e| e.to_string())?;
    for name in ["services.odex", "services.vdex"] {
        fs::rename(
            work.join("data/out").join(name),
            staged.join("oat/arm64").join(name),
        )
        .map_err(|e| format!("{name}: {e}"))?;
    }
    let _ = force_remove(&out);
    fs::rename(&staged, &out).map_err(|e| e.to_string())?;
    force_remove(&work).map_err(|e| e.to_string())
}
