use super::*;
use crate::native_build::{PendingNativeCompile, compile_pending_native};

fn source_list(block: &str) -> Result<Vec<String>> {
    let start = block.find("srcs: [").ok_or("missing dex2oat source list")? + "srcs: [".len();
    let end = block[start..]
        .find(']')
        .ok_or("unterminated dex2oat source list")?
        + start;
    let sources = block[start..end]
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix('"')
                .and_then(|value| value.split('"').next())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    if sources.is_empty()
        || sources
            .iter()
            .any(|source| !source.ends_with(".cc") || source.contains(".."))
    {
        return Err("unexpected pinned dex2oat source list".into());
    }
    Ok(sources)
}

/// Build AOSP's profile-guided app-image producer into a separate archive.
/// The final runtime dylib force-loads it and exports a renamed `main`; a
/// short-lived producer process invokes that entry before launching the app.
pub(crate) fn build_dex2oat(root: &Path) -> Result<PathBuf> {
    let source = root.join("_aosp/art/dex2oat");
    let bp = fs::read_to_string(source.join("Android.bp"))?;
    let defaults = bp
        .split("name: \"libart-dex2oat-defaults\"")
        .nth(1)
        .ok_or("missing pinned libart-dex2oat defaults")?;
    let mut sources = source_list(defaults)?;
    let arm64 = defaults
        .split("arm64: {")
        .nth(1)
        .ok_or("missing pinned dex2oat ARM64 sources")?;
    sources.extend(source_list(arm64)?);
    // ART's ARM64 linker inherits the shared ARM thunk manager even though
    // Soong expresses it through codegen defaults rather than the arm64 list.
    sources.push("linker/arm/relative_patcher_arm_base.cc".to_owned());
    sources.extend(["dex2oat_options.cc".to_owned(), "dex2oat.cc".to_owned()]);

    let build = root.join("_build/dex2oat-darwin");
    fs::create_dir_all(build.join("objects"))?;
    fs::create_dir_all(build.join("linker"))?;

    // Image files retain Android's 32-bit logical boot-image addresses while
    // Darwin maps the corresponding objects in the high compressed-reference
    // window. Keep upstream's range representation and normalize only the
    // pointer being tested at the image-writer boundary.
    let image_writer_header = fs::read_to_string(source.join("linker/image_writer.h"))?;
    let image_range_before = r#"  ALWAYS_INLINE bool IsInBootImage(const void* obj) const {
    return reinterpret_cast<uintptr_t>(obj) - boot_image_begin_ < boot_image_size_;
  }"#;
    let image_range_after = r#"  ALWAYS_INLINE bool IsInBootImage(const void* obj) const {
    return ArtHostAddressToCompressedReferenceAddress(reinterpret_cast<uintptr_t>(obj)) -
        boot_image_begin_ < boot_image_size_;
  }"#;
    if image_writer_header.matches(image_range_before).count() != 1 {
        return Err("pinned dex2oat boot-image range contract changed".into());
    }
    fs::write(
        build.join("linker/image_writer.h"),
        image_writer_header.replacen(image_range_before, image_range_after, 1),
    )?;
    fs::copy(
        source.join("linker/image_writer.cc"),
        build.join("linker/image_writer.cc"),
    )?;
    let mut generator = Command::new("python3");
    generator
        .arg(root.join("_aosp/art/tools/generate_operator_out.py"))
        .arg(&source)
        .arg(source.join("linker/image_writer.h"));
    let generated = generator.output()?;
    if !generated.status.success() {
        return Err(String::from_utf8_lossy(&generated.stderr)
            .into_owned()
            .into());
    }
    let operator_source = build.join("operator_out.cc");
    fs::write(&operator_source, generated.stdout)?;

    // AOSP's host dex2oat configures a monotonic condition variable on Linux,
    // but its Apple branch skips pthread_cond_init() altogether. Android's
    // zero-filled pthread object happens to tolerate that; Darwin's opaque
    // pthread_cond_t does not and pthread_cond_timedwait() returns EINVAL.
    // Keep the pinned source immutable and stage the narrow host-OS fix in the
    // generated build directory.
    let dex2oat_source = fs::read_to_string(source.join("dex2oat.cc"))?;
    let condvar_before = r#"#ifndef __APPLE__
    pthread_condattr_t condattr;
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_condattr_init, (&condattr), reason);
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_condattr_setclock, (&condattr, CLOCK_MONOTONIC), reason);
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_cond_init, (&cond_, &condattr), reason);
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_condattr_destroy, (&condattr), reason);
#endif"#;
    let condvar_after = r#"#if defined(__APPLE__)
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_cond_init, (&cond_, nullptr), reason);
#else
    pthread_condattr_t condattr;
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_condattr_init, (&condattr), reason);
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_condattr_setclock, (&condattr, CLOCK_MONOTONIC), reason);
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_cond_init, (&cond_, &condattr), reason);
    CHECK_WATCH_DOG_PTHREAD_CALL(pthread_condattr_destroy, (&condattr), reason);
