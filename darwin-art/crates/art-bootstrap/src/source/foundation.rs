use super::*;
use sha2::{Digest, Sha256};

const FOUNDATION_SHADOW_SOURCES: &[&str] = &[
    "globals.h",
    "bit_utils.h",
    "macros.h",
    "stl_util_identity.h",
    "mem_map.h",
    "mem_map.cc",
    "mem_map_unix.cc",
    "os_linux.cc",
    "scoped_flock.cc",
    "time_utils.cc",
    "utils.cc",
];

pub(crate) fn build_foundation(root: &Path) -> Result<()> {
    let archives = foundation_archives(root, &root.join("_build/foundation"))?;
    let includes = archives
        .includes
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let probe = root.join("_build/foundation/foundation-probe");
    run_command(
        common_cpp_command(&includes)
            .arg(root.join("probes/foundation.cc"))
            .arg(&archives.artbase)
            .arg(&archives.android_base)
            .arg("-o")
            .arg(&probe),
    )?;
    let output = command_output(&mut Command::new(&probe))?;
    if output.trim() != "libartbase Darwin: 1.500ms" {
        return Err(format!("unexpected foundation probe output: {output:?}").into());
    }
    println!("build-foundation: {}", output.trim());
    Ok(())
}

/// Build-tool clients own a distinct object/shadow/archive namespace. The
/// immutable upstream inputs are shared, never the mutable build outputs.
pub(crate) fn build_foundation_archives_at(root: &Path, output: &Path) -> Result<()> {
    foundation_archives(root, output).map(|_| ())
}

struct FoundationArchives {
    includes: Vec<PathBuf>,
    artbase: PathBuf,
    android_base: PathBuf,
}

