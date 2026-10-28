//! Read-only failure snapshots, taken before the disposable data image detaches.
use std::{ffi::CString, fmt::Write, os::unix::fs::MetadataExt, path::Path};

fn volume(path: &Path) -> std::io::Result<String> {
    let path = CString::new(path.as_os_str().as_encoded_bytes())?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path and writable local statfs buffer.
    if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statfs initializes and NUL-terminates both mount names.
    let (mount, device) = unsafe {
        (
            std::ffi::CStr::from_ptr(stat.f_mntonname.as_ptr()),
            std::ffi::CStr::from_ptr(stat.f_mntfromname.as_ptr()),
        )
    };
    let block = u64::from(stat.f_bsize);
    Ok(format!(
        "mount={mount:?} device={device:?} capacity_bytes={} free_bytes={} available_bytes={}",
        stat.f_blocks.saturating_mul(block),
        stat.f_bfree.saturating_mul(block),
        stat.f_bavail.saturating_mul(block),
    ))
}

pub fn report(data: &Path) -> String {
    let image = aim_storage::data::image_of(data);
    let runtime = aim_storage::data::runtime_of(data);
    let mut report = String::from(
        "storage snapshot (host statfs, before cleanup; guest sys/space.rs also retains its 1 GiB reserve):\n",
    );
    // The image file lives on the host, whereas /data lives inside its mount.
    // Report both even on APFS, where thin provisioning can couple their space.
    for (owner, path) in [
        ("host-image", image.as_path()),
        ("guest-data", data.join("data").as_path()),
        ("host-runtime", runtime.as_path()),
    ] {
        match volume(path) {
            Ok(snapshot) => writeln!(report, "{owner} path={path:?} {snapshot}").unwrap(),
            Err(error) => writeln!(report, "{owner} path={path:?} error={error}").unwrap(),
        }
    }
    match std::fs::metadata(&image) {
        Ok(metadata) => writeln!(
            report,
            "image-file path={image:?} logical_bytes={} allocated_bytes={}",
            metadata.len(),
            metadata.blocks().saturating_mul(512),
        )
        .unwrap(),
        Err(error) => writeln!(report, "image-file path={image:?} error={error}").unwrap(),
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_distinguishes_backing_mount_and_sparse_file_sizes_without_mutation() {
        let root =
            std::env::temp_dir().join(format!("aim-storage-diagnostic-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let data = root.join("guest");
        std::fs::create_dir_all(data.join("data")).unwrap();
        std::fs::create_dir(aim_storage::data::runtime_of(&data)).unwrap();
        let image = aim_storage::data::image_of(&data);
        let file = std::fs::File::create_new(&image).unwrap();
        file.set_len(16 << 20).unwrap();
        let before = file.metadata().unwrap();
        let snapshot = report(&data);
        for owner in ["host-image", "guest-data", "host-runtime"] {
            let line = snapshot
                .lines()
                .find(|line| line.starts_with(owner))
                .unwrap();
            for field in [
                "path=",
                "mount=",
                "device=",
                "capacity_bytes=",
                "free_bytes=",
                "available_bytes=",
            ] {
                assert!(line.contains(field), "{snapshot}");
            }
        }
        assert!(snapshot.contains("logical_bytes=16777216"), "{snapshot}");
        assert!(snapshot.contains(&format!("allocated_bytes={}", before.blocks() * 512)));
        let after = file.metadata().unwrap();
        assert_eq!(
            (before.len(), before.blocks()),
            (after.len(), after.blocks())
        );
        drop(file);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_owners_are_reported_independently_without_creating_them() {
        let root = std::env::temp_dir().join(format!("aim-storage-missing-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let snapshot = report(&root.join("guest"));
        for owner in ["host-image", "guest-data", "host-runtime", "image-file"] {
            let line = snapshot
                .lines()
                .find(|line| line.starts_with(owner))
                .unwrap();
            assert!(line.contains("error="), "{snapshot}");
        }
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }
}
