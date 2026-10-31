//! The launcher apps installed in a guest: the system image's apps and the
//! ones installed into `/data/app` (which also hold system apps' updates
//! and the decompressed compressed ones), one per package.

use std::fs;
use std::path::{Path, PathBuf};

use crate::apk::Apk;
use crate::app::{self, App};
use crate::restrictions;

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
    scan_all(root, data, framework).enabled(data)
}

/// Parsed launcher candidates, including entries disabled by the manifest.
pub struct Scan(Vec<(PathBuf, Vec<App>)>);

pub fn scan_all(root: &Path, data: Option<&Path>, framework: &Apk) -> Scan {
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
        let Ok(apps) = app::candidates(&apk, Some(framework)) else {
            continue;
        };
        let Some(version) = apps.first().map(|a| a.version) else {
            continue;
        };
        match out
            .iter_mut()
            .find(|(_, _, o)| o[0].package == apps[0].package)
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
    Scan(out.into_iter().map(|(_, apk, apps)| (apk, apps)).collect())
}

impl Scan {
    /// Re-evaluate user 0 without opening or parsing APKs again.
    pub fn enabled(&self, data: Option<&Path>) -> Vec<Installed> {
        let state = data
            .map(|d| restrictions::read(&d.join("system/users/0/package-restrictions.xml")))
            .unwrap_or_default();
        let mut apps: Vec<Installed> = self
            .0
            .iter()
            .flat_map(|(apk, apps)| {
                app::select(apps, state.get(&apps[0].package))
                    .into_iter()
                    .map(|app| Installed {
                        apk: apk.clone(),
                        app,
                    })
            })
            .collect();
        apps.sort_by(|a, b| a.app.label.cmp(&b.app.label));
        apps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrictions_reselect_cached_launcher_fixtures() {
        let dir =
            std::env::temp_dir().join(format!("aim-apps-restrictions-{}", std::process::id()));
        let apk_dir = dir.join("system/app/Fixture");
        let user = dir.join("system/users/0");
        fs::create_dir_all(&apk_dir).unwrap();
        fs::create_dir_all(&user).unwrap();
        let apk = apk_dir.join("base.apk");
        fs::write(&apk, include_bytes!("fixtures/launchers.apk")).unwrap();
        let framework = Apk::open(&apk).unwrap();
        let scan = scan_all(&dir, Some(&dir), &framework);
        let path = user.join("package-restrictions.xml");
        let write = |body: &str| {
            fs::write(path.with_extension("new"), format!(
                "<package-restrictions><pkg name='org.fixture'>{body}</pkg></package-restrictions>"
            )).unwrap();
            fs::rename(path.with_extension("new"), &path).unwrap();
        };
        let labels = || {
            scan.enabled(Some(&dir))
                .into_iter()
                .map(|i| i.app.label)
                .collect::<Vec<_>>()
        };
        let shims = dir.join("shims");
        let sync = || {
            crate::shim::sync(
                &shims,
                &scan.enabled(Some(&dir)),
                &framework,
                &dir.join("display"),
                Path::new("/usr/bin/true"),
                Some("restrictions-fixture"),
            )
            .unwrap()
        };
        assert_eq!(labels(), ["On"]);
        assert_eq!(sync().written, ["On.app"]);
        write("<disabled-components><item name='org.fixture.On'/></disabled-components>");
        assert!(labels().is_empty());
        assert_eq!(sync().removed, ["On.app"]);
        write(
            "<enabled-components><item name='org.fixture.On'/><item name='org.fixture.Off'/><item name='org.fixture.Alias'/></enabled-components>",
        );
        assert_eq!(labels(), ["Alias", "Off", "On"]);
        assert_eq!(sync().written.len(), 3);
        // The old committed state wins throughout a ResilientAtomicFile write.
        let backup = user.join("package-restrictions-backup.xml");
        fs::rename(&path, &backup).unwrap();
        fs::write(&path, b"<package-restrictions").unwrap();
        assert_eq!(labels(), ["Alias", "Off", "On"]);
        write("");
        fs::remove_file(&backup).unwrap();
        assert_eq!(labels(), ["On"]);
        assert_eq!(sync().removed.len(), 2);
        crate::shim::clean(&shims).unwrap();
        // Selection remains possible after the source APK disappears.
        fs::remove_file(&apk).unwrap();
        write("<enabled-components><item name='org.fixture.Off'/></enabled-components>");
        let apps = scan.enabled(Some(&dir));
        assert_eq!(labels(), ["Off", "On"]);
        assert_eq!(apps.iter().filter(|i| i.app.primary).count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
