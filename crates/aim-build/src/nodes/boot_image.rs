//! `boot-image`: the boot image for the ART exception (ADR 0012 decision 4,
//! docs/art-exception-patches.md, #165): the image's own boot class path,
//! compiled by the rebuilt dex2oat64 running on the Linux syscall layer
//! (linux-run) with the original linker64, bionic and ART APEX libraries.
//!
//! It mirrors the image's boot image: the primary boot image for the BCP
//! recorded in the original boot.oat (multi-image, speed-profile with the
//! image's boot profiles, dirty-image-objects and preloaded-classes, at
//! ART_BASE_ADDRESS), plus every mainline boot image extension the image
//! ships (verify, over the jars its original records), compiled against it.
//! An oat file compiled against the boot image is only valid when the
//! images cover its whole boot class path. The output is what
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
        recipe: 2,
        action: Action::BootImage,
        boot: true,
    }
}

/// A key of an oat file's header (`key\0value\0` pairs; `classpath` is
/// not the end of `bootclasspath`).
pub(super) fn oat_key(oat: &Path, key: &str) -> Result<String, String> {
    let data = fs::read(oat).map_err(|e| format!("{}: {e}", oat.display()))?;
    let needle = format!("\0{key}\0").into_bytes();
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

/// The dex locations an oat file holds, in order, without multidex entries:
/// each `OatDexFile` starts with its location's length and the location.
pub(super) fn oat_dex_locations(oat: &Path) -> Result<Vec<String>, String> {
    let data = fs::read(oat).map_err(|e| format!("{}: {e}", oat.display()))?;
    let mut found = Vec::new();
    for at in 4..data.len() {
        if data[at] != b'/' {
            continue;
        }
        let len = u32::from_le_bytes(data[at - 4..at].try_into().unwrap()) as usize;
        let Some(location) = data.get(at..at + len.min(4096)) else {
            continue;
        };
        let printable = location.iter().all(|b| b.is_ascii_graphic());
        if printable && (location.ends_with(b".jar") || location.ends_with(b".apk")) {
            let location = String::from_utf8_lossy(location).into_owned();
            if !found.contains(&location) {
                found.push(location);
            }
        }
    }
    if found.is_empty() {
        return Err(format!("{}: no dex locations", oat.display()));
    }
    Ok(found)
}

/// The mainline boot image extensions the image ships: the stems of its
/// `boot-<stem>.art` files beyond the primary image's.
fn extension_stems(image: &Path) -> Result<Vec<String>, String> {
    let framework = image.join("system/framework");
    let bcp = oat_key(&framework.join("arm64/boot.oat"), "bootclasspath")?;
    let primary: Vec<&str> = bcp.split(':').map(stem).collect();
    let mut stems: Vec<String> = fs::read_dir(framework.join("arm64"))
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_prefix("boot-")?.strip_suffix(".art")?;
            (!primary.contains(&stem)).then(|| stem.to_string())
        })
        .collect();
    stems.sort();
    Ok(stems)
}

