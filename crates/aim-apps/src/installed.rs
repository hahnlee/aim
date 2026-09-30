//! The launcher apps installed in a guest: the system image's apps and the
//! ones installed into `/data/app` (which also hold system apps' updates
//! and the decompressed compressed ones), one per package.

use std::fs;
use std::path::{Path, PathBuf};

use crate::apk::Apk;
use crate::app::{self, App};
use crate::restrictions::{self, Restrictions};

/// The system image's app directories.
const SYSTEM_APPS: [&str; 8] = [
    "system/app",
    "system/priv-app",
    "system_ext/app",
    "system_ext/priv-app",
    "product/app",
    "product/priv-app",
    "vendor/app",
    "vendor/priv-app",
];

/// A launcher entry and the APK it is read from.
pub struct Installed {
    pub apk: PathBuf,
    pub app: App,
}

/// `dir/*/*.apk`, and one level deeper (`/data/app/~~x/pkg-y/base.apk`).
fn apks(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                apks(&p, depth - 1, out);
            }
        } else if p.extension().is_some_and(|x| x == "apk") && depth < 2 {
            out.push(p);
        }
    }
}

/// The launcher entries (one per launcher activity) of the guest whose
/// system image root is `root` and whose `/data` is `data`, each package's
/// from one APK: an installed APK over the image's, and the newest
/// version. With `data`, packages not installed or not
/// enabled for user 0, and launcher activities disabled at run time, are
/// left out.
pub fn scan(root: &Path, data: Option<&Path>, framework: &Apk) -> Vec<Installed> {
    let state: Restrictions = data
        .map(|d| restrictions::read(&d.join("system/users/0/package-restrictions.xml")))
        .unwrap_or_default();
    let mut paths = Vec::new();
    for d in SYSTEM_APPS {
        apks(&root.join(d), 1, &mut paths);
    }
    let system = paths.len();
    if let Some(data) = data {
        apks(&data.join("app"), 2, &mut paths);
    }
    // By package: whether it is from /data, its APK and entries.
    let mut out: Vec<(bool, PathBuf, Vec<App>)> = Vec::new();
    for (i, path) in paths.into_iter().enumerate() {
        let from_data = i >= system;
        let Ok(apk) = Apk::open(&path) else { continue };
        let Ok(manifest_package) = apk.manifest().map(|m| match m.named("package") {
            Some(crate::res::Value::String(p)) => p.clone(),
            _ => String::new(),
        }) else {
            continue;
        };
        let pkg = state.get(&manifest_package);
        if pkg.is_some_and(|p| !p.installed || !p.enabled) {
            continue;
        }
        let Ok(apps) = app::read(&apk, Some(framework), pkg) else {
            continue;
        };
        let Some(version) = apps.first().map(|a| a.version) else {
            continue;
        };
        match out
            .iter_mut()
            .find(|(_, _, o)| o[0].package == manifest_package)
        {
            Some((d, p, o)) if (from_data, version) > (*d, o[0].version) => {
                *d = from_data;
                *p = path;
                *o = apps;
            }
            Some(_) => {}
            None => out.push((from_data, path, apps)),
        }
    }
    let mut apps: Vec<Installed> = out
        .into_iter()
        .flat_map(|(_, apk, apps)| {
            apps.into_iter().map(move |app| Installed {
                apk: apk.clone(),
                app,
            })
        })
        .collect();
    apps.sort_by(|a, b| a.app.label.cmp(&b.app.label));
    apps
}
