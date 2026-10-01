//! `userdata/*` (after AOSP's data partition image): the data templates
//! in `target/aim/userdata` a new data directory starts from
//! (docs/first-boot.md). guest-init clones one (`--userdata`,
//! `aim_storage::data::template`) instead of making the data image:
//!
//! - `userdata/empty`: `empty.asif`, an empty data image: an instant APFS
//!   clone where making one costs about half a second of a first boot
//!   (#563);
//! - `userdata/template`: `userdata-<image>-<sku>.asif`, what the
//!   original's first boot of the derived image writes that depends only
//!   on the image and the SKU, for the derived image's identity and this
//!   Mac's SKU, with `userdata-<image>-<sku>.sha256`, the files it holds:
//!   PackageManager's parser cache, settings and decompressed stubs, boot
//!   dexopt's output, and the permission module's state (docs/first-boot.md,
//!   "What can be made at build time"). A first boot from it runs the
//!   original from that state: PackageManager and the permission module as
//!   on a repeat boot, every other owner as on a first boot.
//!
//! A template is the original's own output: a first boot of the derived
//! image on a data directory made from the empty image, in window mode
//! (which shows only app tasks; a first-run task may still show a window
//! for a moment, #607). PackageManagerService's settings are taken as its
//! constructor writes them (`write settings`, before `pms_ready`), when no
//! app has run yet: a later write would hold what apps did in their first
//! seconds (components they enable, packages they start), which differs
//! from boot to boot. The permission module's state is taken once the role
//! controller has written roles and the module has been quiet for
//! [`QUIET`]. Everything else is taken once the boot has completed (boot
//! dexopt ends before `ams_ready`) and been stopped. The shipped paths are
//! copied into a new empty image, not deleted from the used one, whose
//! freed blocks would still hold the boot's keys and seeds. The SKU is the
//! one the build boot's guest-init gave the device, read back from it.
//!
//! The template depends on the derived image, not on the runtime that
//! boots it (as `oat` on `linux-run`): its content is the original's
//! output. A Mac of another SKU boots from the empty image until the node
//! runs there.

use crate::bench::{Guest, Session};
use crate::boot;
use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use aim_storage::copy;
use aim_storage::data::{self, DataImage, EMPTY_TEMPLATE};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// PackageManagerService's parser cache: a parcel of each package it
/// parsed, named after the image's fingerprint and used while newer than
/// its APK.
const CACHE: &str = "data/system/package_cache";
/// What first-boot dexopt compiled for the APKs inside APEXes.
const DALVIK_CACHE: &str = "data/dalvik-cache/arm64";
const PACKAGES: &str = "data/system/packages.xml";
/// PackageManager's settings, user 0's package state and the uid list, as
/// a first boot's PackageManagerService writes them, each file with its
/// reserve copy (`ResilientAtomicFile`), written once the file is. Paths
/// are relative to the data volume's root (`data/` is the guest's
/// `/data`).
const SETTINGS: [(&str, Option<&str>); 3] = [
    (PACKAGES, Some("data/system/packages.xml.reservecopy")),
    (
        "data/system/users/0/package-restrictions.xml",
        Some("data/system/users/0/package-restrictions.xml.reservecopy"),
    ),
    ("data/system/packages.list", None),
];
/// A settings file being written: `ResilientAtomicFile` moves the previous
/// one here first.
const WRITING: [&str; 2] = [
    "data/system/packages-backup.xml",
    "data/system/users/0/package-restrictions-backup.xml",
];
/// The permission module's state of the device and of user 0, each file
/// with its reserve copy: the access service's `access.abx` (runtime
/// permissions, their flags and app ops), the default grants' fingerprint
/// in `runtime-permissions.xml`, and `roles.xml`. A first boot from the
/// template does not make a system package's permission state as the
/// package is added, so without it the default grants and restricted
/// permission exemptions would be missing (docs/first-boot.md, "Checks").
const PERMISSIONS: [&str; 8] = [
    "data/misc/apexdata/com.android.permission/access.abx",
    "data/misc/apexdata/com.android.permission/access.abx.reservecopy",
    "data/misc_de/0/apexdata/com.android.permission/access.abx",
    "data/misc_de/0/apexdata/com.android.permission/access.abx.reservecopy",
    "data/misc_de/0/apexdata/com.android.permission/runtime-permissions.xml",
    "data/misc_de/0/apexdata/com.android.permission/runtime-permissions.xml.reservecopy",
    "data/misc_de/0/apexdata/com.android.permission/roles.xml",
    "data/misc_de/0/apexdata/com.android.permission/roles.xml.reservecopy",
];
const QUIET: Duration = Duration::from_secs(3);
const BOOT_PATIENCE: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(50);

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

