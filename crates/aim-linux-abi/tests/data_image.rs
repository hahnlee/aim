//! The guest's `/data` on its case-sensitive data image (docs/storage.md):
//! the image's own mksh creates names that differ only in case, a chown
//! survives a stop and a fresh runtime directory, as on a second boot of
//! the same data (#261), and a full volume gives the guest ENOSPC rather
//! than I/O errors.

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aim_storage::data::{self, DataImage};
use aim_storage::disk;

/// `sys/space.rs`: the host volume space the guest never gets.
const RESERVE: u64 = 1 << 30;

/// A path map of the original image with `/data` on the data image
/// mounted at `mount` and a fresh runtime directory.
fn path_map(image: &Path, mount: &Path, runtime: &Path) -> PathBuf {
    let _ = std::fs::remove_dir_all(runtime);
    for d in ["dev", "tmp", "kernfs/proc", "kernfs/sys"] {
        std::fs::create_dir_all(runtime.join(d)).unwrap();
    }
    std::fs::create_dir_all(mount.join("data")).unwrap();
    let map = runtime.join("path-map");
    let r = runtime.display();
    std::fs::write(
        &map,
        format!(
            "# aim-guest-init path map v1\nroot\t/\t{}\nrw\t/data\t{}/data\nrw\t/dev\t{r}/dev\nrw\t/tmp\t{r}/tmp\nkernfs\t/proc\t{r}/kernfs/proc\nkernfs\t/sys\t{r}/kernfs/sys\n",
            image.display(),
            mount.display()
        ),
    )
    .unwrap();
    map
}

fn sh(image: &Path, map: &Path, script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--root")
        .arg(image)
        .arg("--path-map")
        .arg(map)
        .args(["/system/bin/sh", "-c", script])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{script}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn guest_data_is_case_sensitive_and_keeps_owners() {
    let Some(image) = aim_paths::original_image_with("system/bin/sh") else {
        return;
    };
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let dir = tmp.join(format!("data-image-{}", std::process::id()));
    let runtime = tmp.join(format!("data-image-{}-run", std::process::id()));
    data::remove(&dir).unwrap();

    let volume = DataImage::attach(&dir, None).unwrap();
    let map = path_map(&image, volume.dir(), &runtime);
    let out = sh(
        &image,
        &map,
        "echo upper > /data/Foo && echo lower > /data/foo && mkdir /data/app1 && \
         chown 10123:10123 /data/app1 && cat /data/Foo /data/foo && ls /data",
    );
    assert_eq!(out, "upper\nlower\nFoo\napp1\nfoo\n");
    volume.detach().unwrap();

    // A second boot: the same data, a new runtime directory.
    let volume = DataImage::attach(&dir, None).unwrap();
    let map = path_map(&image, volume.dir(), &runtime);
    let out = sh(&image, &map, "stat -c '%u:%g' /data/app1 && cat /data/foo");
    assert_eq!(out, "10123:10123\nlower\n");
    drop(volume);
    data::remove(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// Bytes free on the volume holding `path`.
fn free(path: &Path) -> u64 {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path, local buffer.
    assert_eq!(unsafe { libc::statfs(c.as_ptr(), &mut fs) }, 0);
    fs.f_bavail * u64::from(fs.f_bsize)
}

/// `fsck_apfs -n` on the volume of the detached image `image`.
fn fsck(image: &Path) -> std::process::Output {
    let device = disk::attach(image, disk::Attach::default()).unwrap();
    let volume = disk::attachments_of(image)
        .unwrap()
        .into_iter()
        .find(|a| a.device == device)
        .and_then(|a| {
            a.devices
                .into_iter()
                .find(|d| d.trim_start_matches("/dev/disk").contains('s'))
        })
        .unwrap();
    let out = Command::new("fsck_apfs")
        .args(["-n", &volume])
        .output()
        .unwrap();
    disk::detach(&device, Duration::from_secs(20)).unwrap();
    out
}

/// The guest's `/data` on a small disposable volume that fills up: the
/// guest sees the volume's free space less the reserve, a write past it
/// fails with ENOSPC, the volume keeps its reserve, and it stays
/// consistent. (A data image's own APFS reports its host volume's free
/// space, so the same holds for a data image on a filling Mac.)
#[test]
fn a_full_volume_gives_the_guest_enospc() {
    let Some(image) = aim_paths::original_image_with("system/bin/sh") else {
        return;
    };
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let volume = tmp.join(format!("full-volume-{}", std::process::id()));
    let volume_image = volume.with_extension("asif");
    let runtime = tmp.join(format!("full-volume-{}-run", std::process::id()));
    disk::create_case_sensitive(&volume_image, "3g", "aim-full").unwrap();
    std::fs::create_dir_all(&volume).unwrap();
    let device = disk::attach(
        &volume_image,
        disk::Attach {
            mount: Some(&volume),
            ..Default::default()
        },
    )
    .unwrap();
    let map = path_map(&image, &volume, &runtime);
    // Leave the guest 256 MiB: take the rest of the free space (allocated,
    // not written).
    let filler = std::fs::File::create(volume.join("filler")).unwrap();
    let mut store = libc::fstore_t {
        fst_flags: libc::F_ALLOCATEALL,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: (free(&volume) - RESERVE - (256 << 20)) as i64,
        fst_bytesalloc: 0,
    };
    // SAFETY: an open descriptor and a local fstore_t.
    assert_eq!(
        unsafe { libc::fcntl(filler.as_raw_fd(), libc::F_PREALLOCATE, &mut store) },
        0
    );

    let out = sh(
        &image,
        &map,
        "stat -f -c '%a %S' /data; dd if=/dev/zero of=/data/big bs=1048576 2>&1; \
         echo dd=$?; dd if=/dev/zero of=/data/small bs=1 count=1 2>/dev/null; \
         echo small=$?; stat -f -c '%a' /data; \
         stat -c %s /data/big",
    );
    eprintln!("{out}");
    let lines: Vec<&str> = out.lines().collect();
    let (avail, bsize) = lines[0].split_once(' ').unwrap();
    let before = avail.parse::<u64>().unwrap() * bsize.parse::<u64>().unwrap();
    assert!(
        (200 << 20..=256 << 20).contains(&before),
        "the guest saw {before} bytes free"
    );
    assert!(out.contains("No space left on device"), "{out}");
    assert!(!out.contains("I/O error"), "{out}");
    assert!(out.contains("\nsmall=1\n"), "{out}");
    let written: u64 = lines[lines.len() - 1].parse().unwrap();
    let after: u64 = lines[lines.len() - 2].parse().unwrap();
    assert_eq!(after, 0);
    assert!(written >= 128 << 20, "the guest wrote {written} bytes");
    let left = free(&volume);
    assert!(left >= RESERVE / 2, "the volume kept {left} bytes");

    drop(filler);
    disk::detach(&device, Duration::from_secs(20)).unwrap();
    let check = fsck(&volume_image);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    std::fs::remove_file(&volume_image).unwrap();
    let _ = std::fs::remove_dir_all(&volume);
    let _ = std::fs::remove_dir_all(&runtime);
}