fn foundation_archives(root: &Path, build_dir: &Path) -> Result<FoundationArchives> {
    let artbase = root.join("_aosp/art/libartbase");
    let libbase = root.join("_aosp/system/libbase");
    let libziparchive = root.join("_aosp/system/libziparchive");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    if !artbase.join("Android.bp").exists() || !libbase.join("Android.bp").exists() {
        return Err("foundation sources are missing; run `art-bootstrap sync` first".into());
    }

    let patched_source_dir = build_dir.join("patched-source");
    let patched_artbase = patched_source_dir.join("libartbase");
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(patched_artbase.join("base"))?;
    fs::create_dir_all(&object_dir)?;
    let foundation_patches = [
        "patches/art/0002-darwin-dynamic-page-size.patch",
        "patches/art/0020-darwin-low4g-mach-reservation.patch",
        "patches/art/0021-darwin-compressed-reference-window.patch",
        "patches/art/0094-darwin-thread-cpu-nanotime.patch",
        "patches/art/0102-darwin-logical-pthread-names.patch",
        "patches/art/0112-darwin-artbase-private-paths.patch",
        "patches/art/0039-darwin-memmap-exact-anonymous.patch",
    ];
    let shadow_identity = foundation_shadow_identity(root, &artbase, &foundation_patches)?;
    let shadow_identity_path = patched_source_dir.join(".darwin-art-shadow-identity");
    let shadow_current = fs::read_to_string(&shadow_identity_path)
        .is_ok_and(|cached| cached.trim() == shadow_identity)
        && FOUNDATION_SHADOW_SOURCES
            .iter()
            .all(|source| patched_artbase.join("base").join(source).is_file());
    if !shadow_current {
        let candidate_dir =
            build_dir.join(format!("patched-source.candidate-{}", std::process::id()));
        let candidate_artbase = candidate_dir.join("libartbase/base");
        if candidate_dir.exists() {
            fs::remove_dir_all(&candidate_dir)?;
        }
        fs::create_dir_all(&candidate_artbase)?;
        for source in FOUNDATION_SHADOW_SOURCES {
            fs::copy(
                artbase.join("base").join(source),
                candidate_artbase.join(source),
            )?;
        }
        for patch in foundation_patches {
            run_command(
                Command::new("patch")
                    .args(["--batch", "--forward", "-p1", "-i"])
                    .arg(root.join(patch))
                    .current_dir(&candidate_dir),
            )?;
        }
        for source in FOUNDATION_SHADOW_SOURCES {
            publish_if_changed(
                &candidate_artbase.join(source),
                &patched_artbase.join("base").join(source),
            )?;
        }
        fs::remove_dir_all(candidate_dir)?;
        for stale in ["utils.h"] {
            let overlay = patched_artbase.join("base").join(stale);
            if overlay.exists() {
                fs::remove_file(overlay)?;
            }
        }
        let temporary = shadow_identity_path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, format!("{shadow_identity}\n"))?;
        fs::rename(temporary, shadow_identity_path)?;
    }
    let libbase_include = libbase.join("include");
    let artbase_base = artbase.join("base");
    let libziparchive_include = libziparchive.join("include");
    let libziparchive_incfs_include = libziparchive.join("incfs_support/include");
    let compat = root.join("compat");
    let includes = [
        compat.as_path(),
        patched_artbase.as_path(),
        artbase.as_path(),
        artbase_base.as_path(),
        libbase_include.as_path(),
        libziparchive_include.as_path(),
        libziparchive_incfs_include.as_path(),
        tinyxml2.as_path(),
        Path::new("/opt/homebrew/include"),
    ];

    let android_base_sources = [
        root.join("compat/android_base_logging.cc"),
        libbase.join("file.cpp"),
        libbase.join("mapped_file.cpp"),
        libbase.join("parsebool.cpp"),
        libbase.join("properties.cpp"),
        libbase.join("stringprintf.cpp"),
        libbase.join("strings.cpp"),
    ];
    let compiler_identity = command_output(
        Command::new(crate::support::support_build_tool("CLANG", "clang++")).arg("--version"),
    )?;
    let android_base_jobs = android_base_sources
        .into_iter()
        .map(|source| pending_compile(common_cpp_command(&includes), source, &object_dir))
        .collect::<Result<Vec<_>>>()?;
    let (android_base_objects, android_base_compiled, _) =
        compile_pending_native(android_base_jobs, &compiler_identity)?;
    let android_base_archive = build_dir.join("libandroid-base-darwin.a");
    create_archive_if_needed(
        &android_base_archive,
        &android_base_objects,
        android_base_compiled,
    )?;

    let zip_object_dir = build_dir.join("zip-objects");
    fs::create_dir_all(&zip_object_dir)?;
    let zip_sources = [
        "zip_archive.cc",
        "zip_archive_stream_entry.cc",
        "zip_cd_entry_map.cc",
        "zip_error.cpp",
    ];
    let mut zip_jobs = Vec::new();
    for source in zip_sources {
        let mut source_path = libziparchive.join(source);
        if source == "zip_archive.cc" {
            let original = fs::read_to_string(&source_path)?;
            let marker = "::android::base::utf8::open(";
            if original.matches(marker).count() != 1 {
                return Err("unexpected AOSP ZipArchive open boundary".into());
            }
            let adapted = original.replace(marker, "::darwin_art_archive_open(");
            source_path = patched_source_dir.join("zip_archive.cc");
            if fs::read_to_string(&source_path).ok().as_deref() != Some(adapted.as_str()) {
                fs::write(&source_path, adapted)?;
            }
        }
        let object = zip_object_dir.join(format!("{source}.o"));
        let mut command = common_cpp_command(&includes);
        command
            .arg("-DZLIB_CONST")
            .arg("-I")
            .arg(&libziparchive)
            .arg("-include")
            .arg(root.join("compat/filesystem/archive_open.h"))
            .arg("-D_FILE_OFFSET_BITS=64")
            .arg("-DINCFS_SUPPORT_DISABLED=1")
            .arg("-c")
            .arg(&source_path)
            .arg("-o")
            .arg(&object);
        zip_jobs.push(PendingNativeCompile { command, object });
    }
    let object = zip_object_dir.join("archive_open.o");
    let mut command = common_cpp_command(&includes);
    command
        .arg("-c")
        .arg(root.join("compat/filesystem/archive_open.cc"))
        .arg("-o")
        .arg(&object);
    zip_jobs.push(PendingNativeCompile { command, object });
    let (zip_objects, zip_compiled, _) = compile_pending_native(zip_jobs, &compiler_identity)?;
    let zip_archive = build_dir.join("libziparchive-darwin.a");
    create_archive_if_needed(&zip_archive, &zip_objects, zip_compiled)?;

    let artbase_operator_source = build_dir.join("generated/artbase_operator_out.cc");
    generate_operator_source(
        root,
        &artbase,
        &[
            "arch/instruction_set.h",
            "base/allocator.h",
            "base/unix_file/fd_file.h",
        ],
        &artbase_operator_source,
    )?;

    let artbase_sources = [
        artbase_operator_source,
        artbase.join("arch/instruction_set.cc"),
        artbase.join("base/allocator.cc"),
        artbase.join("base/arena_allocator.cc"),
        artbase.join("base/arena_bit_vector.cc"),
        artbase.join("base/bit_vector.cc"),
        artbase.join("base/compiler_filter.cc"),
        artbase.join("base/file_magic.cc"),
        artbase.join("base/file_utils.cc"),
        artbase.join("base/flags.cc"),
        artbase.join("base/hex_dump.cc"),
        artbase.join("base/logging.cc"),
        artbase.join("base/malloc_arena_pool.cc"),
        artbase.join("base/membarrier.cc"),
        artbase.join("base/memfd.cc"),
        artbase.join("base/memory_region.cc"),
        patched_artbase.join("base/mem_map.cc"),
        artbase.join("base/metrics/metrics_common.cc"),
        patched_artbase.join("base/os_linux.cc"),
        artbase.join("base/pointer_size.cc"),
        artbase.join("base/runtime_debug.cc"),
        artbase.join("base/scoped_arena_allocator.cc"),
        patched_artbase.join("base/scoped_flock.cc"),
        artbase.join("base/socket_peer_is_trusted.cc"),
        patched_artbase.join("base/time_utils.cc"),
        artbase.join("base/unix_file/fd_file.cc"),
        artbase.join("base/unix_file/random_access_file_utils.cc"),
        patched_artbase.join("base/utils.cc"),
        artbase.join("base/zip_archive.cc"),
        artbase.join("base/globals_unix.cc"),
        patched_artbase.join("base/mem_map_unix.cc"),
        tinyxml2.join("tinyxml2.cpp"),
    ];
    let artbase_jobs = artbase_sources
        .into_iter()
        .map(|source| {
            let mut command = runtime_cpp_command(&includes);
            // libartbase is embedded into Darwin's production libart image,
            // not loaded as Android's separate libartbase(.d)ylib.  Keep the
            // AOSP globals check disabled for this one object, otherwise its
            // static initializer aborts every host before the embedded
            // provider can be used by an unchanged native ART test.
            if source
                .file_name()
                .is_some_and(|name| name == "globals_unix.cc")
            {
                command.arg("-DART_STATIC_LIBARTBASE");
            }
            pending_compile(command, source, &object_dir)
        })
        .collect::<Result<Vec<_>>>()?;
    let (artbase_objects, artbase_compiled, _) =
        compile_pending_native(artbase_jobs, &compiler_identity)?;
    let artbase_archive = build_dir.join("libartbase-darwin.a");
    create_archive_if_needed(&artbase_archive, &artbase_objects, artbase_compiled)?;
    fs::write(
        build_dir.join("current-depfiles.txt"),
        android_base_objects
            .iter()
            .chain(&zip_objects)
            .chain(&artbase_objects)
            .map(|object| format!("{}\n", object.with_extension("o.d").display()))
            .collect::<String>(),
    )?;

    Ok(FoundationArchives {
        includes: includes.iter().map(|path| path.to_path_buf()).collect(),
        artbase: artbase_archive,
        android_base: android_base_archive,
    })
}

