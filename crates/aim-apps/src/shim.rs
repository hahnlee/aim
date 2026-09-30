//! App shims: one small macOS app bundle per Android launcher activity, as
//! a launcher lists them, with the activity's name and icon, that opens it
//! in window mode.
//!
//! The bundle's executable is the display server binary (`aim-display`),
//! which, run from a bundle whose `Info.plist` names a package
//! (`AIMPackage`) and activity (`AIMActivity`), hosts that activity's task
//! windows under the bundle's Dock icon, talking to the display server at
//! `AIMDisplaySocket` (`docs/windows.md`, "App shims"). A package's primary
//! shim (`AIMPrimary`: the entry named as the application, else its
//! first) has the package's bundle identifier; it also shows the package's
//! other windows and its notifications.
//!
//! Launch Services and Notification Center tell apps apart by bundle
//! identifier, so the shims of a guest other than the user's own have
//! identifiers scoped to its data directory (`scope`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::app::App;

/// The executable's name inside a shim.
const EXECUTABLE: &str = "aim-app";
/// Bumped when a shim's layout or icon drawing changes, so shims are
/// written again.
pub const LAYOUT: u32 = 4;

/// A shim to write: the app, its icon, and where its windows come from.
pub struct Shim<'a> {
    pub app: &'a App,
    pub icns: &'a [u8],
    /// The display server's socket.
    pub socket: &'a Path,
    /// The window host binary (`aim-display`).
    pub host: &'a Path,
    /// The guest's scope in bundle identifiers, if not the user's own.
    pub scope: Option<&'a str>,
}

/// What an existing shim says about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct Stamp {
    pub package: String,
    pub activity: String,
    /// "LAYOUT/version/socket/scope/host size-modification time".
    pub version: String,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The scope of a guest whose `/data` is `data`: a hash of its absolute
/// path, the same for every run.
pub fn scope(data: &Path) -> io::Result<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::path::absolute(data)?;
    // FNV-1a, 32 bits.
    let hash = path
        .as_os_str()
        .as_bytes()
        .iter()
        .fold(0x811c_9dc5u32, |h, &b| {
            (h ^ b as u32).wrapping_mul(0x0100_0193)
        });
    Ok(format!("{hash:08x}"))
}

/// A bundle identifier for `app`: its package's for the primary entry,
/// else the package's and the activity's (without the package prefix);
/// letters, digits, hyphens and dots. A scoped guest's are under
/// `dev.aim.app-SCOPE` instead of `dev.aim.app`.
pub fn bundle_id(app: &App, scope: Option<&str>) -> String {
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    let base = match scope {
        Some(scope) => format!("dev.aim.app-{scope}"),
        None => "dev.aim.app".into(),
    };
    let package = safe(&app.package);
    if app.primary {
        return format!("{base}.{package}");
    }
    let class = app
        .activity
        .strip_prefix(&format!("{}.", app.package))
        .unwrap_or(&app.activity);
    format!("{base}.{package}.{}", safe(class))
}

fn version(s: &Shim) -> io::Result<String> {
    let host = fs::metadata(s.host)?;
    let modified = host
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(format!(
        "{LAYOUT}/{}/{}/{}/{}-{modified}",
        s.app.version,
        s.socket.display(),
        s.scope.unwrap_or(""),
        host.len()
    ))
}

fn info_plist(s: &Shim, version: &str) -> String {
    let label = escape(&s.app.label);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundleIdentifier</key>
	<string>{id}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{label}</string>
	<key>CFBundleDisplayName</key>
	<string>{label}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{code}</string>
	<key>CFBundleVersion</key>
	<string>{code}</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>AIMPackage</key>
	<string>{package}</string>
	<key>AIMActivity</key>
	<string>{activity}</string>
	<key>AIMAppLabel</key>
	<string>{app_label}</string>
{primary}	<key>AIMDisplaySocket</key>
	<string>{socket}</string>
	<key>AIMShimVersion</key>
	<string>{stamp}</string>
</dict>
</plist>
"#,
        id = bundle_id(s.app, s.scope),
        app_label = escape(&s.app.app_label),
        primary = if s.app.primary {
            "\t<key>AIMPrimary</key>\n\t<true/>\n"
        } else {
            ""
        },
        code = s.app.version,
        package = escape(&s.app.package),
        activity = escape(&s.app.activity),
        socket = escape(&s.socket.display().to_string()),
        stamp = escape(version),
    )
}

