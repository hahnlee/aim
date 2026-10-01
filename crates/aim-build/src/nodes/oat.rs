//! `oat`: the image's compiled oat files, compiled again for the ART
//! exception (ADR 0012 decision 4).
//!
//! An oat file with code is valid only for the runtime configuration and
//! boot image it was compiled against; the regenerated boot image has other
//! checksums, so ART rejects the code of every original odex whose
//! compiler filter depends on it (all but `verify`), and system_server and
//! a few apps would run their code interpreted. Each of those is compiled
//! again the way the original was, as its header records: its compiler
//! filter, class loader context and boot class path, its profile
//! (`<jar>.prof` for `speed-profile`) and its app image where the original
//! ships one. `verify` oat files stay: their vdex is still used.
//!
//! services.jar is the `system-server` node's edited jar; its profile names
//! the original's dex checksums, so it goes through profman's text form to
//! name the edited one's.
//!
//! The device's own jar (the `device-services` node) follows services.jar
//! on the system server class path, before the APEXes' jars, so it is
//! compiled too (`speed`, as the APEXes' service jars), and every oat file
//! whose class loader context has services.jar is compiled again with it
//! in the context, `verify` ones included: ART rejects an oat file whose
//! context differs from the loader's.
//!
//! The device's own APKs (the `device-services` node's
//! [`device_services::APPS`]) are preopted as a device vendor preopts an
//! app without a profile, `verify` as the image's apps, so that no first
//! boot compiles them.
//!
//! The output is `root/` (the files at their guest paths),
//! `overlay.toml`, the entries image/overlay.toml includes, and
//! `apps/<name>.toml` per app ([`app_manifest`]), which image/overlay.toml
//! includes beside the app, so that the node needs neither as an input.

use super::boot_image::{Dex2oat, boot_image_location, oat_dex_locations, oat_key};
use super::device_services;
use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SERVICES: &str = "/system/framework/services.jar";
const DEVICE_SERVICES_ODEX: &str = "/system/framework/oat/arm64/aim-services.odex";
const PARTITIONS: [&str; 4] = ["system", "system_ext", "product", "vendor"];
const WORKERS: usize = 4;

pub fn out() -> PathBuf {
    aim_paths::out().join("oat")
}

/// The manifest of the compiled files of the device's app at `guest`,
/// relative to the repository root, as an `[[include]]` names it.
pub fn app_manifest(guest: &str) -> String {
    let name = guest.rsplit('/').next().unwrap();
    let name = name.strip_suffix(".apk").unwrap_or(name);
    format!(
        "{}/apps/{name}.toml",
        out().strip_prefix(aim_paths::root()).unwrap().display()
    )
}

pub fn node() -> Node {
    Node {
        name: "oat".into(),
        deps: vec![
            Dep::on("image"),
            Dep::on("art"),
            Dep::on("boot-image"),
            Dep::on("system-server"),
            Dep::on("device-services"),
            Dep::order_only("host/linux-run"),
        ],
        inputs: Vec::new(),
        outputs: vec![out()],
        tools: Vec::new(),
        recipe: 4,
        action: Action::Oat,
        boot: true,
    }
}

/// One original odex to compile again.
struct Job {
    /// Guest path of the odex.
    odex: String,
    /// Guest path of its jar or APK.
    dex: String,
    filter: String,
    context: String,
    bcp: String,
    app_image: bool,
    /// One of the device's apps, whose files go to its own manifest.
    app: bool,
}

impl Job {
    fn stem(&self) -> &str {
        let name = self.odex.rsplit('/').next().unwrap();
        name.strip_suffix(".odex").unwrap()
    }
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let mut odexes = Vec::new();
    for partition in PARTITIONS {
        find_odex(&image, &image.join(partition), &mut odexes)?;
    }
    odexes.sort();
    let boot_image = boot_image_location(&image)?;
    let mut jobs = Vec::new();
    for odex in odexes {
        let host = image.join(&odex[1..]);
        let filter = oat_key(&host, "compiler-filter")?;
        let original = strip_checksums(&oat_key(&host, "classpath")?);
        let context = with_device_services(&original);
        if filter == "verify" && context == original {
            continue;
        }
        jobs.push(Job {
            dex: oat_dex_locations(&host)?.swap_remove(0),
            context,
            bcp: oat_key(&host, "bootclasspath")?,
            app_image: host.with_extension("art").is_file(),
            filter,
            odex,
            app: false,
        });
    }
    // The device's jar, loaded after services.jar as its class loader has
    // it.
    let services = jobs
        .iter()
        .find(|j| j.dex == SERVICES)
        .ok_or("no oat file of services.jar")?;
    let context = match services.context.strip_suffix(']') {
        Some(jars) if jars.ends_with('[') => format!("{jars}{SERVICES}]"),
        Some(jars) => format!("{jars}:{SERVICES}]"),
        None => return Err(format!("services.jar's context {}", services.context)),
    };
    let bcp = services.bcp.clone();
    jobs.push(Job {
        odex: DEVICE_SERVICES_ODEX.into(),
        dex: device_services::JAR.into(),
        filter: "speed".into(),
        context,
        bcp: bcp.clone(),
        app_image: false,
        app: false,
    });
    // The device's APKs.
    for app in &device_services::APPS {
        jobs.push(Job {
            odex: app_odex(app.guest),
            dex: app.guest.into(),
            filter: "verify".into(),
            // Their target SDK adds no implicit library.
            context: "PCL[]".into(),
            bcp: bcp.clone(),
            app_image: false,
            app: true,
        });
    }