fn pending_compile(
    mut command: Command,
    source: PathBuf,
    object_dir: &Path,
) -> Result<PendingNativeCompile> {
    let file_name = source
        .file_name()
        .ok_or_else(|| format!("source has no file name: {}", source.display()))?;
    let object = object_dir.join(format!("{}.o", file_name.to_string_lossy()));
    command.arg("-c").arg(source).arg("-o").arg(&object);
    Ok(PendingNativeCompile { command, object })
}

fn foundation_shadow_identity(root: &Path, artbase: &Path, patches: &[&str]) -> Result<String> {
    let mut digest = Sha256::new();
    for path in FOUNDATION_SHADOW_SOURCES
        .iter()
        .map(|source| artbase.join("base").join(source))
        .chain(patches.iter().map(|patch| root.join(patch)))
    {
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn publish_if_changed(candidate: &Path, destination: &Path) -> Result<()> {
    let bytes = fs::read(candidate)?;
    if destination.is_file() && fs::read(destination)? == bytes {
        return Ok(());
    }
    let temporary =
        destination.with_extension(format!("darwin-art-copy-tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_identity_covers_every_staged_source() {
        let directory = PathBuf::from(
            command_output(Command::new("mktemp").arg("-d"))
                .unwrap()
                .trim(),
        );
        let artbase = directory.join("libartbase");
        fs::create_dir_all(artbase.join("base")).unwrap();
        for source in FOUNDATION_SHADOW_SOURCES {
            fs::write(artbase.join("base").join(source), b"original").unwrap();
        }
        let before = foundation_shadow_identity(&directory, &artbase, &[]).unwrap();
        for source in FOUNDATION_SHADOW_SOURCES {
            let path = artbase.join("base").join(source);
            fs::write(&path, b"changed").unwrap();
            assert_ne!(
                before,
                foundation_shadow_identity(&directory, &artbase, &[]).unwrap(),
                "{source}"
            );
            fs::write(path, b"original").unwrap();
        }
        assert_eq!(
            before,
            foundation_shadow_identity(&directory, &artbase, &[]).unwrap()
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
