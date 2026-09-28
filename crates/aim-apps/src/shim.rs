//! App shims: one small macOS app bundle per Android launcher app, with
//! the app's name and icon, that opens the app in window mode.
//!
//! The bundle's executable is the display server binary (`aim-display`),
//! which, run from a bundle whose `Info.plist` names a package
//! (`AIMPackage`), hosts that app's windows under the bundle's Dock icon,
//! talking to the display server at `AIMDisplaySocket`
//! (`docs/windows.md`, "App shims").

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::app::App;

/// The executable's name inside a shim.
const EXECUTABLE: &str = "aim-app";
/// Bumped when a shim's layout or icon drawing changes, so shims are
/// written again.
pub const LAYOUT: u32 = 2;

/// A shim to write: the app, its icon, and where its windows come from.
pub struct Shim<'a> {
    pub app: &'a App,
    pub icns: &'a [u8],
    /// The display server's socket.
    pub socket: &'a Path,
    /// The window host binary (`aim-display`).
    pub host: &'a Path,
}

/// What an existing shim says about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct Stamp {
    pub package: String,
    /// "LAYOUT/version/socket/host size-modification time".
    pub version: String,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A bundle identifier for `package`: letters, digits, hyphens and dots.
pub fn bundle_id(package: &str) -> String {
    let safe: String = package
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("dev.aim.app.{safe}")
}

fn version(s: &Shim) -> io::Result<String> {
    let host = fs::metadata(s.host)?;
    let modified = host
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(format!(
        "{LAYOUT}/{}/{}/{}-{modified}",
        s.app.version,
        s.socket.display(),
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
	<key>AIMDisplaySocket</key>
	<string>{socket}</string>
	<key>AIMShimVersion</key>
	<string>{stamp}</string>
</dict>
</plist>
"#,
        id = bundle_id(&s.app.package),
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
        version: plist_value(&plist, "AIMShimVersion").unwrap_or_default(),
    })
}

/// Whether the shim at `bundle` is `s` as it would be written now.
pub fn current(bundle: &Path, s: &Shim) -> bool {
    stamp(bundle)
        .is_some_and(|st| st.package == s.app.package && version(s).is_ok_and(|v| st.version == v))
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
    let _ = fs::remove_dir_all(bundle);
    fs::rename(&tmp, bundle)
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

/// The bundle path for `app` in `dir`: its label, or its label and
/// package when another app has the label.
pub fn path(dir: &Path, app: &App, taken: &[String]) -> PathBuf {
    let clean: String = app
        .label
        .chars()
        .map(|c| if c == '/' || c == ':' { '-' } else { c })
        .collect();
    let name = if taken.contains(&clean) {
        format!("{clean} ({})", app.package)
    } else {
        clean
    };
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
        };
        let p = info_plist(&s, "1/7/x");
        assert_eq!(
            plist_value(&p, "AIMPackage").as_deref(),
            Some("org.example_app")
        );
        assert_eq!(plist_value(&p, "CFBundleName").as_deref(), Some("A & <B>"));
        assert_eq!(plist_value(&p, "AIMShimVersion").as_deref(), Some("1/7/x"));
        assert_eq!(bundle_id("org.example_app"), "dev.aim.app.org.example-app");
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

/// What a sync changed.
#[derive(Debug, Default)]
pub struct Synced {
    pub written: Vec<String>,
    pub removed: Vec<String>,
}

/// Make `dir` hold one shim per app of `apps`: new and changed apps are
/// written, and our shims of apps no longer installed are removed. Other
/// files in `dir` are left alone.
pub fn sync(
    dir: &Path,
    apps: &[crate::installed::Installed],
    framework: &crate::apk::Apk,
    socket: &Path,
    host: &Path,
) -> io::Result<Synced> {
    fs::create_dir_all(dir)?;
    let mut ours: Vec<(PathBuf, Stamp)> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .filter_map(|p| stamp(&p).map(|s| (p, s)))
        .collect();
    let mut done = Synced::default();
    let mut taken: Vec<String> = Vec::new();
    for i in apps {
        let bundle = match ours.iter().position(|(_, s)| s.package == i.app.package) {
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
        done.written.push(i.app.package.clone());
    }
    for (bundle, s) in ours {
        fs::remove_dir_all(&bundle)?;
        done.removed.push(s.package);
    }
    Ok(done)
}
