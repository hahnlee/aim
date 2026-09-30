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
//!   Mac's SKU, with `userdata-<image>-<sku>.sha256`, the files it holds.
//!
//! A template is the original's own output: a first boot of the derived
//! image on a data directory made from the empty image, in window mode
//! (which shows only app tasks; a first-run task may still show a window
//! for a moment, #607), shut down as a device is
//! (`svc power shutdown`: PackageManagerService writes its settings, then
//! init's `sys.powerctl` ends the boot). The shipped paths are then copied
//! into a new empty image, not deleted from the used one, whose freed
//! blocks would still hold the boot's keys and seeds. The SKU is the one
//! the build boot's guest-init gave the device, read back from it.
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
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What a template holds of the build boot's data volume, relative to its
/// root (`data/` is the guest's `/data`): PackageManagerService's parser
/// cache, a parcel of each package it parsed, named after the image's
/// fingerprint and used while newer than its APK.
const SHIPPED: [&str; 1] = ["data/system/package_cache"];
const BOOT_PATIENCE: Duration = Duration::from_secs(300);
const SHUTDOWN_PATIENCE: Duration = Duration::from_secs(120);

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
        recipe: 1,
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
    let (boot, template) = (work.join("boot"), work.join("template"));
    for dir in [&boot, &template] {
        data::remove(dir)?;
    }
    let _ = force_remove(&work);
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    // The previous templates are of another derived image.
    for entry in fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("userdata-"))
        {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }

    let sku = first_boot(ctx, &work, &boot, log)?;
    let identity = aim_android_image::identity::read_tree_identity(&aim_paths::derived_image())?
        .ok_or("the derived image has no identity")?;
    let name = data::template_name(&identity, sku.as_deref());
    let from = DataImage::attach(&boot, None)?;
    let to = DataImage::attach(&template, Some(&out.join(EMPTY_TEMPLATE)))?;
    check(from.dir())?;
    let shipped: Vec<&Path> = SHIPPED.iter().map(Path::new).collect();
    let files = copy::copy_paths(from.dir(), to.dir(), &shipped)?;
    let mut manifest = String::new();
    for file in &files {
        let hash = crate::hash::sha256_file(&to.dir().join(file)).map_err(|e| e.to_string())?;
        writeln!(manifest, "{hash}  {}", file.display()).unwrap();
    }
    from.detach()?;
    to.detach()?;
    let listed = out.join(Path::new(&name).with_extension("sha256"));
    fs::write(&listed, manifest).map_err(|e| format!("{}: {e}", listed.display()))?;
    fs::rename(data::image_of(&template), out.join(&name)).map_err(|e| e.to_string())?;
    for dir in [&boot, &template] {
        data::remove(dir)?;
    }
    force_remove(&work).map_err(|e| e.to_string())?;
    log.line(&format!("{name}: {} files", files.len()));
    Ok(())
}

/// Boots the derived image on the new data directory `dir` to
/// `sys.boot_completed` and shuts it down; returns the SKU the boot had.
fn first_boot(ctx: &Ctx, work: &Path, dir: &Path, log: &mut Log) -> Result<Option<String>, String> {
    let output = fs::File::create(work.join("guest-init.log")).map_err(|e| e.to_string())?;
    let display = boot::start_display(ctx, work, true)?;
    let init = boot::guest_init(ctx, dir, &work.join("display"))
        .arg("--quiet")
        .stdout(output.try_clone().map_err(|e| e.to_string())?)
        .stderr(output)
        .spawn();
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
    let start = Instant::now();
    let getprop = |name: &str| {
        guest
            .run(&["/system/bin/getprop", name], Duration::from_secs(10))
            .map(|v| v.trim().to_string())
    };
    loop {
        session.check()?;
        if start.elapsed() > BOOT_PATIENCE {
            return Err(format!("no sys.boot_completed after {BOOT_PATIENCE:?}"));
        }
        if guest.path_map.exists() && getprop("sys.boot_completed")? == "1" {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    log.line(&format!(
        "build boot: sys.boot_completed after {:.1} s",
        start.elapsed().as_secs_f64()
    ));
    let sku = Some(getprop("ro.boot.product.vendor.sku")?).filter(|s| !s.is_empty());

    // The shutdown never answers: it ends with the boot.
    let mut request = shutdown(&guest, dir)?;
    let stopping = Instant::now();
    let status = loop {
        if let Some(status) = session.init.try_wait().map_err(|e| e.to_string())? {
            break Ok(status);
        }
        if stopping.elapsed() > SHUTDOWN_PATIENCE {
            break Err(format!("no shutdown after {SHUTDOWN_PATIENCE:?}"));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if matches!(request.try_wait(), Ok(None)) {
        // SAFETY: the process group of a child we started, not yet reaped.
        unsafe { libc::killpg(request.id() as i32, libc::SIGKILL) };
    }
    let _ = request.wait();
    drop(session);
    let status = status?;
    if !status.success() {
        return Err(format!(
            "the build boot ended with {status} (see {})",
            work.join("guest-init.log").display()
        ));
    }
    log.line(&format!(
        "build boot: shut down in {:.1} s",
        stopping.elapsed().as_secs_f64()
    ));
    Ok(sku)
}

/// Asks the guest of `guest` on the data directory `dir` to shut down, as
/// `adb shell svc power shutdown` does: the image's shell with init's
/// environment (`<data>.run/environ`, which app_process needs), the boot's
/// binder and process table (tools/guest-shell.sh).
fn shutdown(guest: &Guest, dir: &Path) -> Result<Child, String> {
    let runtime = data::runtime_of(dir);
    let environ = runtime.join("environ");
    let environ =
        fs::read_to_string(&environ).map_err(|e| format!("{}: {e}", environ.display()))?;
    Command::new(&guest.linux_run)
        .env_clear()
        .envs(environ.lines().filter_map(|l| l.split_once('=')))
        .arg("--inherit-env")
        .arg("--root")
        .arg(aim_paths::derived_image())
        .arg("--path-map")
        .arg(&guest.path_map)
        .args(["--binder", &guest.binder])
        .arg("--by-pid")
        .arg(runtime.join("identity/by-pid"))
        .args(["/system/bin/svc", "power", "shutdown"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("linux-run: {e}"))
}

/// The build boot's volume at `volume` holds what the template ships: one
/// parser cache, of the image's fingerprint.
fn check(volume: &Path) -> Result<(), String> {
    let cache = volume.join(SHIPPED[0]);
    let caches = fs::read_dir(&cache)
        .map_err(|e| format!("{}: {e}", cache.display()))?
        .count();
    if caches != 1 {
        return Err(format!("{}: {caches} caches, not one", cache.display()));
    }
    Ok(())
}
