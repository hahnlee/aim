//! `device-services`: the device's own system services, added as a device
//! vendor adds them (docs/system-services.md, "The system_server bridge"):
//!
//! - `aim-services.jar`, the classes of `java/device-services/src` and the
//!   Java of its AIDL, compiled against the stubs of the image's classes in
//!   `java/device-services/stubs` and checked against the image
//!   ([`java::check_linkage`]);
//! - `systemserverclasspath.pb`, the platform's classpath fragment with the
//!   jar appended, which the `oat` node compiles like the others;
//! - `framework-overlay.apk`, the static overlay of framework-res of
//!   `java/framework-overlay` that names the service in
//!   `config_deviceSpecificSystemServices`, signed with AOSP's test key;
//! - `lightweight-shell-overlay.apk`, the overlay of
//!   `java/lightweight-shell-overlay` for the image without the Android
//!   shell (docs/m1-shell.md), signed likewise;
//! - `notification-permission.apk`, the activity of
//!   `java/notification-permission` that system_server starts in place of
//!   PermissionController's dialog for POST_NOTIFICATIONS (#470), and
//!   `media-projection.apk`, the MediaProjection consent activity of
//!   `java/media-projection` (docs/media.md), and `image-wallpaper.apk`,
//!   the lightweight shell's static wallpaper of `java/image-wallpaper`
//!   (#603), and `lightweight-home.apk`, its HOME activity of
//!   `java/lightweight-home`, each compiled with the Java of the AIDL and
//!   checked against the boot class path.

use super::java::{self, Toolchain};
use super::{files, repo, service_aidl};
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::assemble::force_remove;
use aim_android_image::classpath::{self, BOOTCLASSPATH, Jar, SYSTEMSERVERCLASSPATH};
use std::fs;
use std::path::PathBuf;

const SOURCES: &str = "java/device-services";
const OVERLAY: &str = "java/framework-overlay";
const SHELL_OVERLAY: &str = "java/lightweight-shell-overlay";
/// The jar's guest path.
pub const JAR: &str = "/system/framework/aim-services.jar";

/// An app with code the node builds.
pub struct App {
    /// Its sources, relative to the repository root.
    pub sources: &'static str,
    /// The APK's name in the node's output.
    pub apk: &'static str,
    /// Where image/overlay.toml or a variant adds it, and where the `oat`
    /// node compiles it for.
    pub guest: &'static str,
}

pub const APPS: [App; 4] = [
    App {
        sources: "java/notification-permission",
        apk: "notification-permission.apk",
        guest: "/system/app/AimNotificationPermission/AimNotificationPermission.apk",
    },
    App {
        sources: "java/media-projection",
        apk: "media-projection.apk",
        guest: "/system/app/AimMediaProjection/AimMediaProjection.apk",
    },
    App {
        sources: "java/image-wallpaper",
        apk: "image-wallpaper.apk",
        guest: "/system_ext/priv-app/AimImageWallpaper/AimImageWallpaper.apk",
    },
    App {
        sources: "java/lightweight-home",
        apk: "lightweight-home.apk",
        guest: "/system_ext/app/AimHome/AimHome.apk",
    },
];
const FRAGMENT: &str = "system/etc/classpaths/systemserverclasspath.pb";

pub fn out() -> PathBuf {
    aim_paths::out().join("device-services")
}

pub fn jar() -> PathBuf {
    out().join("aim-services.jar")
}

pub fn node() -> Node {
    let mut inputs = vec![repo(java::LOCK)];
    inputs.extend(files(SOURCES));
    inputs.extend(files(OVERLAY));
    inputs.extend(files(SHELL_OVERLAY));
    for app in &APPS {
        inputs.extend(files(app.sources));
    }
    let mut outputs = vec![
        jar(),
        out().join("systemserverclasspath.pb"),
        out().join("framework-overlay.apk"),
        out().join("lightweight-shell-overlay.apk"),
    ];
    outputs.extend(APPS.iter().map(|app| out().join(app.apk)));
    Node {
        name: "device-services".into(),
        deps: vec![Dep::on("image")],
        inputs,
        outputs,
        tools: Vec::new(),
        recipe: 2,
        action: Action::DeviceServices,
        boot: true,
    }
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let tools = Toolchain::fetch(log)?;
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let out = out();
    let work = out.with_extension("work");
    let _ = force_remove(&work);
    let staged = work.join("out");
    fs::create_dir_all(&staged).map_err(|e| e.to_string())?;

    let sources = repo(SOURCES);
    let stubs = work.join("stubs");
    tools.javac(log, &[sources.join("stubs")], &[], &stubs, false)?;
    let generated = work.join("aidl");
    tools.aidl(log, &sources.join("aidl"), &generated)?;
    let classes = work.join("classes");
    tools.javac(
        log,
        &[sources.join("src"), generated.clone()],
        std::slice::from_ref(&stubs),
        &classes,
        true,
    )?;
    let stubs_dex = tools.d8(log, &stubs, None, &work.join("stubs-dex"))?;
    let dex = tools.d8(log, &classes, Some(&stubs), &work.join("dex"))?;
    let boot = classpath::jars(&image, "bootclasspath.pb", BOOTCLASSPATH)?;
    let mut jars = boot.clone();
    jars.extend(classpath::jars(
        &image,
        "systemserverclasspath.pb",
        SYSTEMSERVERCLASSPATH,
    )?);
    java::check_linkage(&image, &jars, Some(&stubs_dex), &dex)?;
    service_aidl::check_own_stubs(&dex)?;
    tools.jar(log, &dex, &staged.join("aim-services.jar"))?;

    let mut fragment = fs::read(image.join(FRAGMENT)).map_err(|e| format!("{FRAGMENT}: {e}"))?;
    if classpath::parse(&fragment)?.iter().any(|j| j.path == JAR) {
        return Err(format!("{FRAGMENT} already has {JAR}"));
    }
    fragment.extend(classpath::encode(&Jar {
        path: JAR.into(),
        classpath: SYSTEMSERVERCLASSPATH,
    }));
    fs::write(staged.join("systemserverclasspath.pb"), fragment).map_err(|e| e.to_string())?;

    for (dir, apk) in [
        (OVERLAY, "framework-overlay.apk"),
        (SHELL_OVERLAY, "lightweight-shell-overlay.apk"),
    ] {
        let overlay = repo(dir);
        tools.apk(
            log,
            &overlay.join("AndroidManifest.xml"),
            Some(&overlay.join("res")),
            None,
            &image.join("system/framework/framework-res.apk"),
            &staged.join(apk),
        )?;
    }

    // An app's code: it links against the boot class path only.
    for App { sources, apk, .. } in &APPS {
        let app = repo(sources);
        let apk = apk.strip_suffix(".apk").unwrap();
        let app_classes = work.join(format!("{apk}-classes"));
        tools.javac(
            log,
            &[app.join("src"), generated.clone()],
            std::slice::from_ref(&stubs),
            &app_classes,
            true,
        )?;
        let app_dex = tools.d8(
            log,
            &app_classes,
            Some(&stubs),
            &work.join(format!("{apk}-dex")),
        )?;
        java::check_linkage(&image, &boot, None, &app_dex)?;
        tools.apk(
            log,
            &app.join("AndroidManifest.xml"),
            None,
            Some(&app_dex),
            &image.join("system/framework/framework-res.apk"),
            &staged.join(format!("{apk}.apk")),
        )?;
    }

    let _ = force_remove(&out);
    fs::rename(&staged, &out).map_err(|e| e.to_string())?;
    force_remove(&work).map_err(|e| e.to_string())
}
