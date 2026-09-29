//! `boot-image`: the boot image for the ART exception (ADR 0012 decision 4,
//! docs/art-exception-patches.md, #165): the image's own boot class path,
//! compiled by the rebuilt dex2oat64 running on the Linux syscall layer
//! (linux-run) with the original linker64, bionic and ART APEX libraries.
//!
//! It mirrors the image's boot image: the primary boot image for the BCP
//! recorded in the original boot.oat (multi-image, speed-profile with the
//! image's boot profiles, dirty-image-objects and preloaded-classes, at
//! ART_BASE_ADDRESS), plus every mainline boot image extension the image
//! ships (verify), compiled against it. The output is what
//! image/overlay.toml replaces:
//!   framework/arm64/boot*.{art,oat}  ->  /system/framework/arm64/
//!   framework/boot*.vdex             ->  /system/framework/
//!
//! linux-run is an order-only dependency: the output is deterministic
//! (`--force-determinism`), whatever the syscall layer's version.

use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn node() -> Node {
    Node {
        name: "boot-image".into(),
        deps: vec![
            Dep::on("art"),
            Dep::on("image"),
            Dep::order_only("host/linux-run"),
        ],
        inputs: Vec::new(),
        outputs: vec![aim_paths::boot_image().join("framework")],
        tools: Vec::new(),
        recipe: 1,
        action: Action::BootImage,
        boot: true,
    }
}

