use crate::Result;
use darwin_art_build_contract::support_java::{production_sources, SOURCE_MANIFEST};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const WM_SOURCE_PREFIX: &str = "runtime/framework/wm/";

fn wm_sources_from_manifest(manifest: &str) -> Result<Vec<PathBuf>> {
    let sources = production_sources(manifest)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    let mut wm_sources = sources
        .into_iter()
        .filter(|source| source.to_string_lossy().starts_with(WM_SOURCE_PREFIX))
        .collect::<Vec<_>>();
    wm_sources.sort();
    if wm_sources.is_empty() {
        return Err("production Java manifest has no WMS sources".into());
    }
    Ok(wm_sources)
}

pub(crate) fn append_wm_sources(root: &Path, command: &mut Command) -> Result<()> {
    let manifest = fs::read_to_string(root.join(SOURCE_MANIFEST))?;
    for source in wm_sources_from_manifest(&manifest)? {
        let path = root.join(source);
        if !path.is_file() {
            return Err(format!("WMS source is missing: {}", path.display()).into());
        }
        command.arg(path);
    }
    Ok(())
}

fn validated_wm_package_dir(class_dir: &Path) -> Result<Option<PathBuf>> {
    validated_package_dir(class_dir, "wm")
}

fn validated_package_dir(class_dir: &Path, subsystem: &str) -> Result<Option<PathBuf>> {
    let class_metadata = fs::symlink_metadata(class_dir)?;
    if class_metadata.file_type().is_symlink() || !class_metadata.is_dir() {
        return Err(format!(
            "class output root is not a directory: {}",
            class_dir.display()
        )
        .into());
    }
    let mut package_dir = class_dir.to_owned();
    for component in ["dev", "darwinart", "runtime", subsystem] {
        package_dir.push(component);
        let metadata = match fs::symlink_metadata(&package_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "Framework class output path component is not a directory: {}",
                package_dir.display()
            )
            .into());
        }
    }
    Ok(Some(package_dir))
}

pub(crate) fn clear_wm_class_output(class_dir: &Path) -> Result<()> {
    if let Some(package_dir) = validated_wm_package_dir(class_dir)? {
        fs::remove_dir_all(package_dir)?;
    }
    Ok(())
}

