//! AOSP enum operator generation and atomic, content-stable publication.

use crate::{Result, support::command_output};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_PUBLICATION: AtomicU64 = AtomicU64::new(0);
struct Publication(PathBuf);
impl Drop for Publication {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(crate) fn generate_operator_source(
    root: &Path,
    local_path: &Path,
    headers: &[&str],
    destination: &Path,
) -> Result<()> {
    // Execute the pinned generator against current headers, even if a previous
    // output exists. Content-stable publication keeps native object caches warm.
    let mut generate = Command::new(crate::support::support_build_tool("PYTHON", "python3"));
    generate
        .arg(root.join("_aosp/art/tools/generate_operator_out.py"))
        .arg(local_path);
    for header in headers {
        generate.arg(local_path.join(header));
    }
    let output = command_output(&mut generate)?;
    publish(destination, output.as_bytes())
}

fn publish(destination: &Path, bytes: &[u8]) -> Result<()> {
    if fs::read(destination).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let parent = destination
        .parent()
        .ok_or("generated operator has no parent")?;
    fs::create_dir_all(parent)?;
    let filename = destination
        .file_name()
        .ok_or("generated operator has no filename")?;
    let (mut file, publication) = loop {
        let mut name = filename.to_os_string();
        name.push(format!(
            ".generated-{}-{}",
            std::process::id(),
            NEXT_PUBLICATION.fetch_add(1, Ordering::Relaxed)
        ));
        let path = parent.join(name);
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => break (file, Publication(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    file.write_all(bytes)?;
    drop(file);
    fs::rename(&publication.0, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_generator_refreshes_headers_and_preserves_unchanged_mtime() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let directory = PathBuf::from(
            command_output(Command::new("mktemp").arg("-d"))
                .unwrap()
                .trim(),
        );
        let header = directory.join("shade.h");
        let destination = directory.join("operator.cc");
        fs::write(&header, "enum class Shade {\n  kBlue,\n};\n").unwrap();
        generate_operator_source(root, &directory, &["shade.h"], &destination).unwrap();
        assert!(fs::read_to_string(&destination).unwrap().contains("kBlue"));
        let before = fs::metadata(&destination).unwrap().modified().unwrap();
        generate_operator_source(root, &directory, &["shade.h"], &destination).unwrap();
        assert_eq!(
            before,
            fs::metadata(&destination).unwrap().modified().unwrap()
        );
        fs::write(&header, "enum class Shade {\n  kRed,\n};\n").unwrap();
        generate_operator_source(root, &directory, &["shade.h"], &destination).unwrap();
        let changed = fs::read(&destination).unwrap();
        assert!(String::from_utf8_lossy(&changed).contains("kRed"));
        assert!(!String::from_utf8_lossy(&changed).contains("kBlue"));
        // A failed generation must leave the last complete output untouched.
        fs::remove_file(&header).unwrap();
        assert!(generate_operator_source(root, &directory, &["shade.h"], &destination).is_err());
        assert_eq!(changed, fs::read(&destination).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }
}
