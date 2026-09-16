use super::*;
use sha2::{Digest, Sha256};

pub(crate) fn build_foundation(root: &Path) -> Result<()> {
    let artbase = root.join("_aosp/art/libartbase");
    let libbase = root.join("_aosp/system/libbase");
    let libziparchive = root.join("_aosp/system/libziparchive");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    if !artbase.join("Android.bp").exists() || !libbase.join("Android.bp").exists() {
        return Err("foundation sources are missing; run `art-bootstrap sync` first".into());
    }

    let build_dir = root.join("_build/foundation");
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
        && [
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
        ]
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
        for source in [
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
        ] {
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
        for source in [
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
        ] {
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
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
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

    let probe = build_dir.join("foundation-probe");
    run_command(
        common_cpp_command(&includes)
            .arg(root.join("probes/foundation.cc"))
            .arg(&artbase_archive)
            .arg(&android_base_archive)
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
    for path in ["globals.h", "mem_map.cc", "mem_map_unix.cc"]
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

fn create_archive_if_needed(archive: &Path, objects: &[PathBuf], compiled: usize) -> Result<()> {
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
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if compiled > 0 || current != expected {
        create_archive(archive, objects)?;
    }
    Ok(())
}
