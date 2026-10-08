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
//!   PackageManager's settings and decompressed stubs, boot dexopt's
//!   output, and the permission module's state (docs/first-boot.md, "What
//!   can be made at build time"). A first boot from it runs the original
//!   from that state: PackageManager and the permission module as on a
//!   repeat boot, every other owner as on a first boot. PackageManager's
//!   parser cache is not shipped: a parse resolves resource values for the
//!   device's locale, so the device's first boot makes its own (#722).
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The template boot's socket must fit in Darwin's `sockaddr_un.sun_path`
/// even when the build lives under a long worktree path.
struct DisplaySocketDir(PathBuf);

impl DisplaySocketDir {
    fn new() -> Result<Self, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aim-template-{}-{now}", std::process::id()));
        fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for DisplaySocketDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// PackageManagerService's parser cache, named after the image's
/// fingerprint: not shipped, as its entries hold resource values resolved
/// for the build boot's locale (`<meta-data>` strings), which the original
/// never re-resolves (#722).
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
        recipe: 3,
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

    let sku = first_boot(ctx, &work, &boot, &settings, &permissions, log, None)?;
    let identity = aim_android_image::identity::read_tree_identity(&aim_paths::derived_image())?
        .ok_or("the derived image has no identity")?;
    let name = data::template_name(&identity, sku.as_deref());
    let from = DataImage::attach(&boot, None)?;
    let mut volume_files = vec![PathBuf::from(DALVIK_CACHE)];
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
        &out.join(EMPTY_TEMPLATE),
    )?;
    from.detach()?;
    data::remove(&boot)?;
    force_remove(&work).map_err(|e| e.to_string())?;
    Ok(())
}

/// Explicit existing inputs and an exclusively owned output; never a graph build.
pub struct IsolatedTemplate {
    pub output: PathBuf,
    pub image: PathBuf,
    pub host_runtime: PathBuf,
    pub display: PathBuf,
    pub empty: PathBuf,
}

fn canonical_future(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return std::env::current_dir().map_err(|error| error.to_string());
    }
    if path.exists() {
        return fs::canonicalize(path).map_err(|error| error.to_string());
    }
    let parent = path.parent().ok_or("output has no existing ancestor")?;
    let name = path.file_name().ok_or("output has no name")?;
    Ok(canonical_future(parent)?.join(name))
}

fn isolated_output(config: &IsolatedTemplate) -> Result<PathBuf, String> {
    let output = canonical_future(&config.output)?;
    for input in [
        out(),
        config.image.clone(),
        config.host_runtime.clone(),
        config.display.clone(),
        config.empty.clone(),
    ] {
        let input = canonical_future(&input)?;
        if output.starts_with(&input) || input.starts_with(&output) {
            return Err(format!(
                "isolated template output {} overlaps protected input {}",
                output.display(),
                input.display()
            ));
        }
    }
    Ok(output)
}

