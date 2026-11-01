//! Snapshot/restore an entire stopped disposable DATA image, including sync/job/cache companions.
use aim_storage::{data, disk};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, os::fd::AsRawFd, path::Path};

fn digest(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 1024 * 1024];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn regular(path: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err(format!("{} is not a regular image", path.display()));
    }
    Ok(())
}
fn copy(source: &Path, target: &Path) -> Result<(), String> {
    fs::copy(source, target).map_err(|e| e.to_string())?;
    fs::File::open(target)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
fn sibling(data: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = data.file_name().unwrap().to_os_string();
    name.push(suffix);
    data.with_file_name(name)
}
fn transfer(mode: &str, data: &Path, archive: &Path, cohort: &str) -> Result<(), String> {
    if !matches!(mode, "snapshot" | "restore")
        || cohort.len() != 64
        || !cohort.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("expected snapshot|restore DATA ARCHIVE MATCHED_COHORT_SHA256".into());
    }
    let data = data::mount_point(data)?;
    if fs::read(sibling(&data, ".disposable")).map_err(|e| e.to_string())?
        != b"AIM disposable cohort\n"
    {
        return Err("DATA requires a sibling .disposable marker".into());
    }
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(sibling(&data, ".lock"))
        .map_err(|e| e.to_string())?;
    // SAFETY: advisory lock on our open DATA lock descriptor, shared with guest-init.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("DATA still has a live guest owner".into());
    }
    let image = data::image_of(&data);
    regular(&image)?;
    if disk::is_mount_point(&data) || !disk::attachments_of(&image)?.is_empty() {
        return Err("DATA must be stopped and detached".into());
    }
    let saved = archive.join("data.asif");
    let identity = archive.join("identity");
    if mode == "snapshot" {
        fs::create_dir(archive).map_err(|e| e.to_string())?;
        copy(&image, &saved)?;
        let receipt = format!("{cohort}\n{}\n", digest(&saved)?);
        fs::write(&identity, receipt).map_err(|e| e.to_string())?;
        fs::File::open(&identity)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::File::open(archive)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::File::open(
            archive
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    } else {
        regular(&saved)?;
        if !disk::attachments_of(&saved)?.is_empty() {
            return Err("snapshot is attached".into());
        }
        let receipt = fs::read_to_string(identity).map_err(|e| e.to_string())?;
        if receipt != format!("{cohort}\n{}\n", digest(&saved)?) {
            return Err("snapshot digest or matched runtime cohort differs".into());
        }
        let stage = sibling(&data, &format!(".restore-{}", std::process::id()));
        fs::create_dir(&stage).map_err(|e| e.to_string())?;
        let result = (|| {
            let temporary = stage.join("data.asif");
            copy(&saved, &temporary)?;
            if receipt != format!("{cohort}\n{}\n", digest(&temporary)?) {
                return Err("staged snapshot digest differs".into());
            }
            fs::rename(temporary, &image).map_err(|e| e.to_string())
        })();
        let cleanup = fs::remove_dir_all(stage).map_err(|e| e.to_string());
        result.and(cleanup)?;
    }
    fs::File::open(data.parent().unwrap())
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "usage: aim-data-cohort snapshot|restore DATA ARCHIVE MATCHED_COHORT_SHA256".into(),
        );
    }
    transfer(&args[0], Path::new(&args[1]), Path::new(&args[2]), &args[3])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopped_disposable_cohort_roundtrip_and_rejections() {
        let root = aim_paths::scratch_dir("sync-data-cohort");
        fs::create_dir_all(&root).unwrap();
        let data = root.join("data.with.dots");
        let image = data::image_of(&data);
        let archive = root.join("snapshot");
        let cohort = "a".repeat(64);
        let original =
            b"whole ASIF: accounts.xml.bak/status.bin/stats.new/job/users/registered_services";
        fs::write(&image, original).unwrap();
        assert!(transfer("snapshot", &data, &archive, &cohort).is_err());
        fs::write(sibling(&data, ".disposable"), b"AIM disposable cohort\n").unwrap();
        let lock = fs::File::create(sibling(&data, ".lock")).unwrap();
        // SAFETY: hold the same advisory DATA lock as a live guest-init.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert!(transfer("snapshot", &data, &archive, &cohort).is_err());
        drop(lock);
        transfer("snapshot", &data, &archive, &cohort).unwrap();
        assert!(transfer("snapshot", &data, &archive, &cohort).is_err());
        fs::write(&image, b"changed entire cohort").unwrap();
        assert!(transfer("restore", &data, &archive, &"b".repeat(64)).is_err());
        assert_eq!(fs::read(&image).unwrap(), b"changed entire cohort");
        transfer("restore", &data, &archive, &cohort).unwrap();
        assert_eq!(fs::read(&image).unwrap(), original);
        fs::write(archive.join("data.asif"), b"tampered").unwrap();
        assert!(transfer("restore", &data, &archive, &cohort).is_err());
        assert_eq!(fs::read(&image).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }
}
