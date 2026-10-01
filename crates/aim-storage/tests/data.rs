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
    let image = DataImage::attach(&dir, None).unwrap();
    assert!(disk::is_mount_point(&dir));
    // Made at its ceiling, so the next attach need not grow it.
    let ceiling = data::ceiling(&data::image_of(&dir)).unwrap();
    assert_eq!(disk::capacity(&dir), Some(ceiling));
    fs::write(dir.join("Foo"), "upper").unwrap();
    fs::write(dir.join("foo"), "lower").unwrap();
    assert_eq!(fs::read_to_string(dir.join("Foo")).unwrap(), "upper");
    assert_eq!(fs::read_to_string(dir.join("foo")).unwrap(), "lower");
    image.detach().unwrap();
    assert!(!disk::is_mount_point(&dir));

    // The data persists in the image.
    let image = DataImage::attach(&dir, None).unwrap();
    assert_eq!(fs::read_to_string(dir.join("foo")).unwrap(), "lower");
    drop(image);
    data::remove(&dir).unwrap();
    assert!(!data::image_of(&dir).exists());
}

#[test]
fn deleted_data_returns_to_the_host_at_stop() {
    let dir = dir("reclaim");
    let image = DataImage::attach(&dir, None).unwrap();
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
    let image = DataImage::attach(&dir, None).unwrap();
    let second = DataImage::attach(&dir, None).unwrap_err();
    assert!(second.contains("in use"), "{second}");
    drop(image);

    // A holder that died without detaching: attached, but nobody holds
    // the lock.
    let crashed = disk::attach(
        &data::image_of(&dir),
        disk::Attach {
            mount: Some(&dir),
            ..Default::default()
        },
    )
    .unwrap();
    fs::write(dir.join("kept"), "x").unwrap();
    let image = DataImage::attach(&dir, None).unwrap();
    // Found by the image, not the device: the crashed attachment's device
    // is free once detached, and any attach (the other tests here, other
    // processes) may get it.
    let now = disk::attachments_of(&data::image_of(&dir)).unwrap();
    assert_eq!(now.len(), 1, "{now:?}");
    assert!(
        now[0].mounts.contains(&fs::canonicalize(&dir).unwrap()),
        "{now:?}"
    );
    let (_, from) = disk::mount_of(&dir).unwrap();
    assert!(
        now[0].devices.contains(&from),
        "{from} not in {now:?} (crashed: {crashed})"
    );
    assert_eq!(fs::read_to_string(dir.join("kept")).unwrap(), "x");
    drop(image);
    data::remove(&dir).unwrap();
}

#[test]
fn old_plain_data_directories_are_refused() {
    let dir = dir("plain");
    fs::create_dir_all(dir.join("data")).unwrap();
    let error = DataImage::attach(&dir, None).unwrap_err();
    assert!(error.contains("before data images"), "{error}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_image_grows_to_its_host_volume_and_keeps_its_data() {
    let dir = dir("grow");
    let image = data::image_of(&dir);
    disk::create_case_sensitive(&image, "1g", "aim-data").unwrap();
    fs::create_dir_all(&dir).unwrap();
    let device = disk::attach(
        &image,
        disk::Attach {
            mount: Some(&dir),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(disk::capacity(&dir).unwrap() <= 1 << 30);
    fs::write(dir.join("kept"), "x").unwrap();
    disk::detach(&device, std::time::Duration::from_secs(20)).unwrap();

    let attached = DataImage::attach(&dir, None).unwrap();
    let ceiling = data::ceiling(&image).unwrap();
    assert_eq!(disk::capacity(&dir), Some(ceiling));
    assert_eq!(fs::read_to_string(dir.join("kept")).unwrap(), "x");
    let before = data::usage(&dir).unwrap().allocated;
    drop(attached);
    // Nothing is preallocated: the file holds what the volume uses.
    assert!(before < 64 << 20, "the grown image holds {before} bytes");
    data::remove(&dir).unwrap();
}

#[test]
fn data_directories_start_from_a_template_apart() {
    let template = dir("template");
    let template_image = data::image_of(&template);
    data::create(&template_image).unwrap();
    let image = DataImage::attach(&template, None).unwrap();
    fs::write(template.join("shipped"), "t").unwrap();
    drop(image);

    // Two data directories from it, attached at once: each has what the
    // template had, and its own writes.
    let (one, two) = (dir("from-a"), dir("from-b"));
    let a = DataImage::attach(&one, Some(&template_image)).unwrap();
    let b = DataImage::attach(&two, Some(&template_image)).unwrap();
    for d in [&one, &two] {
        assert_eq!(fs::read_to_string(d.join("shipped")).unwrap(), "t");
    }
    fs::write(one.join("mine"), "a").unwrap();
    fs::remove_file(one.join("shipped")).unwrap();
    assert!(!two.join("mine").exists());
    assert!(two.join("shipped").exists());
    drop((a, b));

    // The template is only read when there is no image.
    let a = DataImage::attach(&one, Some(&template_image)).unwrap();
    assert_eq!(fs::read_to_string(one.join("mine")).unwrap(), "a");
    assert!(!one.join("shipped").exists());
    drop(a);
    for d in [&one, &two, &template] {
        data::remove(d).unwrap();
    }
}