/// A key of the original boot.oat's header (`key\0value\0` pairs).
fn oat_key(oat: &Path, key: &str) -> Result<String, String> {
    let data = fs::read(oat).map_err(|e| format!("{}: {e}", oat.display()))?;
    let mut needle = key.as_bytes().to_vec();
    needle.push(0);
    let at = data
        .windows(needle.len())
        .position(|w| w == needle)
        .ok_or_else(|| format!("{}: no {key}", oat.display()))?
        + needle.len();
    let end = data[at..]
        .iter()
        .position(|&b| b == 0)
        .ok_or("unterminated value")?;
    Ok(String::from_utf8_lossy(&data[at..at + end]).into_owned())
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".jar").unwrap_or(name)
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    // Never a link: the tree is read-only input.
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let framework = image.join("system/framework");
    let art = aim_paths::art().join("stripped");
    let linux_run = ctx.workspace.host_bin("linux-run");
    let bcp = oat_key(&framework.join("arm64/boot.oat"), "bootclasspath")?;
    let jars: Vec<&str> = bcp.split(':').collect();

    // Mainline extensions: boot-<stem>.art files beyond the primary image.
    let primary: Vec<&str> = jars.iter().map(|j| stem(j)).collect();
    let mut arts: Vec<PathBuf> = fs::read_dir(framework.join("arm64"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("boot-") && name.ends_with(".art")
        })
        .collect();
    arts.sort();
    let mut extensions = Vec::new();
    for art_file in arts {
        let name = art_file.file_stem().unwrap().to_string_lossy();
        let stem = &name["boot-".len()..];
        if primary.contains(&stem) {
            continue;
        }
        let mut candidates: Vec<PathBuf> = fs::read_dir(image.join("apex"))
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|apex| apex.path().join("javalib").join(format!("{stem}.jar")))
            .collect();
        candidates.sort();
        candidates.push(image.join(format!("system/framework/{stem}.jar")));
        let jar = candidates
            .into_iter()
            .find(|c| c.is_file())
            .ok_or_else(|| format!("no jar for boot image extension {stem}"))?;
        extensions.push(format!("/{}", jar.strip_prefix(&image).unwrap().display()));
    }

    // The guest view: the original image with the ART exception binaries in
    // place of the ART APEX's, and a writable /data and /tmp. The output is
    // staged in the work tree and replaces the previous one only once
    // complete, so a failed rebuild keeps it.
    let out = aim_paths::boot_image();
    let work = out.with_extension("work");
    // dex2oat's linux-run records load-time sites in the work cache, whose
    // entries are read-only.
    let _ = force_remove(&work);
    for dir in ["data/out/arm64", "data/ext/arm64", "tmp"] {
        fs::create_dir_all(work.join(dir)).map_err(|e| e.to_string())?;
    }
    let mut map = format!(
        "root\t/\t{}\nrw\t/data\t{}\nrw\t/tmp\t{}\n",
        image.display(),
        work.join("data").display(),
        work.join("tmp").display()
    );
    for lib in [
        "libart",
        "libartbase",
        "libdexfile",
        "libprofile",
        "libopenjdkjvm",
    ] {
        map.push_str(&format!(
            "rw\t/apex/com.android.art/lib64/{lib}.so\t{}\n",
            art.join(format!("lib64/{lib}.so")).display()
        ));
    }
    map.push_str(&format!(
        "rw\t/apex/com.android.art/bin/dex2oat64\t{}\n",
        art.join("bin/dex2oat64").display()
    ));
    fs::write(work.join("path-map"), map).map_err(|e| e.to_string())?;

    // Without a generated linker configuration linker64 uses its default
    // namespace, which LD_LIBRARY_PATH points at the APEXes libart links.
    let dex2oat = |log: &mut Log, args: &[String]| {
        log.run(
            Command::new(&linux_run)
                .env_clear()
                .env(
                    "LD_LIBRARY_PATH",
                    "/apex/com.android.art/lib64:/apex/com.android.i18n/lib64:/apex/com.android.os.statsd/lib64",
                )
                .env("ANDROID_ROOT", "/system")
                .env("ANDROID_DATA", "/data")
                .env("ANDROID_ART_ROOT", "/apex/com.android.art")
                .env("ANDROID_I18N_ROOT", "/apex/com.android.i18n")
                .arg("--inherit-env")
                .arg("--path-map")
                .arg(work.join("path-map"))
                .arg("--cache")
                .arg(work.join("cache"))
                .arg("/apex/com.android.art/bin/dex2oat64")
                .args([
                    "--runtime-arg",
                    "-Xms64m",
                    "--runtime-arg",
                    "-Xmx1024m",
                    "--instruction-set=arm64",
                    "--instruction-set-features=default",
                    "--image-format=lz4",
                    "--force-determinism",
                    "--generate-build-id",
                    "--avoid-storing-invocation",
                    "--compilation-reason=prebuilt",
                    "--android-root=/system",
                ])
                .args(args),
        )
    };

    let mut primary_args: Vec<String> = jars
        .iter()
        .flat_map(|jar| [format!("--dex-file={jar}"), format!("--dex-location={jar}")])
        .collect();
    primary_args.extend(
        [
            "--profile-file=/apex/com.android.art/etc/boot-image.prof",
            "--profile-file=/system/etc/boot-image.prof",
            "--dirty-image-objects=/apex/com.android.art/etc/dirty-image-objects",
            "--dirty-image-objects=/system/etc/dirty-image-objects",
            "--preloaded-classes=/system/etc/preloaded-classes",
            "--compiler-filter=speed-profile",
            "--resolve-startup-const-strings=true",
            "--abort-on-hard-verifier-error",
            "--base=0x70000000",
            "--oat-file=/data/out/arm64/boot.oat",
            "--oat-location=/system/framework/arm64/boot.oat",
            "--image=/data/out/arm64/boot.art",
        ]
        .map(String::from),
    );
    dex2oat(log, &primary_args)?;

    // An extension's image is named after the base, "boot", plus its jar's
    // stem.
    for jar in &extensions {
        dex2oat(
            log,
            &[
                format!("--dex-file={jar}"),
                format!("--dex-location={jar}"),
                "--compiler-filter=verify".into(),
                "--runtime-arg".into(),
                format!("-Xbootclasspath:{bcp}:{jar}"),
                "--runtime-arg".into(),
                format!("-Xbootclasspath-locations:{bcp}:{jar}"),
                "--boot-image=/data/out/boot.art".into(),
                "--oat-file=/data/ext/arm64/boot.oat".into(),
                "--oat-location=/system/framework/arm64/boot.oat".into(),
                "--image=/data/ext/arm64/boot.art".into(),
            ],
        )?;
        for entry in fs::read_dir(work.join("data/ext/arm64")).map_err(|e| e.to_string())? {
            let from = entry.map_err(|e| e.to_string())?.path();
            fs::rename(
                &from,
                work.join("data/out/arm64").join(from.file_name().unwrap()),
            )
            .map_err(|e| e.to_string())?;
        }
    }

    let staged = work.join("framework");
    fs::create_dir_all(staged.join("arm64")).map_err(|e| e.to_string())?;
    let mut files = 0;
    for entry in fs::read_dir(work.join("data/out/arm64")).map_err(|e| e.to_string())? {
        let from = entry.map_err(|e| e.to_string())?.path();
        let name = from.file_name().unwrap().to_owned();
        let to = match from.extension().and_then(|e| e.to_str()) {
            Some("art" | "oat") => staged.join("arm64").join(name),
            Some("vdex") => staged.join(name),
            _ => continue,
        };
        fs::rename(&from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
        files += 1;
    }
    // The previous output moves into the work tree, which goes last.
    let dest = out.join("framework");
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    if dest.exists() {
        fs::rename(&dest, work.join("previous")).map_err(|e| e.to_string())?;
    }
    fs::rename(&staged, &dest).map_err(|e| e.to_string())?;
    force_remove(&work).map_err(|e| e.to_string())?;
    log.line(&format!(
        "boot image: {} ({files} files for {} BCP jars + {} extension(s))",
        out.display(),
        jars.len(),
        extensions.len()
    ));
    Ok(())
}