#endif"#;
    if dex2oat_source.matches(condvar_before).count() != 1 {
        return Err("pinned dex2oat Apple watchdog contract changed".into());
    }
    let staged_dex2oat_source = build.join("dex2oat.cc");
    fs::write(
        &staged_dex2oat_source,
        dex2oat_source.replacen(condvar_before, condvar_after, 1),
    )?;

    let include_paths = [
        "_build/dex2oat-darwin",
        "_build/runtime-common/patched-source/runtime",
        "_build/foundation/patched-source/libartbase",
        "_build/runtime-arm64/generated",
        "compat",
        "_aosp/art/dex2oat",
        "_aosp/art/compiler",
        "_aosp/art/runtime",
        "_aosp/art/runtime/base",
        "_aosp/art/runtime/oat",
        "_aosp/art/libartbase",
        "_aosp/art/libdexfile",
        "_aosp/art/libprofile",
        "_aosp/art/libelffile",
        "_aosp/art/cmdline",
        "_aosp/art/profman",
        "_aosp/art/profman/include",
        "_aosp/art/libartpalette/include",
        "_aosp/system/libbase/include",
        "_aosp/system/logging/liblog/include",
        "_aosp/libnativehelper/include_jni",
        "_aosp/libnativehelper/header_only_include",
        "_aosp/external/fmtlib/include",
        "_aosp/external/tinyxml2",
        "_aosp/external/dlmalloc",
        "_aosp/external/lz4/lib",
        "_aosp/boringssl-full/src/include",
        "/opt/homebrew/include",
    ]
    .map(|path| {
        if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            root.join(path)
        }
    });
    let includes = include_paths
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut source_paths = sources
        .into_iter()
        .map(|path| {
            if path == "dex2oat.cc" {
                staged_dex2oat_source.clone()
            } else if path == "linker/image_writer.cc" {
                build.join("linker/image_writer.cc")
            } else {
                source.join(path)
            }
        })
        .collect::<Vec<_>>();
    source_paths.push(operator_source);
    source_paths.push(root.join("runtime/art/tools/dex2oat_entry.cc"));
    source_paths.extend(
        [
            "boot_image_profile.cc",
            "profman.cc",
            "profile_assistant.cc",
            "inline_cache_format_util.cc",
        ]
        .map(|path| root.join("_aosp/art/profman").join(path)),
    );
    source_paths.push(root.join("runtime/art/tools/profman_entry.cc"));
    source_paths.sort();

    let mut jobs = Vec::new();
    for source_path in source_paths {
        let relative = source_path.strip_prefix(root)?;
        let object = build.join("objects").join(format!(
            "{}.o",
            relative.to_string_lossy().replace('/', "_")
        ));
        let mut command = runtime_cpp_command(&includes);
        command.args([
            "-std=gnu++20",
            "-DART_ENABLE_CODEGEN_arm64",
            "-Wno-unused-command-line-argument",
            "-include",
            "mirror/object_reference.h",
        ]);
        if source_path == staged_dex2oat_source {
            command.arg("-Dmain=darwin_art_dex2oat_main");
        } else if source_path == root.join("_aosp/art/profman/profman.cc") {
            command.arg("-Dmain=darwin_art_profman_main");
        }
        command
            .arg("-idirafter")
            .arg(&ndk_include)
            .arg("-idirafter")
            .arg(&ndk_arch_include)
            .arg("-c")
            .arg(&source_path)
            .arg("-o")
            .arg(&object);
        jobs.push(PendingNativeCompile { command, object });
    }
    let (objects, compiled, cached) = compile_pending_native(jobs, &identity)?;
    let archive = build.join("libart-dex2oat-darwin.a");
    create_archive(&archive, &objects)?;
    println!(
        "build-dex2oat: AOSP ARM64 objects={} compiled={compiled} cached={cached} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(archive)
}
