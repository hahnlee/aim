//! The app shims in `--apps DIR` (`docs/windows.md`, "App shims"), as
//! aim-apps writes them: one per launcher activity, read again when the
//! directory changes. The server launches them (for notifications) and
//! titles its own windows with their labels.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// A shim, from its `Info.plist`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shim {
    pub bundle: PathBuf,
    pub package: String,
    /// The launcher activity's class; empty for the system shim.
    pub activity: String,
    /// The activity's label (`CFBundleName`) and the application's.
    pub label: String,
    pub app_label: String,
    /// The package's primary shim (`AIMPrimary`).
    pub primary: bool,
}

/// The shims' directory (`--apps`).
static DIR: OnceLock<PathBuf> = OnceLock::new();
/// The shims, and when the directory was read.
static INDEX: Mutex<(Option<SystemTime>, Vec<Shim>)> = Mutex::new((None, Vec::new()));

pub fn set_dir(dir: &Path) {
    let _ = DIR.set(dir.to_owned());
}

/// The value of `key` in a plist aim-apps wrote.
fn value(plist: &str, key: &str) -> Option<String> {
    let rest = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(
        rest[start..start + end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

fn read(bundle: PathBuf) -> Option<Shim> {
    let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
    let package = value(&plist, "AIMPackage")?;
    let label = value(&plist, "CFBundleName").unwrap_or_else(|| package.clone());
    Some(Shim {
        activity: value(&plist, "AIMActivity").unwrap_or_default(),
        app_label: value(&plist, "AIMAppLabel").unwrap_or_else(|| label.clone()),
        primary: plist.contains("<key>AIMPrimary</key>"),
        package,
        label,
        bundle,
    })
}

/// Run `f` on the shims, read again if the directory changed.
fn with<R>(f: impl FnOnce(&[Shim]) -> R) -> R {
    let mut index = INDEX.lock().unwrap();
    if let Some(dir) = DIR.get() {
        let modified = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
        if modified != index.0 {
            let shims = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "app"))
                .filter_map(read)
                .collect();
            *index = (modified, shims);
        }
    }
    f(&index.1)
}

/// The shim that stands for `package`: its primary one, else any.
fn of<'a>(shims: &'a [Shim], package: &str) -> Option<&'a Shim> {
    let mut all = shims.iter().filter(|s| s.package == package);
    let first = all.clone().next();
    all.find(|s| s.primary).or(first)
}

/// The bundle of the shim that stands for `package`.
pub fn bundle(package: &str) -> Option<PathBuf> {
    with(|shims| of(shims, package).map(|s| s.bundle.clone()))
}

/// Whether the shim of `package`'s `activity` (a class) stands for the
/// package: its primary one, else its first; any, for a package the
/// shims do not know.
pub fn stands_for(package: &str, activity: &str) -> bool {
    with(|shims| of(shims, package).is_none_or(|s| s.activity == activity))
}

/// The title of a task of `package` started with `activity`
/// (`package/class`) that has no label of its own, as a launcher names
/// it: that launcher activity's label, else the application's.
pub fn title(package: &str, activity: &str) -> Option<String> {
    let class = activity.split_once('/').map_or("", |(_, c)| c);
    with(|shims| {
        match shims
            .iter()
            .find(|s| s.package == package && s.activity == class)
        {
            Some(s) => Some(s.label.clone()),
            None => of(shims, package).map(|s| s.app_label.clone()),
        }
    })
}