/// The value of `key` in a plist we wrote.
fn plist_value(plist: &str, key: &str) -> Option<String> {
    let at = plist.find(&format!("<key>{key}</key>"))?;
    let rest = &plist[at..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(
        rest[start..start + end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// The stamp of the shim at `bundle`, if it is one of ours.
pub fn stamp(bundle: &Path) -> Option<Stamp> {
    let plist = fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
    Some(Stamp {
        package: plist_value(&plist, "AIMPackage")?,
        activity: plist_value(&plist, "AIMActivity").unwrap_or_default(),
        version: plist_value(&plist, "AIMShimVersion").unwrap_or_default(),
    })
}

/// Whether the shim at `bundle` is `s` as it would be written now.
pub fn current(bundle: &Path, s: &Shim) -> bool {
    stamp(bundle).is_some_and(|st| {
        st.package == s.app.package
            && st.activity == s.app.activity
            && version(s).is_ok_and(|v| st.version == v)
    })
}

/// Write `s` as the bundle `bundle` (replacing what is there).
pub fn write(bundle: &Path, s: &Shim) -> io::Result<()> {
    let version = version(s)?;
    let tmp = bundle.with_extension("app.new");
    let _ = fs::remove_dir_all(&tmp);
    let contents = tmp.join("Contents");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;
    fs::write(contents.join("Info.plist"), info_plist(s, &version))?;
    fs::write(contents.join("PkgInfo"), b"APPL????")?;
    fs::write(contents.join("Resources/AppIcon.icns"), s.icns)?;
    let exe = contents.join("MacOS").join(EXECUTABLE);
    clone(s.host, &exe)?;
    sign(&tmp)?;
    let _ = fs::remove_dir_all(bundle);
    fs::rename(&tmp, bundle)
}

/// Sign the bundle ad hoc, sealing its `Info.plist` with the executable:
/// macOS lets only a signed bundle post notifications as itself.
fn sign(bundle: &Path) -> io::Result<()> {
    let status = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(bundle)
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "codesign {}: {status}",
            bundle.display()
        )));
    }
    Ok(())
}

/// Copy `from` to `to`, as an APFS clone where the volume allows.
fn clone(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let (f, t) = (
        CString::new(from.as_os_str().as_bytes())?,
        CString::new(to.as_os_str().as_bytes())?,
    );
    // SAFETY: two NUL-terminated paths.
    if unsafe { libc::clonefile(f.as_ptr(), t.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    fs::copy(from, to).map(drop)
}

/// The bundle path for `app` in `dir`: its label, else (another shim has
/// the label) its label and package, else its label and activity.
pub fn path(dir: &Path, app: &App, taken: &[String]) -> PathBuf {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c == '/' || c == ':' { '-' } else { c })
            .collect()
    };
    let label = clean(&app.label);
    let names = [
        label.clone(),
        format!("{label} ({})", clean(&app.package)),
        format!("{label} ({})", clean(&app.activity)),
    ];
    let name = names
        .iter()
        .find(|n| !taken.contains(n))
        .unwrap_or(&names[2]);
    dir.join(format!("{name}.app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_round_trip() {
        let app = App {
            package: "org.example_app".into(),
            label: "A & <B>".into(),
            app_label: "Example".into(),
            primary: true,
            version: 7,
            activity: "org.example_app.Main".into(),
            icon: None,
            theme: 0,
        };
        let s = Shim {
            app: &app,
            icns: b"",
            socket: Path::new("/tmp/display"),
            host: Path::new("/bin/sh"),
            scope: None,
        };
        let p = info_plist(&s, "1/7/x");
        assert_eq!(
            plist_value(&p, "AIMPackage").as_deref(),
            Some("org.example_app")
        );
        assert_eq!(plist_value(&p, "CFBundleName").as_deref(), Some("A & <B>"));
        assert_eq!(plist_value(&p, "AIMShimVersion").as_deref(), Some("1/7/x"));
        assert_eq!(plist_value(&p, "AIMAppLabel").as_deref(), Some("Example"));
        assert!(p.contains("<key>AIMPrimary</key>"));
        assert_eq!(bundle_id(&app, None), "dev.aim.app.org.example-app");
        assert_eq!(
            bundle_id(&app, Some("0a1b2c3d")),
            "dev.aim.app-0a1b2c3d.org.example-app"
        );
        let other = App {
            primary: false,
            activity: "org.example_app.voice.Search$A".into(),
            ..app.clone()
        };
        assert_eq!(
            bundle_id(&other, None),
            "dev.aim.app.org.example-app.voice.Search-A"
        );
        assert_eq!(
            bundle_id(&other, Some("0a1b2c3d")),
            "dev.aim.app-0a1b2c3d.org.example-app.voice.Search-A"
        );
        assert!(!info_plist(&Shim { app: &other, ..s }, "").contains("AIMPrimary"));
    }

    #[test]
    fn scopes_follow_the_data_directory() {
        let a = scope(Path::new("/a/data")).unwrap();
        assert_eq!(a.len(), 8);
        assert_eq!(a, scope(Path::new("/a/data")).unwrap());
        assert_ne!(a, scope(Path::new("/b/data")).unwrap());
    }

    #[test]
    fn paths_stay_apart() {
        let app = |package: &str, activity: &str| App {
            package: package.into(),
            label: "Clock".into(),
            app_label: "Clock".into(),
            primary: true,
            version: 1,
            activity: activity.into(),
            icon: None,
            theme: 0,
        };
        let dir = Path::new("/apps");
        let name = |a: &App, taken: &[&str]| {
            let taken: Vec<String> = taken.iter().map(|s| s.to_string()).collect();
            path(dir, a, &taken)
        };
        let a = app("a.clock", "a.clock.Main");
        assert_eq!(name(&a, &[]), dir.join("Clock.app"));
        assert_eq!(name(&a, &["Clock"]), dir.join("Clock (a.clock).app"));
        assert_eq!(
            name(&a, &["Clock", "Clock (a.clock)"]),
            dir.join("Clock (a.clock.Main).app")
        );
    }
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSRegisterURL(url: *const std::ffi::c_void, update: bool) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: *const std::ffi::c_void,
        path: *const u8,
        len: isize,
        dir: bool,
    ) -> *const std::ffi::c_void;
    fn CFRelease(r: *const std::ffi::c_void);
}