fn template_inputs(config: &IsolatedTemplate) -> Result<serde_json::Value, String> {
    let hash = |path: &Path| {
        crate::hash::sha256_file(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    Ok(serde_json::json!({
        "image": fs::canonicalize(&config.image).map_err(|error| error.to_string())?,
        "identity": aim_android_image::identity::read_tree_identity(&config.image)?.ok_or("template image has no identity")?,
        "overlay_receipt_sha256": hash(&config.image.join(".overlay-receipt"))?,
        "host_runtime": fs::canonicalize(&config.host_runtime).map_err(|error| error.to_string())?,
        "guest_init_sha256": hash(&config.host_runtime.join("guest-init"))?,
        "linux_run_sha256": hash(&config.host_runtime.join("linux-run"))?,
        "display_sha256": hash(&config.display)?, "empty_sha256": hash(&config.empty)?,
    }))
}

pub fn run_isolated_template(ctx: &Ctx, config: &IsolatedTemplate) -> Result<(), String> {
    let output = isolated_output(config)?;
    let canonical = IsolatedTemplate {
        output: output.clone(),
        image: fs::canonicalize(&config.image).map_err(|error| error.to_string())?,
        host_runtime: fs::canonicalize(&config.host_runtime).map_err(|error| error.to_string())?,
        display: fs::canonicalize(&config.display).map_err(|error| error.to_string())?,
        empty: fs::canonicalize(&config.empty).map_err(|error| error.to_string())?,
    };
    let config = &canonical;
    if output.exists() {
        return Err(format!("{}: isolated output must be new", output.display()));
    }
    for input in [
        &config.empty,
        &config.display,
        &config.host_runtime.join("guest-init"),
        &config.host_runtime.join("linux-run"),
    ] {
        if !input.is_file() {
            return Err(format!(
                "missing existing template input {}",
                input.display()
            ));
        }
    }
    let _image_lease = aim_storage::system::ImageLease::read_root(&config.image)?;
    let provenance = template_inputs(config)?;
    let identity = provenance["identity"]
        .as_str()
        .ok_or("template identity missing")?
        .to_owned();
    fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
    fs::create_dir(&output).map_err(|error| error.to_string())?;
    let mut log = Log::create(output.join("build.log"), true)?;
    fs::write(
        output.join("inputs-before.json"),
        serde_json::to_vec_pretty(&provenance).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let inputs = output.join("inputs");
    fs::create_dir(&inputs).map_err(|error| error.to_string())?;
    fs::copy(&config.empty, inputs.join(EMPTY_TEMPLATE)).map_err(|error| error.to_string())?;
    let work = output.join("work");
    let settings = work.join("settings");
    let permissions = work.join("permissions");
    let data = work.join("boot");
    fs::create_dir_all(&settings).map_err(|error| error.to_string())?;
    let sku = first_boot(
        ctx,
        &work,
        &data,
        &settings,
        &permissions,
        &mut log,
        Some(config),
    )?;
    let name = data::template_name(&identity, sku.as_deref());
    let from = DataImage::attach(&data, None)?;
    let mut volume_files = vec![PathBuf::from(DALVIK_CACHE)];
    volume_files.extend(check(from.dir(), &settings)?);
    let settings_files: Vec<_> = SETTINGS
        .iter()
        .flat_map(|(file, reserve)| [Some(*file), *reserve])
        .flatten()
        .map(PathBuf::from)
        .filter(|path| settings.join(path).exists())
        .collect();
    let permission_files: Vec<_> = PERMISSIONS.iter().map(PathBuf::from).collect();
    if template_inputs(config)? != provenance {
        return Err("isolated template inputs changed during boot".into());
    }
    publish(
        &[
            (from.dir(), &volume_files),
            (&settings, &settings_files),
            (&permissions, &permission_files),
        ],
        &work,
        &output,
        &name,
        &mut log,
        &inputs.join(EMPTY_TEMPLATE),
    )?;
    from.detach()?;
    fs::write(
        output.join("provenance.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "inputs": provenance, "sku": sku, "template": name,
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    // Preserve the boot log and constructor captures as gate evidence.
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
    empty: &Path,
) -> Result<(), String> {
    let template = work.join("template");
    let to = DataImage::attach(&template, Some(empty))?;
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
    isolated: Option<&IsolatedTemplate>,
) -> Result<Option<String>, String> {
    let output = fs::File::create(work.join("guest-init.log")).map_err(|e| e.to_string())?;
    let display_dir = DisplaySocketDir::new()?;
    let display = match isolated {
        Some(config) => boot::start_display_binary(&config.display, &display_dir.0, true)?,
        None => boot::start_display(ctx, &display_dir.0, true)?,
    };
    let mut command = match isolated {
        Some(config) => {
            let mut command = std::process::Command::new(config.host_runtime.join("guest-init"));
            command
                .arg("--image")
                .arg(&config.image)
                .arg("--data")
                .arg(dir)
                .arg("--run")
                .arg("--gpu")
                .arg(aim_paths::angle())
                .arg("--vulkan")
                .arg(aim_paths::moltenvk())
                .arg("--display")
                .arg(display_dir.0.join("display"))
                .arg("--userdata")
                .arg(config.output.join("inputs"));
            command
        }
        None => boot::guest_init(ctx, dir, &display_dir.0.join("display")),
    };
    let init = command
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
        linux_run: isolated.map_or_else(
            || ctx.workspace.host_bin("linux-run"),
            |config| config.host_runtime.join("linux-run"),
        ),
        path_map: data::runtime_of(dir).join("path-map"),
        binder: format!("dev.aim.guest-init.{}.binder", session.init.id()),
        data: dir.join("data"),
    };
    let getprop = |name: &str| {
        guest
            .run(&["/system/bin/getprop", name], Duration::from_secs(10))
            .map(|v| v.trim().to_string())
    };
    if isolated.is_some() {
        log.line(&format!(
            "owned template guest-init pid={}, display pid={}",
            session.init.id(),
            session.display.id()
        ));
    }
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
    if isolated.is_some() {
        let events = guest.run(
            &[
                "/system/bin/logcat",
                "-d",
                "-b",
                "events",
                "-v",
                "threadtime",
            ],
            Duration::from_secs(30),
        )?;
        fs::write(work.join("first-boot-events.log"), events).map_err(|error| error.to_string())?;
        log.line(&format!(
            "owned template guest-init pid={}, display pid={}",
            session.init.id(),
            session.display.id()
        ));
    }
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
/// what a template may ship: settings of the image's fingerprint (that of
/// the boot's one parser cache) that list only the image's packages
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
    let root = fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| aim_android_xml::read(&b))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let problem = |what: String| Err(format!("{}: {what}", path.display()));
    let top = |name: &'static str| root.children().filter(move |e| e.name == name);
    let recorded = top("version")
        .find(|v| v.attr("volumeUuid").is_none())
        .and_then(|v| v.string("fingerprint"));
    if recorded.as_deref() != Some(fingerprint.as_str()) {
        return problem(format!(
            "fingerprint {recorded:?}, the cache's {fingerprint}"
        ));
    }
    if top("verifier").next().is_some() {
        return problem("a verifier identity".into());
    }
    let updated: Vec<_> = top("updated-package")
        .filter_map(|p| p.string("name"))
        .collect();
    let mut stubs = Vec::new();
    for package in top("package") {
        let (Some(name), Some(code)) = (package.string("name"), package.string("codePath")) else {
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

#[cfg(test)]
mod isolation_tests {
    use super::*;
    #[test]
    fn protected_inputs_and_symlink_aliases_cannot_be_outputs() {
        let directory =
            std::env::temp_dir().join(format!("aim-template-isolation-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let image = directory.join("image");
        fs::create_dir(&image).unwrap();
        let alias = directory.join("alias");
        std::os::unix::fs::symlink(&image, &alias).unwrap();
        let mut config = IsolatedTemplate {
            output: directory.join("new"),
            image: image.clone(),
            host_runtime: directory.join("host"),
            display: directory.join("display"),
            empty: directory.join("empty.asif"),
        };
        assert!(isolated_output(&config).is_ok());
        config.output = out().join("must-not-create");
        assert!(isolated_output(&config).is_err());
        config.output = image.join("nested");
        assert!(isolated_output(&config).is_err());
        config.output = alias.join("nested");
        assert!(isolated_output(&config).is_err());
        config.output = directory.clone();
        assert!(isolated_output(&config).is_err());
        assert!(!image.join("nested").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