pub fn template() -> Node {
    Node {
        name: "userdata/template".into(),
        deps: vec![
            Dep::on("derived-image"),
            Dep::order_only("userdata/empty"),
            Dep::order_only("translation-cache"),
            Dep::order_only("host/guest-init"),
            Dep::order_only("host/linux-run"),
            Dep::order_only("host/aim-display"),
            Dep::order_only("host/aim-binderd"),
        ],
        inputs: Vec::new(),
        // Named after the SKU, which the build boot names.
        outputs: Vec::new(),
        tools: Vec::new(),
        recipe: 2,
        action: Action::UserdataTemplate,
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

pub fn run_template(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    let out = out();
    let work = aim_paths::out().join("userdata-template.work");
    let (boot, settings) = (work.join("boot"), work.join("settings"));
    let permissions = work.join("permissions");
    for dir in [&boot, &work.join("template")] {
        data::remove(dir)?;
    }
    let _ = force_remove(&work);
    fs::create_dir_all(&settings).map_err(|e| format!("{}: {e}", settings.display()))?;
    // The previous templates are of another derived image.
    if let Ok(entries) = fs::read_dir(&out) {
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("userdata-"))
            {
                fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }

    let sku = first_boot(ctx, &work, &boot, &settings, &permissions, log)?;
    let identity = aim_android_image::identity::read_tree_identity(&aim_paths::derived_image())?
        .ok_or("the derived image has no identity")?;
    let name = data::template_name(&identity, sku.as_deref());
    let from = DataImage::attach(&boot, None)?;
    let mut volume_files = vec![PathBuf::from(CACHE), PathBuf::from(DALVIK_CACHE)];
    volume_files.extend(check(from.dir(), &settings)?);
    let settings_files: Vec<PathBuf> = SETTINGS
        .iter()
        .flat_map(|(file, reserve)| [Some(*file), *reserve])
        .flatten()
        .map(PathBuf::from)
        .filter(|p| settings.join(p).exists())
        .collect();
    let permission_files: Vec<PathBuf> = PERMISSIONS.iter().map(PathBuf::from).collect();
    publish(
        &[
            (from.dir(), &volume_files),
            (&settings, &settings_files),
            (&permissions, &permission_files),
        ],
        &work,
        &out,
        &name,
        log,
    )?;
    from.detach()?;
    data::remove(&boot)?;
    force_remove(&work).map_err(|e| e.to_string())?;
    Ok(())
}

/// Copies `paths` of each tree into a new clone of the empty image and
/// publishes it as `dest/name`, with the list of its files.
fn publish(
    sources: &[(&Path, &[PathBuf])],
    work: &Path,
    dest: &Path,
    name: &str,
    log: &mut Log,
) -> Result<(), String> {
    let template = work.join("template");
    let to = DataImage::attach(&template, Some(&out().join(EMPTY_TEMPLATE)))?;
    let mut files = Vec::new();
    for (from, paths) in sources {
        let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        files.extend(copy::copy_paths(from, to.dir(), &paths)?);
    }
    files.sort();
    let mut manifest = String::new();
    for file in &files {
        let hash = crate::hash::sha256_file(&to.dir().join(file)).map_err(|e| e.to_string())?;
        writeln!(manifest, "{hash}  {}", file.display()).unwrap();
    }
    to.detach()?;
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let listed = dest.join(Path::new(name).with_extension("sha256"));
    fs::write(&listed, manifest).map_err(|e| format!("{}: {e}", listed.display()))?;
    fs::rename(data::image_of(&template), dest.join(name)).map_err(|e| e.to_string())?;
    data::remove(&template)?;
    log.line(&format!(
        "{}: {} files",
        dest.join(name).display(),
        files.len()
    ));
    Ok(())
}

/// Boots the derived image on the new data directory `dir` to
/// `sys.boot_completed`, copying PackageManager's settings into `settings`
/// as they are first written and the permission module's state into
/// `permissions`, and stops it; returns the SKU the boot had.
fn first_boot(
    ctx: &Ctx,
    work: &Path,
    dir: &Path,
    settings: &Path,
    permissions: &Path,
    log: &mut Log,
) -> Result<Option<String>, String> {
    let output = fs::File::create(work.join("guest-init.log")).map_err(|e| e.to_string())?;
    let display = boot::start_display(ctx, work, true)?;
    let init = boot::guest_init(ctx, dir, &work.join("display"))
        .arg("--quiet")
        .stdout(output.try_clone().map_err(|e| e.to_string())?)
        .stderr(output)
        .spawn();
    // Dropping it stops the boot.
    let mut session = match init {
        Ok(init) => Session { display, init },
        Err(e) => {
            let mut display = display;
            let _ = display.kill();
            let _ = display.wait();
            return Err(format!("guest-init: {e}"));
        }
    };
    let guest = Guest {
        linux_run: ctx.workspace.host_bin("linux-run"),
        path_map: data::runtime_of(dir).join("path-map"),
        binder: format!("dev.aim.guest-init.{}.binder", session.init.id()),
        data: dir.join("data"),
    };
    let getprop = |name: &str| {
        guest
            .run(&["/system/bin/getprop", name], Duration::from_secs(10))
            .map(|v| v.trim().to_string())
    };
    let start = Instant::now();
    let settings_files: Vec<&str> = SETTINGS
        .iter()
        .flat_map(|(file, reserve)| [Some(*file), *reserve])
        .flatten()
        .collect();
    let (mut written, mut granted, mut completed, mut asked) = (None, None, None, start);
    while written.is_none() || granted.is_none() || completed.is_none() {
        session.check()?;
        if start.elapsed() > BOOT_PATIENCE {
            return Err(format!(
                "after {BOOT_PATIENCE:?}: settings {written:?}, permissions {granted:?}, \
                 sys.boot_completed {completed:?}"
            ));
        }
        if written.is_none() && take(dir, settings, &settings_files, &WRITING, Duration::ZERO)? {
            written = Some(start.elapsed());
        }
        if granted.is_none() && take(dir, permissions, &PERMISSIONS, &[], QUIET)? {
            granted = Some(start.elapsed());
        }
        if completed.is_none() && asked.elapsed() >= Duration::from_millis(500) {
            asked = Instant::now();
            if guest.path_map.exists() && getprop("sys.boot_completed")? == "1" {
                completed = Some(start.elapsed());
            }
        }
        std::thread::sleep(POLL);
    }
    log.line(&format!(
        "build boot: settings after {:.1} s, sys.boot_completed after {:.1} s, permissions after {:.1} s",
        written.unwrap_or_default().as_secs_f64(),
        completed.unwrap_or_default().as_secs_f64(),
        granted.unwrap_or_default().as_secs_f64(),
    ));
    let sku = Some(getprop("ro.boot.product.vendor.sku")?).filter(|s| !s.is_empty());
    drop(session);
    Ok(sku)
}

/// Copies `files` from the data volume at `dir` into `dest` once every
/// one of them is written, none is being written (`writing`) and none has
/// changed for `quiet`; whether it did.
fn take(
    dir: &Path,
    dest: &Path,
    files: &[&str],
    writing: &[&str],
    quiet: Duration,
) -> Result<bool, String> {
    let written = || -> Option<Vec<SystemTime>> {
        if writing.iter().any(|w| dir.join(w).exists()) {
            return None;
        }
        files
            .iter()
            .map(|f| fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok())
            .collect()
    };
    let Some(before) = written() else {
        return Ok(false);
    };
    let newest = before
        .iter()
        .max()
        .copied()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if newest.elapsed().unwrap_or_default() < quiet {
        return Ok(false);
    }
    let _ = force_remove(dest);
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let paths: Vec<&Path> = files.iter().map(Path::new).collect();
    // A write that began meanwhile fails the copy or changes a file: the
    // next poll takes the files again.
    let copied = copy::copy_paths(dir, dest, &paths).is_ok();
    Ok(copied && written() == Some(before))
}

/// The build boot's volume at `volume` and the settings taken from it hold
/// what a template may ship: one parser cache, and settings of the image's
/// fingerprint (that of the cache) that list only the image's packages
/// (the stubs' decompressed copies in `/data/app`, which update a system
/// package, and no package an installer added) and no verifier identity.
/// Returns the stubs' directories.
fn check(volume: &Path, settings: &Path) -> Result<Vec<PathBuf>, String> {
    let cache = volume.join(CACHE);
    let caches: Vec<String> = fs::read_dir(&cache)
        .map_err(|e| format!("{}: {e}", cache.display()))?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", cache.display()))?;
    let [fingerprint] = &caches[..] else {
        return Err(format!(
            "{}: {} caches, not one",
            cache.display(),
            caches.len()
        ));
    };
    let path = settings.join(PACKAGES);
    let elements =
        crate::abx::elements(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    let problem = |what: String| Err(format!("{}: {what}", path.display()));
    let top = |name: &'static str| {
        elements
            .iter()
            .filter(move |e| e.depth == 1 && e.name == name)
    };
    let recorded = top("version")
        .find(|v| v.attr("volumeUuid").is_none())
        .and_then(|v| v.attr("fingerprint"));
    if recorded != Some(fingerprint.as_str()) {
        return problem(format!(
            "fingerprint {recorded:?}, the cache's {fingerprint}"
        ));
    }
    if top("verifier").next().is_some() {
        return problem("a verifier identity".into());
    }
    let updated: Vec<&str> = top("updated-package")
        .filter_map(|p| p.attr("name"))
        .collect();
    let mut stubs = Vec::new();
    for package in top("package") {
        let (Some(name), Some(code)) = (package.attr("name"), package.attr("codePath")) else {
            return problem("a package without a name or code path".into());
        };
        let Some(dir) = code.strip_prefix("/data/app/") else {
            continue;
        };
        if !updated.contains(&name) {
            return problem(format!("{name} in {code}, which no system package is"));
        }
        let dir = Path::new("data/app").join(dir.split_once('/').map_or(dir, |(d, _)| d));
        if !volume.join(&dir).is_dir() {
            return problem(format!("{name}: no {}", dir.display()));
        }
        stubs.push(dir);
    }
    Ok(stubs)
}
