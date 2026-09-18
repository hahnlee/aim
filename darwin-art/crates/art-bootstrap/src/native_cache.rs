//! Persistent dependency and link fingerprints for native artifacts.
//!
//! This module deliberately knows nothing about ART source selection or
//! command dispatch. It is the reusable cache boundary shared by bootstrap,
//! probes, and the Ninja graph.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::Result;
use crate::support::{describe_command, run_command};

pub(crate) fn compile_with_dependency_cache(
    command: &mut Command,
    object: &Path,
    compiler_identity: &str,
    file_hash_cache: &mut FileHashCache,
) -> Result<bool> {
    let depfile = object.with_extension("o.d");
    let fingerprint = object.with_extension("o.fingerprint");
    command.arg("-MMD").arg("-MF").arg(&depfile);
    let command_description = describe_command(command);

    if object.is_file()
        && depfile.is_file()
        && fingerprint.is_file()
        && let Some(current) = dependency_fingerprint(
            &depfile,
            &command_description,
            compiler_identity,
            file_hash_cache,
        )?
        && fs::read_to_string(&fingerprint).is_ok_and(|cached| cached == current)
    {
        return Ok(false);
    }

    run_command(command)?;
    let current = dependency_fingerprint(
        &depfile,
        &command_description,
        compiler_identity,
        file_hash_cache,
    )?
    .ok_or_else(|| {
        format!(
            "compiler did not produce a usable dependency file for {}",
            object.display()
        )
    })?;
    atomic_write(&fingerprint, current.as_bytes())?;
    Ok(true)
}

fn dependency_fingerprint(
    depfile: &Path,
    command_description: &str,
    compiler_identity: &str,
    file_hash_cache: &mut FileHashCache,
) -> Result<Option<String>> {
    let Ok(depfile_text) = fs::read_to_string(depfile) else {
        return Ok(None);
    };
    let normalized = depfile_text.replace("\\\r\n", " ").replace("\\\n", " ");
    let Some((_, dependency_text)) = normalized.split_once(':') else {
        return Ok(None);
    };
    let mut dependencies = parse_makefile_words(dependency_text)
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    dependencies.sort();
    dependencies.dedup();
    if dependencies.is_empty() || dependencies.iter().any(|path| !path.is_file()) {
        return Ok(None);
    }

    let mut dependency_hashes = String::new();
    for dependency in &dependencies {
        dependency_hashes.push_str(file_hash_cache.hash(dependency)?);
        dependency_hashes.push_str("  ");
        dependency_hashes.push_str(&dependency.to_string_lossy());
        dependency_hashes.push('\n');
    }
    Ok(Some(format!(
        "cache-format=1\ncompiler={compiler_identity}\n{}command={command_description}\n{dependency_hashes}",
        std::env::var("DARWIN_ART_SUPPORT_TOOLCHAIN_IDENTITY")
            .map(|identity| format!("support-toolchain={identity}\n"))
            .unwrap_or_default()
    )))
}

#[derive(Default)]
pub(crate) struct FileHashCache {
    entries: BTreeMap<String, FileHashEntry>,
}

struct FileHashEntry {
    metadata: String,
    sha256: String,
}

impl FileHashCache {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let Ok(contents) = fs::read_to_string(path) else {
            return Ok(Self::default());
        };
        let mut entries = BTreeMap::new();
        for line in contents.lines() {
            let mut fields = line.splitn(3, '\t');
            let (Some(path), Some(metadata), Some(sha256)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            entries.insert(
                path.to_owned(),
                FileHashEntry {
                    metadata: metadata.to_owned(),
                    sha256: sha256.to_owned(),
                },
            );
        }
        Ok(Self { entries })
    }

    fn hash(&mut self, path: &Path) -> Result<&str> {
        let path_key = path.to_string_lossy().into_owned();
        let metadata = fs::metadata(path)?;
        let metadata_key = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.size(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        );
        let cache_hit = self
            .entries
            .get(&path_key)
            .is_some_and(|entry| entry.metadata == metadata_key);
        if !cache_hit {
            let mut file = fs::File::open(path)?;
            let mut digest = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            let sha256 = format!("{:x}", digest.finalize());
            self.entries.insert(
                path_key.clone(),
                FileHashEntry {
                    metadata: metadata_key,
                    sha256,
                },
            );
        }
        Ok(&self.entries[&path_key].sha256)
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let mut contents = String::new();
        for (path, entry) in &self.entries {
            contents.push_str(path);
            contents.push('\t');
            contents.push_str(&entry.metadata);
            contents.push('\t');
            contents.push_str(&entry.sha256);
            contents.push('\n');
        }
        atomic_write(path, contents.as_bytes())?;
        Ok(())
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("darwin-art-cache-tmp-{}", std::process::id()));
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)
}

pub(crate) fn parse_makefile_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            word.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(character);
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Link an artifact only when the command or one of its file inputs changed.
pub(crate) fn link_with_cache(
    command: &mut Command,
    output: &Path,
    stamp: &Path,
) -> Result<Output> {
    let fingerprint = link_fingerprint(command, output);
    let cached =
        output.is_file() && stamp.is_file() && fs::read_to_string(stamp)?.trim() == fingerprint;
    if cached {
        return Ok(Command::new("true").output()?);
    }

    let result = command.output()?;
    if result.status.success() {
        if !output.is_file() {
            return Err(format!(
                "linker reported success without output: {}",
                output.display()
            )
            .into());
        }
        if let Some(parent) = stamp.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(stamp, format!("{fingerprint}\n").as_bytes())?;
    }
    Ok(result)
}

fn link_fingerprint(command: &Command, output: &Path) -> String {
    let mut digest = Sha256::new();
    if let Ok(identity) = std::env::var("DARWIN_ART_SUPPORT_TOOLCHAIN_IDENTITY") {
        digest.update(identity.as_bytes());
        digest.update([0]);
    }
    digest.update(command.get_program().to_string_lossy().as_bytes());
    digest.update([0]);
    let output = output.to_string_lossy();
    for argument in command.get_args() {
        let value = argument.to_string_lossy();
        if value == output {
            digest.update(b"<output>");
            digest.update([0]);
            continue;
        }
        digest.update(value.as_bytes());
        digest.update([0]);
        if let Ok(metadata) = fs::metadata(argument) {
            digest.update(metadata.len().to_le_bytes());
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                digest.update(duration.as_secs().to_le_bytes());
                digest.update(duration.subsec_nanos().to_le_bytes());
            }
        } else {
            digest.update([0xff]);
        }
    }
    format!("{:x}", digest.finalize())
}