fn collect_package_classes(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(format!("symlink in framework compiler output: {}", path.display()).into());
        }
        if metadata.is_dir() {
            collect_package_classes(root, &path, output)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("class") {
            if !metadata.is_file() {
                return Err(
                    format!("unsupported framework class output: {}", path.display()).into(),
                );
            }
            output.push(path.strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}

pub(crate) fn wm_class_files(class_dir: &Path) -> Result<Vec<PathBuf>> {
    let package_dir = validated_wm_package_dir(class_dir)?.ok_or_else(|| {
        format!(
            "WMS class output is unavailable under {}",
            class_dir.display()
        )
    })?;
    let mut classes = Vec::new();
    collect_package_classes(class_dir, &package_dir, &mut classes)?;
    classes.sort();
    classes.dedup();
    if classes.is_empty() {
        return Err("WMS compiler output contains no class files".into());
    }
    Ok(classes)
}

pub(crate) fn append_wm_classes(class_dir: &Path, command: &mut Command) -> Result<()> {
    for class_file in wm_class_files(class_dir)? {
        command.arg(class_dir.join(class_file));
    }
    Ok(())
}

pub(crate) fn clear_am_class_output(class_dir: &Path) -> Result<()> {
    if let Some(package_dir) = validated_package_dir(class_dir, "am")? {
        fs::remove_dir_all(package_dir)?;
    }
    Ok(())
}

pub(crate) fn append_am_classes(class_dir: &Path, command: &mut Command) -> Result<()> {
    let package_dir =
        validated_package_dir(class_dir, "am")?.ok_or("AMS compiler output is unavailable")?;
    let mut classes = Vec::new();
    collect_package_classes(class_dir, &package_dir, &mut classes)?;
    classes.sort();
    classes.dedup();
    if classes.is_empty() {
        return Err("AMS compiler output contains no class files".into());
    }
    for class_file in classes {
        command.arg(class_dir.join(class_file));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn am_nested_inputs_and_stale_cleanup_are_package_scoped() {
        let class_dir = temporary_directory("am-args");
        let package_dir = class_dir.join("dev/darwinart/runtime/am");
        let sibling = class_dir.join("dev/darwinart/runtime/wm");
        fs::create_dir_all(&package_dir).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(package_dir.join("Controller$Attempt.class"), b"class").unwrap();
        fs::write(package_dir.join("Controller.class"), b"class").unwrap();
        fs::write(sibling.join("keep.class"), b"class").unwrap();
        let mut command = Command::new("d8");
        append_am_classes(&class_dir, &mut command).unwrap();
        assert_eq!(
            command.get_args().map(PathBuf::from).collect::<Vec<_>>(),
            vec![
                package_dir.join("Controller$Attempt.class"),
                package_dir.join("Controller.class"),
            ]
        );
        clear_am_class_output(&class_dir).unwrap();
        assert!(!package_dir.exists());
        assert!(sibling.join("keep.class").is_file());
        assert!(append_am_classes(&class_dir, &mut Command::new("d8")).is_err());
        fs::remove_dir_all(class_dir).unwrap();
    }

    #[test]
    fn am_package_symlink_is_not_followed_or_deleted() {
        let class_dir = temporary_directory("am-symlink");
        let outside = temporary_directory("am-outside");
        fs::create_dir_all(&outside).unwrap();
        let parent = class_dir.join("dev/darwinart/runtime");
        fs::create_dir_all(&parent).unwrap();
        fs::write(outside.join("keep.class"), b"class").unwrap();
        symlink(&outside, parent.join("am")).unwrap();
        assert!(clear_am_class_output(&class_dir).is_err());
        assert!(append_am_classes(&class_dir, &mut Command::new("d8")).is_err());
        assert!(outside.join("keep.class").is_file());
        fs::remove_file(parent.join("am")).unwrap();
        fs::remove_dir_all(class_dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(name: &str) -> PathBuf {
        // Parallel tests can read the same clock value (#21).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "darwin-art-{name}-{}-{nonce}-{serial}",
            std::process::id()
        ))
    }

    #[test]
    fn projects_only_authoritative_wm_sources() {
        let manifest = concat!(
            "runtime/framework/am/ActivityManagerEndpoint.java\n",
            "runtime/framework/wm/WindowManagerEndpoint.java\n",
            "runtime/framework/wm/WindowFocusRegistry.java\n",
        );
        assert_eq!(
            wm_sources_from_manifest(manifest).unwrap(),
            vec![
                PathBuf::from("runtime/framework/wm/WindowFocusRegistry.java"),
                PathBuf::from("runtime/framework/wm/WindowManagerEndpoint.java"),
            ]
        );
    }

    #[test]
    fn rejects_malformed_or_non_production_catalog_entries() {
        for manifest in [
            "runtime/framework/../Probe.java\n",
            "probes/ProbeContext.java\n",
            "runtime/framework/compile-stubs/android/net/NetworkCapabilities.java\n",
            "runtime/framework/am/ActivityManagerEndpoint.java\n",
        ] {
            assert!(wm_sources_from_manifest(manifest).is_err(), "{manifest}");
        }
    }

    #[test]
    fn discovers_nested_wm_classes_in_deterministic_order() {
        let class_dir = temporary_directory("wm-classes");
        let package_dir = class_dir.join("dev/darwinart/runtime/wm");
        fs::create_dir_all(package_dir.join("nested")).unwrap();
        fs::write(package_dir.join("WindowManagerEndpoint.class"), b"class").unwrap();
        fs::write(
            package_dir.join("nested/WindowManagerEndpoint$State.class"),
            b"class",
        )
        .unwrap();
        fs::write(package_dir.join("ignored.txt"), b"not a class").unwrap();
        assert_eq!(
            wm_class_files(&class_dir).unwrap(),
            vec![
                PathBuf::from("dev/darwinart/runtime/wm/WindowManagerEndpoint.class"),
                PathBuf::from("dev/darwinart/runtime/wm/nested/WindowManagerEndpoint$State.class"),
            ]
        );
        fs::remove_dir_all(class_dir).unwrap();
    }

    #[test]
    fn clears_only_the_validated_wm_output_package() {
        let class_dir = temporary_directory("wm-clear");
        let package_dir = class_dir.join("dev/darwinart/runtime/wm");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("stale.class"), b"class").unwrap();
        let sibling = class_dir.join("dev/darwinart/runtime/am");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("keep.class"), b"class").unwrap();
        clear_wm_class_output(&class_dir).unwrap();
        assert!(!package_dir.exists());
        assert!(sibling.join("keep.class").is_file());
        fs::remove_dir_all(class_dir).unwrap();
    }

    #[test]
    fn rejects_symlinked_path_components_without_deleting_outside_output() {
        let class_dir = temporary_directory("wm-symlink");
        let parent = class_dir.join("dev/darwinart");
        fs::create_dir_all(&parent).unwrap();
        let outside = temporary_directory("wm-outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.class"), b"class").unwrap();
        symlink(&outside, parent.join("runtime")).unwrap();
        assert!(clear_wm_class_output(&class_dir).is_err());
        assert!(outside.join("keep.class").is_file());
        fs::remove_file(parent.join("runtime")).unwrap();
        fs::remove_dir_all(&class_dir).unwrap();

        let class_dir = temporary_directory("wm-broken");
        let package_parent = class_dir.join("dev/darwinart/runtime");
        fs::create_dir_all(&package_parent).unwrap();
        symlink("missing-wm-output", package_parent.join("wm")).unwrap();
        assert!(wm_class_files(&class_dir).is_err());
        assert!(clear_wm_class_output(&class_dir).is_err());
        fs::remove_file(package_parent.join("wm")).unwrap();
        fs::remove_dir_all(class_dir).unwrap();
    }

    #[test]
    fn appends_each_nested_class_once_in_sorted_order() {
        let class_dir = temporary_directory("wm-args");
        let package_dir = class_dir.join("dev/darwinart/runtime/wm");
        fs::create_dir_all(package_dir.join("nested")).unwrap();
        fs::write(package_dir.join("Z.class"), b"class").unwrap();
        fs::write(package_dir.join("nested/A$Inner.class"), b"class").unwrap();
        let mut command = Command::new("d8");
        append_wm_classes(&class_dir, &mut command).unwrap();
        let args = command.get_args().map(PathBuf::from).collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                package_dir.join("Z.class"),
                package_dir.join("nested/A$Inner.class"),
            ]
        );
        fs::remove_dir_all(class_dir).unwrap();
    }
}
