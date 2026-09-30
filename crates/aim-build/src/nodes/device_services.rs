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
//!   `config_deviceSpecificSystemServices`, signed with AOSP's test key.

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
/// The jar's guest path.
pub const JAR: &str = "/system/framework/aim-services.jar";
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
    Node {
        name: "device-services".into(),
        deps: vec![Dep::on("image")],
        inputs,
        outputs: vec![
            jar(),
            out().join("systemserverclasspath.pb"),
            out().join("framework-overlay.apk"),
        ],
        tools: Vec::new(),
        recipe: 1,
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
        &[sources.join("src"), generated],
        std::slice::from_ref(&stubs),
        &classes,
        true,
    )?;
    let stubs_dex = tools.d8(log, &stubs, None, &work.join("stubs-dex"))?;
    let dex = tools.d8(log, &classes, Some(&stubs), &work.join("dex"))?;
    let mut jars = classpath::jars(&image, "bootclasspath.pb", BOOTCLASSPATH)?;
    jars.extend(classpath::jars(
        &image,
        "systemserverclasspath.pb",
        SYSTEMSERVERCLASSPATH,
    )?);
    java::check_linkage(&image, &jars, &stubs_dex, &dex)?;
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

    let overlay = repo(OVERLAY);
    tools.apk(
        log,
        &overlay.join("AndroidManifest.xml"),
        &overlay.join("res"),
        &image.join("system/framework/framework-res.apk"),
        &staged.join("framework-overlay.apk"),
    )?;

    let _ = force_remove(&out);
    fs::rename(&staged, &out).map_err(|e| e.to_string())?;
    force_remove(&work).map_err(|e| e.to_string())
}
