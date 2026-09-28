//! The guest's `/data` on its case-sensitive data image (docs/storage.md):
//! the image's own mksh creates names that differ only in case, and a
//! chown survives a stop and a fresh runtime directory, as on a second
//! boot of the same data (#261).

use std::path::{Path, PathBuf};
use std::process::Command;

use aim_storage::data::{self, DataImage};

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

    let volume = DataImage::attach(&dir).unwrap();
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
    let volume = DataImage::attach(&dir).unwrap();
    let map = path_map(&image, volume.dir(), &runtime);
    let out = sh(&image, &map, "stat -c '%u:%g' /data/app1 && cat /data/foo");
    assert_eq!(out, "10123:10123\nlower\n");
    drop(volume);
    data::remove(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}
