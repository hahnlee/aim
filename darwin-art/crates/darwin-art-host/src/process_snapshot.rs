//! Construct process bootstrap inputs. Android build identity belongs to the
//! runtime build, while memory granularity, entropy and the time zone cross
//! the Darwin boundary.
//! This does not implement mutable property-service transactions.
use darwin_art_engine::ProcessSnapshotInputs;

const BUILD_PROPERTIES: &str = include_str!("../../../runtime/framework/android-build.properties");

fn build_properties() -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
    let mut properties = Vec::new();
    let mut names = std::collections::HashSet::new();
    for line in BUILD_PROPERTIES.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or("invalid runtime build property")?;
        if name.is_empty() || !names.insert(name) || name.contains('\0') || value.contains('\0') {
            return Err("duplicate or invalid runtime build property".into());
        }
        properties.push((name.as_bytes().to_vec(), value.as_bytes().to_vec()));
    }
    Ok(properties)
}

/// The host's IANA time zone, from the `/etc/localtime` link macOS keeps
/// (`/var/db/timezone/zoneinfo/Asia/Seoul`). Android reads it from
/// `persist.sys.timezone` (bionic `localtime`, RuntimeInit's libcore
/// TimezoneGetter). An unreadable or unusual link leaves the property unset,
/// which Android treats as GMT, rather than guessing.
fn host_time_zone() -> Option<String> {
    zone_from_localtime_link(&std::fs::read_link("/etc/localtime").ok()?)
}

fn zone_from_localtime_link(link: &std::path::Path) -> Option<String> {
    let link = link.to_str()?;
    let (_, zone) = link.rsplit_once("/zoneinfo/")?;
    let valid = !zone.is_empty()
        && !zone.starts_with('/')
        && zone.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && zone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'));
    valid.then(|| zone.to_owned())
}

pub(super) fn inputs() -> Result<ProcessSnapshotInputs, String> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 || !(page_size as u64).is_power_of_two() {
        return Err("invalid host memory page size".into());
    }
    let mut random = [0u8; 16];
    // SAFETY: exact writable span; failure propagates rather than seeding AT_RANDOM.
    if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
        return Err(format!(
            "process entropy: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(ProcessSnapshotInputs {
        environment: [
            ("ANDROID_ROOT", "/system"),
            ("ANDROID_DATA", "/data"),
            ("ANDROID_STORAGE", "/storage"),
            ("EXTERNAL_STORAGE", "/storage/emulated/0"),
        ]
        .into_iter()
        .map(|(name, value)| (name.as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect(),
        properties: {
            let mut properties = build_properties()?;
            if let Some(zone) = host_time_zone() {
                properties.push((b"persist.sys.timezone".to_vec(), zone.into_bytes()));
            }
            properties
        },
        page_size: page_size as u64,
        // FP and ASIMD are guaranteed by this arm64 execution target. Do not
        // advertise optional Android instructions merely because macOS has them.
        hwcap: 3,
        hwcap2: 0,
        secure: false, // No set-ID guest exec is performed by the host launcher.
        random,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires rebuilt native engine; set DARWIN_ART_TEST_ENGINE_PATH"]
    fn native_engine_retains_snapshot_until_image_owner_drop() {
        use std::os::fd::AsFd;
        let path = std::env::var_os("DARWIN_ART_TEST_ENGINE_PATH").expect("native engine path");
        let path = std::path::Path::new(&path);
        let snapshot = inputs().unwrap();
        {
            let mut engine = darwin_art_engine::EngineSession::open(path).unwrap();
            engine.install_process_snapshot(&snapshot).unwrap();
            let directory = std::fs::File::open("/").unwrap();
            engine
                .install_process_filesystem(directory.as_fd(), b"/", b"/")
                .unwrap();
            assert!(
                engine
                    .install_process_filesystem(directory.as_fd(), b"/", b"/")
                    .is_err()
            );
            // Installation copied directory authority. The borrowed fd is not
            // retained; closing it cannot invalidate the installed owner.
            drop(directory);
            assert!(engine.install_process_snapshot(&snapshot).is_err());
            // Closing ART must not yet release a snapshot potentially still
            // used by graphics/provider teardown. Engine drop releases it.
            assert_eq!(engine.close(), 0);
            assert!(engine.install_process_snapshot(&snapshot).is_err());
            let directory = std::fs::File::open("/").unwrap();
            // A second image owner reaches the native duplicate check rather
            // than this engine's closed flag: close must not release the FS.
            let mut contender = darwin_art_engine::EngineSession::open(path).unwrap();
            assert!(matches!(
                contender.install_process_filesystem(directory.as_fd(), b"/", b"/"),
                Err(darwin_art_engine::ProcessFilesystemError::AlreadyInstalled)
            ));
            assert!(
                engine
                    .install_process_filesystem(directory.as_fd(), b"/", b"/")
                    .is_err()
            );
        }
        let mut next = darwin_art_engine::EngineSession::open(path).unwrap();
        next.install_process_snapshot(&snapshot).unwrap();
        let directory = std::fs::File::open("/").unwrap();
        next.install_process_filesystem(directory.as_fd(), b"/", b"/")
            .unwrap();
    }
    #[test]
    fn time_zone_comes_from_the_localtime_link() {
        use std::path::Path;
        assert_eq!(
            zone_from_localtime_link(Path::new("/var/db/timezone/zoneinfo/Asia/Seoul")),
            Some("Asia/Seoul".into())
        );
        assert_eq!(
            zone_from_localtime_link(Path::new("/usr/share/zoneinfo/America/Argentina/Salta")),
            Some("America/Argentina/Salta".into())
        );
        assert_eq!(
            zone_from_localtime_link(Path::new("/usr/share/zoneinfo/Etc/GMT+9")),
            Some("Etc/GMT+9".into())
        );
        assert_eq!(zone_from_localtime_link(Path::new("/etc/localtime.bak")), None);
        assert_eq!(zone_from_localtime_link(Path::new("/x/zoneinfo/../etc")), None);
        assert_eq!(zone_from_localtime_link(Path::new("/x/zoneinfo/")), None);
        let host = host_time_zone();
        let input = inputs().unwrap();
        let property = input
            .properties
            .iter()
            .find(|(key, _)| key == b"persist.sys.timezone")
            .map(|(_, value)| String::from_utf8(value.clone()).unwrap());
        assert_eq!(property, host);
    }
    #[test]
    fn runtime_identity_is_explicit_and_hardware_values_are_not_fabricated() {
        let input = inputs().unwrap();
        assert_eq!(
            input.page_size,
            unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64
        );
        assert!(
            input
                .properties
                .iter()
                .any(|(key, value)| key == b"ro.build.version.sdk" && value == b"36")
        );
        assert!(
            !input
                .properties
                .iter()
                .any(|(key, _)| key.starts_with(b"device.cpu."))
        );
        assert!(
            !input
                .properties
                .iter()
                .any(|(key, _)| key == b"bionic.linker.16kb.app_compat.enabled")
        );
    }
}
