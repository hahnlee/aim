//! `cargo aim storage [DATA...]`: what the guest's disk images occupy on
//! the host (docs/storage.md): the system image, the derived image's
//! shadow, and each data image (`cargo aim boot`'s, the bench's and those
//! named), with what their volumes use while attached.

use aim_storage::{data, disk};
use std::path::{Path, PathBuf};

fn mib(bytes: u64) -> String {
    format!("{:.0} MiB", bytes as f64 / (1 << 20) as f64)
}

fn row(what: &str, path: &Path, detail: String) {
    println!("{what:<16} {detail:<44} {}", path.display());
}

fn file(what: &str, path: &Path, mount: Option<&Path>) {
    match disk::allocated(path) {
        Ok(bytes) => {
            let attached = mount.is_some_and(disk::is_mount_point);
            let state = if attached { "attached" } else { "detached" };
            row(what, path, format!("{:>10}  {state}", mib(bytes)));
        }
        Err(_) => row(what, path, "absent".into()),
    }
}

/// Data directories with an image: under `dir`, one level down.
fn data_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "asif"))
        .map(|p| p.with_extension(""))
        .collect();
    out.sort();
    out
}

pub fn run(named: &[String]) -> Result<(), String> {
    file(
        "system image",
        &aim_paths::system_image(),
        Some(&aim_paths::system_image_mount()),
    );
    file(
        "derived shadow",
        &aim_paths::derived_image_shadow(),
        Some(&aim_paths::derived_image_mount()),
    );
    let mut dirs: Vec<PathBuf> = named.iter().map(PathBuf::from).collect();
    dirs.extend(data_dirs(&aim_paths::out().join("boot")));
    dirs.extend(data_dirs(&aim_paths::out().join("bench")));
    for dir in dirs {
        let image = data::image_of(&dir);
        match data::usage(&dir) {
            Ok(usage) => {
                let detail = match usage.used {
                    Some(used) => format!(
                        "{:>10}  attached, {} used, {} reclaimable",
                        mib(usage.allocated),
                        mib(used),
                        mib(usage.allocated.saturating_sub(used))
                    ),
                    None => format!("{:>10}  detached", mib(usage.allocated)),
                };
                row("data image", &image, detail);
            }
            Err(_) => row("data image", &image, "absent".into()),
        }
    }
    println!(
        "\nA data image returns what the guest deleted when it is detached; \
         guest-init compacts it at stop above {} reclaimable.",
        mib(data::RECLAIM_THRESHOLD)
    );
    Ok(())
}