/// The boot image the runtime loads by default (ART's
/// `GetDefaultBootImageLocation`): the primary image and its extensions.
pub(super) fn boot_image_location(image: &Path) -> Result<String, String> {
    let mut location = String::from("/system/framework/boot.art");
    for stem in extension_stems(image)? {
        location.push_str(&format!(":/system/framework/boot-{stem}.art"));
    }
    Ok(location)
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".jar").unwrap_or(name)
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    // Never a link: the tree is read-only input.
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let framework = image.join("system/framework");
    let bcp = oat_key(&framework.join("arm64/boot.oat"), "bootclasspath")?;
    let jars: Vec<&str> = bcp.split(':').collect();

    // An extension is named after its first jar and holds the jars its
    // original oat file holds (the image's one extension: every mainline
    // BCP jar), compiled over the class path that oat file records.
    let stems = extension_stems(&image)?;
    let mut extensions: Vec<(String, Vec<String>)> = Vec::new();
    for stem in &stems {
        let oat = framework.join(format!("arm64/boot-{stem}.oat"));
        let base = oat_key(&oat, "bootclasspath")?;
        extensions.push((base, oat_dex_locations(&oat)?));
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
    let dex2oat = Dex2oat::new(ctx, &image, &work, &[])?;
    let dex2oat = |log: &mut Log, args: &[String]| dex2oat.run(log, args);

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

    // An extension's image is named after the base, "boot", plus its first
    // jar's stem. Without a profile it holds every class of its jars, as
    // the original does.
    for (base, jars) in &extensions {
        let all = format!("{base}:{}", jars.join(":"));
        let mut args: Vec<String> = jars
            .iter()
            .flat_map(|jar| [format!("--dex-file={jar}"), format!("--dex-location={jar}")])
            .collect();
        args.extend([
            "--single-image".into(),
            "--compiler-filter=verify".into(),
            "--runtime-arg".into(),
            format!("-Xbootclasspath:{all}"),
            "--runtime-arg".into(),
            format!("-Xbootclasspath-locations:{all}"),
            "--boot-image=/data/out/boot.art".into(),
            "--oat-file=/data/ext/arm64/boot.oat".into(),
            "--oat-location=/system/framework/arm64/boot.oat".into(),
            "--image=/data/ext/arm64/boot.art".into(),
        ]);
        dex2oat(log, &args)?;
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

/// The rebuilt dex2oat64 on the syscall layer, in a guest view of the
/// original image with the ART exception binaries in place of the ART
/// APEX's, a writable `/data` and `/tmp` under `work`, and `files`
/// (guest path, host path) mapped over the image.
pub(super) struct Dex2oat {
    linux_run: PathBuf,
    work: PathBuf,
}

impl Dex2oat {
    pub fn new(
        ctx: &Ctx,
        image: &Path,
        work: &Path,
        files: &[(&str, PathBuf)],
    ) -> Result<Dex2oat, String> {
        let art = aim_paths::art().join("stripped");
        let mut map = format!(
            "root\t/\t{}\nrw\t/data\t{}\nrw\t/tmp\t{}\n",
            image.display(),
            work.join("data").display(),
            work.join("tmp").display()
        );
        let mut entries: Vec<(String, PathBuf)> = [
            "libart",
            "libartbase",
            "libdexfile",
            "libprofile",
            "libopenjdkjvm",
        ]
        .iter()
        .map(|lib| {
            (
                format!("/apex/com.android.art/lib64/{lib}.so"),
                art.join(format!("lib64/{lib}.so")),
            )
        })
        .collect();
        entries.push((
            "/apex/com.android.art/bin/dex2oat64".into(),
            art.join("bin/dex2oat64"),
        ));
        entries.extend(files.iter().map(|(g, h)| (g.to_string(), h.clone())));
        for (guest, host) in entries {
            map.push_str(&format!("rw\t{guest}\t{}\n", host.display()));
        }
        fs::write(work.join("path-map"), map).map_err(|e| e.to_string())?;
        Ok(Dex2oat {
            linux_run: ctx.workspace.host_bin("linux-run"),
            work: work.to_path_buf(),
        })
    }

    /// The image's (or the exception's) ART tool `binary` on the syscall
    /// layer. Without a generated linker configuration linker64 uses its
    /// default namespace, which LD_LIBRARY_PATH points at the APEXes libart
    /// links.
    fn command(&self, binary: &str) -> Command {
        let mut command = Command::new(&self.linux_run);
        command
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
            .arg(self.work.join("path-map"))
            .arg("--cache")
            .arg(self.work.join("cache"))
            .arg(binary);
        command
    }

    /// Runs dex2oat64 with the common arguments and `args`.
    pub fn run(&self, log: &mut Log, args: &[String]) -> Result<(), String> {
        log.run(
            self.command("/apex/com.android.art/bin/dex2oat64")
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
    }

    /// Runs the image's profman with `args`; returns its standard output.
    pub fn profman(&self, log: &mut Log, args: &[String]) -> Result<String, String> {
        log.output(self.command("/apex/com.android.art/bin/profman").args(args))
    }
}