    // The guest view: the regenerated boot image and the edited jars over
    // the image's; the originals of those jars, for their profiles, under
    // /data/original.
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let boot = aim_paths::boot_image().join("framework");
    for (dir, guest) in [
        (boot.join("arm64"), "/system/framework/arm64"),
        (boot.clone(), "/system/framework"),
    ] {
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                files.push((format!("{guest}/{name}"), path));
            }
        }
    }
    let edited = [(SERVICES, super::system_server::out().join("services.jar"))];
    files.extend(edited.iter().map(|(g, h)| (g.to_string(), h.clone())));
    files.push((device_services::JAR.into(), device_services::jar()));
    for app in &device_services::APPS {
        files.push((app.guest.into(), device_services::out().join(app.apk)));
    }
    let file_refs: Vec<(&str, PathBuf)> =
        files.iter().map(|(g, h)| (g.as_str(), h.clone())).collect();

    let out = out();
    let work = out.with_extension("work");
    let _ = force_remove(&work);
    let staged = work.join("staged");
    let workers = WORKERS.min(jobs.len());
    let mut dex2oats = Vec::new();
    for index in 0..workers {
        let dir = work.join(format!("worker{index}"));
        for sub in ["data/out", "data/prof", "tmp"] {
            fs::create_dir_all(dir.join(sub)).map_err(|e| e.to_string())?;
        }
        for (guest, _) in &edited {
            let copy = dir.join("data/original").join(&guest[1..]);
            fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(image.join(&guest[1..]), &copy).map_err(|e| e.to_string())?;
        }
        dex2oats.push((Dex2oat::new(ctx, &image, &dir, &file_refs)?, dir));
    }

    log.line(&format!(
        "{} oat files with code, {workers} at once",
        jobs.len()
    ));
    let edited: Vec<&str> = edited.iter().map(|(g, _)| *g).collect();
    let next = Mutex::new(jobs.iter());
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for (dex2oat, dir) in &dex2oats {
            let (next, failed, staged, edited, boot_image) =
                (&next, &failed, &staged, &edited, &boot_image);
            let mut log = log.share().expect("log handle");
            scope.spawn(move || {
                while let Some(job) = { next.lock().unwrap().next() } {
                    let original = edited.contains(&job.dex.as_str());
                    if let Err(e) =
                        compile(job, boot_image, original, dex2oat, dir, staged, &mut log)
                    {
                        failed.lock().unwrap().push(format!("{}: {e}", job.odex));
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    if !failed.is_empty() {
        return Err(failed.join("\n"));
    }

    // Each output replaces the original's file of that name.
    let root = aim_paths::root();
    let relative = |p: &Path| p.strip_prefix(root).unwrap().display().to_string();
    let header = |includer: &str| {
        format!(
            "# Written by the `oat` node of `cargo aim`; {includer} includes it.\n\nschema = 1\n"
        )
    };
    let mut manifest = header("image/overlay.toml");
    fs::create_dir_all(staged.join("apps")).map_err(|e| e.to_string())?;
    for job in &jobs {
        let mut app = header("the manifest that adds the app");
        let entries = if job.app { &mut app } else { &mut manifest };
        let dir = job.odex.rsplit_once('/').unwrap().0;
        for extension in ["odex", "vdex", "art"] {
            let guest = format!("{dir}/{}.{extension}", job.stem());
            let produced = staged.join("root").join(&guest[1..]);
            match (image.join(&guest[1..]).is_file(), produced.is_file()) {
                (true, true) => write!(
                    entries,
                    "\n[[replace]]\npath = \"{guest}\"\nsource = \"{}\"\nreason = \"compiled again ({}, as the original) for the ART exception's runtime and boot image, whose checksums the original's code names\"\n",
                    relative(&out.join("root").join(&guest[1..])),
                    job.filter
                )
                .unwrap(),
                (true, false) => return Err(format!("dex2oat wrote no {guest}")),
                (false, true) if !image.join(&job.dex[1..]).exists() => write!(
                    entries,
                    "\n[[add]]\npath = \"{guest}\"\nsource = \"{}\"\nreason = \"the device's own code compiled ({})\"\n",
                    relative(&out.join("root").join(&guest[1..])),
                    job.filter
                )
                .unwrap(),
                (false, _) => {}
            }
        }
        if job.app {
            let path = root.join(app_manifest(&job.dex));
            let staged_path = staged.join(path.strip_prefix(&out).unwrap());
            fs::write(staged_path, app).map_err(|e| e.to_string())?;
        }
    }
    fs::write(staged.join("overlay.toml"), manifest).map_err(|e| e.to_string())?;

    let _ = force_remove(&out);
    fs::rename(&staged, &out).map_err(|e| e.to_string())?;
    force_remove(&work).map_err(|e| e.to_string())?;
    log.line(&format!("oat files: {}", out.display()));
    Ok(())
}

/// Compiles `job` against `boot_image` (the location the runtime loads)
/// with `dex2oat`, whose view has `dir` as /data, into
/// `staged/root/<guest path>`. `original`: the jar is an edited one whose
/// original is under /data/original.
fn compile(
    job: &Job,
    boot_image: &str,
    original: bool,
    dex2oat: &Dex2oat,
    dir: &Path,
    staged: &Path,
    log: &mut Log,
) -> Result<(), String> {
    let stem = job.stem();
    let mut args = vec![
        format!("--dex-file={}", job.dex),
        format!("--dex-location={}", job.dex),
        format!("--compiler-filter={}", job.filter),
        format!("--class-loader-context={}", job.context),
        "--runtime-arg".into(),
        format!("-Xbootclasspath:{}", job.bcp),
        "--runtime-arg".into(),
        format!("-Xbootclasspath-locations:{}", job.bcp),
        format!("--boot-image={boot_image}"),
        format!("--oat-file=/data/out/{stem}.odex"),
        format!("--oat-location={}", job.odex),
    ];
    if job.filter == "speed-profile" {
        let apk = if original {
            format!("/data/original{}", job.dex)
        } else {
            job.dex.clone()
        };
        let text = dex2oat.profman(
            log,
            &[
                "--dump-classes-and-methods".into(),
                format!("--profile-file={}.prof", job.dex),
                format!("--apk={apk}"),
                format!("--dex-location={}", job.dex),
            ],
        )?;
        fs::write(dir.join(format!("data/prof/{stem}.txt")), text).map_err(|e| e.to_string())?;
        dex2oat.profman(
            log,
            &[
                format!("--create-profile-from=/data/prof/{stem}.txt"),
                format!("--apk={}", job.dex),
                format!("--dex-location={}", job.dex),
                format!("--reference-profile-file=/data/prof/{stem}.prof"),
            ],
        )?;
        args.push(format!("--profile-file=/data/prof/{stem}.prof"));
    }
    if job.app_image {
        args.push(format!("--app-image-file=/data/out/{stem}.art"));
    }
    dex2oat.run(log, &args)?;

    let dest = staged.join("root").join(&job.odex[1..]).with_file_name("");
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(dir.join("data/out")).map_err(|e| e.to_string())? {
        let from = entry.map_err(|e| e.to_string())?.path();
        fs::rename(&from, dest.join(from.file_name().unwrap())).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The odex of the app `apk` (`<dir>/<name>.apk`):
/// `<dir>/oat/arm64/<name>.odex`.
fn app_odex(apk: &str) -> String {
    let (dir, name) = apk.rsplit_once('/').unwrap();
    let stem = name.strip_suffix(".apk").unwrap_or(name);
    format!("{dir}/oat/arm64/{stem}.odex")
}

/// Guest paths of the `oat/arm64/*.odex` files under `dir`, not through
/// links.
fn find_odex(image: &Path, dir: &Path, found: &mut Vec<String>) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            find_odex(image, &path, found)?;
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "odex")
            && path.parent().is_some_and(|p| p.ends_with("oat/arm64"))
        {
            found.push(format!("/{}", path.strip_prefix(image).unwrap().display()));
        }
    }
    Ok(())
}

/// A class loader context without its dex checksums (`jar*123`), which
/// dex2oat records again from the jars it opens.
fn strip_checksums(context: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for c in context.chars() {
        if c == '*' {
            skipping = true;
        } else if skipping && c.is_ascii_digit() {
            continue;
        } else {
            skipping = false;
            out.push(c);
        }
    }
    out
}

/// `context` with the device's jar after services.jar, where the system
/// server class path has it.
fn with_device_services(context: &str) -> String {
    let jar = device_services::JAR;
    context
        .replace(&format!("{SERVICES}:"), &format!("{SERVICES}:{jar}:"))
        .replace(&format!("{SERVICES}]"), &format!("{SERVICES}:{jar}]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_odex_sits_in_its_oat_directory() {
        assert_eq!(
            app_odex("/system/app/A/A.apk"),
            "/system/app/A/oat/arm64/A.odex"
        );
    }

    #[test]
    fn an_app_has_a_manifest_of_its_own() {
        assert_eq!(
            app_manifest("/system_ext/app/AimHome/AimHome.apk"),
            "target/aim/oat/apps/AimHome.toml"
        );
    }

    #[test]
    fn device_services_follow_services() {
        let jar = device_services::JAR;
        assert_eq!(
            with_device_services(&format!("PCL[];PCL[/a.jar:{SERVICES}:/apex/b.jar]")),
            format!("PCL[];PCL[/a.jar:{SERVICES}:{jar}:/apex/b.jar]")
        );
        assert_eq!(
            with_device_services(&format!("PCL[/a.jar:{SERVICES}]")),
            format!("PCL[/a.jar:{SERVICES}:{jar}]")
        );
        assert_eq!(with_device_services("PCL[/a.jar]"), "PCL[/a.jar]");
    }
}
