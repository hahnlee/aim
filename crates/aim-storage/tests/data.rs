//! Data images on the real disk image tools (docs/storage.md): each test
//! creates, attaches and removes its own image in cargo's target tmp.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use aim_storage::data::{self, DataImage};
use aim_storage::disk;

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("storage-{name}-{}", std::process::id()));
    let _ = data::remove(&dir);
    dir
}

#[test]
fn names_that_differ_only_in_case_coexist() {
    let dir = dir("case");
    let image = DataImage::attach(&dir).unwrap();
    assert!(disk::is_mount_point(&dir));
    fs::write(dir.join("Foo"), "upper").unwrap();
    fs::write(dir.join("foo"), "lower").unwrap();
    assert_eq!(fs::read_to_string(dir.join("Foo")).unwrap(), "upper");
    assert_eq!(fs::read_to_string(dir.join("foo")).unwrap(), "lower");
    image.detach().unwrap();
    assert!(!disk::is_mount_point(&dir));

    // The data persists in the image.
    let image = DataImage::attach(&dir).unwrap();
    assert_eq!(fs::read_to_string(dir.join("foo")).unwrap(), "lower");
    drop(image);
    data::remove(&dir).unwrap();
    assert!(!data::image_of(&dir).exists());
}

#[test]
fn deleted_data_returns_to_the_host_at_stop() {
    let dir = dir("reclaim");
    let image = DataImage::attach(&dir).unwrap();
    let empty = data::usage(&dir).unwrap().allocated;
    let mut file = fs::File::create(dir.join("big")).unwrap();
    // Incompressible, so the volume really stores 1 GiB.
    let mut block = vec![0u8; 1 << 20];
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..1024 {
        for chunk in block.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    let written = data::usage(&dir).unwrap();
    eprintln!("empty {empty}, written {written:?}");
    assert!(written.allocated >= empty + (1 << 30) - (64 << 20));
    fs::remove_file(dir.join("big")).unwrap();
    image.detach().unwrap();
    let stopped = data::usage(&dir).unwrap().allocated;
    eprintln!("after delete and stop {stopped}");
    assert!(
        stopped < empty + (64 << 20),
        "the image kept {} MiB of the deleted GiB",
        (stopped - empty) >> 20
    );
    data::remove(&dir).unwrap();
}

#[test]
fn one_user_and_recovery_from_a_crash() {
    let dir = dir("lock");
    let image = DataImage::attach(&dir).unwrap();
    let second = DataImage::attach(&dir).unwrap_err();
    assert!(second.contains("in use"), "{second}");
    drop(image);

    // A holder that died without detaching: attached, but nobody holds
    // the lock.
    let device = disk::attach(
        &data::image_of(&dir),
        disk::Attach {
            mount: Some(&dir),
            ..Default::default()
        },
    )
    .unwrap();
    fs::write(dir.join("kept"), "x").unwrap();
    let image = DataImage::attach(&dir).unwrap();
    assert!(
        disk::attached()
            .unwrap()
            .iter()
            .all(|a| a.device != device || a.mounts.contains(&fs::canonicalize(&dir).unwrap()))
    );
    assert_eq!(fs::read_to_string(dir.join("kept")).unwrap(), "x");
    drop(image);
    data::remove(&dir).unwrap();
}

#[test]
fn old_plain_data_directories_are_refused() {
    let dir = dir("plain");
    fs::create_dir_all(dir.join("data")).unwrap();
    let error = DataImage::attach(&dir).unwrap_err();
    assert!(error.contains("before data images"), "{error}");
    fs::remove_dir_all(&dir).unwrap();
}