/// Tell Launch Services about a written bundle, so the Dock, Launchpad and
/// Spotlight show its name and icon as they are now.
fn register(bundle: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let p = bundle.as_os_str().as_bytes();
    // SAFETY: a URL created and released here.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            p.as_ptr(),
            p.len() as isize,
            true,
        );
        if !url.is_null() {
            LSRegisterURL(url, true);
            CFRelease(url);
        }
    }
}

/// The Launch Services registration tool.
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// Remove the shim at `bundle`, and its Launch Services registration, so
/// the Dock, Launchpad and Spotlight forget it.
fn remove(bundle: &Path) -> io::Result<()> {
    let unregistered = std::process::Command::new(LSREGISTER)
        .arg("-u")
        .arg(bundle)
        .status();
    if !unregistered.is_ok_and(|s| s.success()) {
        eprintln!("aim-apps: lsregister -u {}: failed", bundle.display());
    }
    fs::remove_dir_all(bundle)
}

/// Our shims in `dir`.
fn ours(dir: &Path) -> io::Result<Vec<(PathBuf, Stamp)>> {
    Ok(fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .filter_map(|p| stamp(&p).map(|s| (p, s)))
        .collect())
}

/// A bundle's file name.
fn name(bundle: &Path) -> String {
    bundle
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Remove our shims from `dir` (other files stay): their names.
pub fn clean(dir: &Path) -> io::Result<Vec<String>> {
    let mut removed = Vec::new();
    for (bundle, _) in ours(dir)? {
        remove(&bundle)?;
        removed.push(name(&bundle));
    }
    Ok(removed)
}

/// What a sync changed: the bundles' names.
#[derive(Debug, Default)]
pub struct Synced {
    pub written: Vec<String>,
    pub removed: Vec<String>,
}

/// Make `dir` hold one shim per entry of `apps`: new and changed entries
/// are written, and our shims of entries no longer there are removed.
/// Other files in `dir` are left alone.
pub fn sync(
    dir: &Path,
    apps: &[crate::installed::Installed],
    framework: &crate::apk::Apk,
    socket: &Path,
    host: &Path,
    scope: Option<&str>,
) -> io::Result<Synced> {
    fs::create_dir_all(dir)?;
    let mut ours = ours(dir)?;
    let mut done = Synced::default();
    // Names of the shims kept or written, and of those still to be matched.
    let mut taken: Vec<String> = ours
        .iter()
        .filter_map(|(p, _)| Some(p.file_stem()?.to_string_lossy().into_owned()))
        .collect();
    for i in apps {
        let found = ours
            .iter()
            .position(|(_, s)| s.package == i.app.package && s.activity == i.app.activity);
        let bundle = match found {
            Some(k) => ours.swap_remove(k).0,
            None => path(dir, &i.app, &taken),
        };
        if let Some(name) = bundle.file_stem() {
            taken.push(name.to_string_lossy().into_owned());
        }
        let probe = Shim {
            app: &i.app,
            icns: &[],
            socket,
            host,
            scope,
        };
        if current(&bundle, &probe) {
            continue;
        }
        let Ok(apk) = crate::apk::Apk::open(&i.apk) else {
            continue;
        };
        let Some(icns) = i.app.icns(&apk, Some(framework)) else {
            continue;
        };
        write(
            &bundle,
            &Shim {
                icns: &icns,
                ..probe
            },
        )?;
        register(&bundle);
        done.written.push(name(&bundle));
    }
    for (bundle, _) in ours {
        remove(&bundle)?;
        done.removed.push(name(&bundle));
    }
    Ok(done)
}
