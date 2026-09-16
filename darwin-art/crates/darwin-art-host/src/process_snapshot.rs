//! Construct process bootstrap inputs. Android build identity belongs to the
//! runtime build, while memory granularity and entropy cross the Darwin boundary.
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
        properties: build_properties()?,
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
