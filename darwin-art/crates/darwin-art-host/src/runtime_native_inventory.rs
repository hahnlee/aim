//! Enumerate non-system Mach-O dependencies without loading/executing them.
//! This inventory feeds trusted runtime identity, not a replacement for dyld.
use crate::macho_load_commands;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

pub(crate) fn collect(executable: &Path, roots: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    if !executable.is_absolute() {
        return Err(invalid("native inventory requires an absolute executable"));
    }
    let executable = fs::canonicalize(executable)?;
    let executable_directory = executable
        .parent()
        .ok_or_else(|| invalid("executable has no directory"))?;
    let mut files = BTreeSet::new();
    visit(&executable, executable_directory, &[], &mut files, 0)?;
    let commands = read_commands(&executable)?;
    let runpaths = commands
        .runpaths
        .iter()
        .map(|path| expand(path, executable_directory, executable_directory))
        .collect::<io::Result<Vec<_>>>()?;
    for root in roots {
        if !root.is_absolute() {
            return Err(invalid("native inventory requires absolute roots"));
        }
        visit(root, executable_directory, &runpaths, &mut files, 0)?;
    }
    Ok(files.into_iter().collect())
}

fn visit(
    path: &Path,
    executable: &Path,
    inherited: &[PathBuf],
    files: &mut BTreeSet<PathBuf>,
    depth: usize,
) -> io::Result<()> {
    if depth > 64 || files.len() >= 512 {
        return Err(invalid("native dependency inventory limit exceeded"));
    }
    let path = fs::canonicalize(path)?;
    if !files.insert(path.clone()) {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("native image has no directory"))?;
    let commands = read_commands(&path)?;
    let mut runpaths = commands
        .runpaths
        .iter()
        .map(|value| expand(value, parent, executable))
        .collect::<io::Result<Vec<_>>>()?;
    runpaths.extend_from_slice(inherited);
    for dependency in commands.dependencies {
        let candidates = if let Some(suffix) = dependency.path.strip_prefix("@rpath/") {
            runpaths
                .iter()
                .map(|root| root.join(suffix))
                .collect::<Vec<_>>()
        } else {
            vec![expand(&dependency.path, parent, executable)?]
        };
        let mut selected = None;
        for candidate in candidates {
            if host_system(&candidate) {
                selected = Some(None);
                break;
            }
            match fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_file() => {
                    selected = Some(Some(candidate));
                    break;
                }
                Ok(_) => return Err(invalid("native dependency is not a regular file")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match selected {
            Some(Some(path)) => visit(&path, executable, &runpaths, files, depth + 1)?,
            Some(None) => {}
            None if dependency.weak => {}
            None => {
                return Err(invalid(&format!(
                    "unresolved native dependency {} from {}",
                    dependency.path,
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn expand(value: &str, loader: &Path, executable: &Path) -> io::Result<PathBuf> {
    if value == "@loader_path" {
        return Ok(loader.into());
    }
    if value == "@executable_path" {
        return Ok(executable.into());
    }
    if let Some(suffix) = value.strip_prefix("@loader_path/") {
        return Ok(loader.join(suffix));
    }
    if let Some(suffix) = value.strip_prefix("@executable_path/") {
        return Ok(executable.join(suffix));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(invalid("unsupported relative native load path"));
    }
    Ok(path.into())
}

fn host_system(path: &Path) -> bool {
    !path.components().any(|part| part == Component::ParentDir)
        && (path.starts_with("/usr/lib") || path.starts_with("/System/Library"))
}

fn read_commands(path: &Path) -> io::Result<macho_load_commands::LoadCommands> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    const LIMIT: u64 = 128 * 1024 * 1024;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err(invalid("native image is not a bounded regular file"));
    }
    let mut bytes = Vec::new();
    (&file).take(LIMIT + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if bytes.len() as u64 != metadata.len()
        || after.len() != metadata.len()
        || after.mtime() != metadata.mtime()
        || after.mtime_nsec() != metadata.mtime_nsec()
        || after.ctime() != metadata.ctime()
        || after.ctime_nsec() != metadata.ctime_nsec()
        || current.dev() != after.dev()
        || current.ino() != after.ino()
    {
        return Err(invalid("native image changed during inventory"));
    }
    macho_load_commands::parse(&bytes)
}

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expansion_and_host_boundaries_are_explicit() {
        assert!(
            collect(Path::new("relative-host"), &[])
                .unwrap_err()
                .to_string()
                .contains("absolute")
        );
        assert_eq!(
            expand(
                "@loader_path/sub/lib.dylib",
                Path::new("/a"),
                Path::new("/b")
            )
            .unwrap(),
            Path::new("/a/sub/lib.dylib")
        );
        assert_eq!(
            expand(
                "@executable_path/lib.dylib",
                Path::new("/a"),
                Path::new("/b")
            )
            .unwrap(),
            Path::new("/b/lib.dylib")
        );
        assert!(expand("lib.dylib", Path::new("/a"), Path::new("/b")).is_err());
        assert!(host_system(Path::new("/usr/lib/libSystem.B.dylib")));
        assert!(!host_system(Path::new("/usr/library/lib.dylib")));
        assert!(!host_system(Path::new("/usr/lib/../../tmp/lib.dylib")));
    }
    #[test]
    #[ignore = "explicit real runtime inventory check; requires built images"]
    fn built_runtime_inventory_contains_transitive_native_dependencies() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let files = collect(
            &root.join("target/debug/darwin-art-host"),
            &[
                root.join(
                    "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib",
                ),
                root.join("_build/moltenvk/libMoltenVK.dylib"),
                root.join("_build/runtime-graphics-link-probe/libopenjdk-named-jni-owner.dylib"),
            ],
        )
        .unwrap();
        for name in [
            "libMoltenVK.dylib",
            "libopenjdk-named-jni-owner.dylib",
            "libEGL.dylib",
            "libGLESv2.dylib",
            "libperfetto_c.dylib",
        ] {
            assert!(
                files.iter().any(|path| path.file_name().unwrap() == name),
                "missing {name}"
            );
        }
        for prefix in ["liblz4.", "libzstd."] {
            assert!(
                files.iter().any(|path| path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(prefix)),
                "missing {prefix}"
            );
        }
        assert!(!files.iter().any(|path| host_system(path)));
    }
}
