use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use crate::Result;
pub(crate) use crate::native_cache::{
    FileHashCache, compile_with_dependency_cache, link_with_cache,
};
use crate::support::{command_output, run_command};

pub(crate) fn build_elf_loader(root: &Path) -> Result<PathBuf> {
    run_command(
        Command::new("cargo")
            .args(["build", "-q", "--release", "-p", "darwin-art-elf-loader"])
            .current_dir(root),
    )?;
    let archive = root.join("target/release/libdarwin_art_elf_loader.a");
    if !archive.is_file() {
        return Err(format!("Rust ELF loader archive is missing: {}", archive.display()).into());
    }
    Ok(archive)
}

pub(crate) struct PendingNativeCompile {
    pub(crate) command: Command,
    pub(crate) object: PathBuf,
}

pub(crate) fn common_cpp_command(includes: &[&Path]) -> Command {
    let mut command = Command::new(crate::support::support_build_tool("CLANG", "clang++"));
    if let Some(sdk) = std::env::var_os("DARWIN_ART_SUPPORT_SDK") {
        command.arg("-isysroot").arg(sdk);
    }
    command.args([
        "-std=c++20",
        "-O2",
        "-DNDEBUG",
        "-DART_PAGE_SIZE_AGNOSTIC",
        "-ftrivial-auto-var-init=zero",
        "-ffunction-sections",
        "-fdata-sections",
    ]);
    for include in includes {
        command.arg(format!("-I{}", include.display()));
    }
    command.args(["-include", "base/globals.h"]);
    command
}

pub(crate) fn compile_pending_native(
    jobs: Vec<PendingNativeCompile>,
    compiler_identity: &str,
) -> Result<(Vec<PathBuf>, usize, usize)> {
    if jobs.is_empty() {
        return Ok((Vec::new(), 0, 0));
    }
    let default_jobs = thread::available_parallelism()
        .map(|count| count.get().min(8))
        .unwrap_or(4)
        .max(1);
    let workers = std::env::var("DARWIN_ART_NATIVE_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_jobs)
        .min(8);
    let mut pending = jobs;
    let mut objects = Vec::new();
    let mut compiled = 0;
    let mut cached = 0;
    while !pending.is_empty() {
        let batch_len = pending.len().min(workers);
        let batch: Vec<_> = pending.drain(..batch_len).collect();
        let results = thread::scope(|scope| {
            let mut handles = Vec::with_capacity(batch.len());
            for job in batch {
                let identity = compiler_identity.to_owned();
                handles.push(scope.spawn(move || {
                    let cache_path = job.object.with_extension("hashes.cache");
                    let mut cache =
                        FileHashCache::load(&cache_path).map_err(|error| error.to_string())?;
                    let mut command = job.command;
                    let did_compile = compile_with_dependency_cache(
                        &mut command,
                        &job.object,
                        &identity,
                        &mut cache,
                    )
                    .map_err(|error| error.to_string())?;
                    cache.save(&cache_path).map_err(|error| error.to_string())?;
                    Ok::<_, String>((job.object, did_compile))
                }));
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "native compile worker panicked".to_owned())?
                })
                .collect::<std::result::Result<Vec<_>, String>>()
        })
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
        for (object, did_compile) in results {
            record_cache_result(did_compile, &mut compiled, &mut cached);
            objects.push(object);
        }
    }
    Ok((objects, compiled, cached))
}

pub(crate) fn compile_cpp(source: &Path, object_dir: &Path, includes: &[&Path]) -> Result<PathBuf> {
    let file_name = source
        .file_name()
        .ok_or_else(|| format!("source has no file name: {}", source.display()))?;
    let object = object_dir.join(format!("{}.o", file_name.to_string_lossy()));
    run_command(
        common_cpp_command(includes)
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(&object),
    )?;
    Ok(object)
}

pub(crate) fn record_cache_result(
    compiled: bool,
    compiled_objects: &mut usize,
    cached_objects: &mut usize,
) {
    if compiled {
        *compiled_objects += 1;
    } else {
        *cached_objects += 1;
    }
}

pub(crate) fn create_archive(archive: &Path, objects: &[PathBuf]) -> Result<()> {
    if archive.exists() {
        fs::remove_file(archive)?;
    }
    let mut command = Command::new("ar");
    command.arg("rcs").arg(archive);
    for object in objects {
        command.arg(object);
    }
    run_command(&mut command)
}

/// Cached objects may be unchanged even when a source was removed. Archive
/// membership must match the current producer list, not historical members.
pub(crate) fn create_archive_if_needed(
    archive: &Path,
    objects: &[PathBuf],
    compiled: usize,
) -> Result<()> {
    let expected = objects
        .iter()
        .map(|object| {
            object
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| format!("object has no file name: {}", object.display()))
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let current = if archive.is_file() {
        command_output(Command::new("ar").arg("-t").arg(archive))?
            .lines()
            // Darwin ar lists its linker symbol index as a member. It is
            // metadata, not a producer object; all actual members stay exact.
            .filter(|member| {
                !matches!(
                    *member,
                    "__.SYMDEF" | "__.SYMDEF SORTED" | "__.SYMDEF_64" | "__.SYMDEF_64 SORTED"
                )
            })
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if !archive.is_file() || compiled > 0 || current != expected {
        create_archive(archive, objects)?;
    }
    Ok(())
}

#[cfg(test)]
mod archive_tests {
    use super::*;

    #[test]
    fn archive_membership_removes_cached_obsolete_objects() {
        let directory = PathBuf::from(
            command_output(Command::new("mktemp").arg("-d"))
                .unwrap()
                .trim(),
        );
        let mut objects = Vec::new();
        for name in ["one", "two"] {
            let source = directory.join(format!("{name}.c"));
            let object = directory.join(format!("{name}.o"));
            fs::write(&source, format!("int {name}(void) {{ return 1; }}\n")).unwrap();
            run_command(
                Command::new("clang")
                    .arg("-c")
                    .arg(&source)
                    .arg("-o")
                    .arg(&object),
            )
            .unwrap();
            objects.push(object);
        }
        let archive = directory.join("fixture.a");
        create_archive_if_needed(&archive, &objects, 2).unwrap();
        let before = fs::metadata(&archive).unwrap().modified().unwrap();
        create_archive_if_needed(&archive, &objects, 0).unwrap();
        assert_eq!(before, fs::metadata(&archive).unwrap().modified().unwrap());
        // No source recompilation, but the removed source must leave no member.
        objects.pop();
        create_archive_if_needed(&archive, &objects, 0).unwrap();
        let members = command_output(Command::new("ar").arg("-t").arg(&archive)).unwrap();
        assert!(members.lines().any(|member| member == "one.o"));
        assert!(!members.lines().any(|member| member == "two.o"));
        fs::remove_dir_all(directory).unwrap();
    }
}
