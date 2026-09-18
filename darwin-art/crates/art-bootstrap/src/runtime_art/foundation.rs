use super::*;

pub(crate) fn build_runtime_platform(root: &Path) -> Result<()> {
    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let patched_artbase_base = patched_artbase.join("base");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let runtime = root.join("_aosp/art/runtime");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    let android_jni_include = root.join("_aosp/libnativehelper/include_jni");
    let dlmalloc = root.join("_aosp/external/dlmalloc");
    if !runtime.join("Android.bp").exists() || !tinyxml2.join("tinyxml2.h").exists() {
        return Err("runtime platform sources are missing; run `art-bootstrap sync` first".into());
    }

    let includes = [
        patched_artbase.as_path(),
        patched_artbase_base.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        runtime.as_path(),
        palette_include.as_path(),
        libbase_include.as_path(),
        tinyxml2.as_path(),
        android_jni_include.as_path(),
        dlmalloc.as_path(),
        Path::new("/opt/homebrew/include"),
    ];
    let build_dir = root.join("_build/runtime-platform");
    let object_dir = build_dir.join("objects");
    let patched_source_root = build_dir.join("patched-source");
    let patched_runtime = patched_source_root.join("runtime");
    fs::create_dir_all(&object_dir)?;
    fs::create_dir_all(&patched_runtime)?;

    // Keep the Darwin fatal-signal exit adaptation as a reviewable patch over
    // one pristine AOSP source.  The other platform files remain directly
    // sourced from _aosp and retain their upstream host behavior.
    let runtime_linux = runtime.join("runtime_linux.cc");
    let staged_runtime_linux = patched_runtime.join("runtime_linux.cc");
    fs::copy(&runtime_linux, &staged_runtime_linux)?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0195-darwin-fatal-signal-process-exit.patch"))
            .current_dir(&patched_source_root),
    )?;

    let sources = [
        staged_runtime_linux,
        runtime.join("thread_linux.cc"),
        runtime.join("monitor_linux.cc"),
    ];
    let mut objects = Vec::new();
    for source in sources {
        let file_name = source
            .file_name()
            .ok_or_else(|| format!("source has no file name: {}", source.display()))?;
        let object = object_dir.join(format!("{}.o", file_name.to_string_lossy()));
        run_command(
            runtime_cpp_command(&includes)
                .arg("-c")
                .arg(&source)
                .arg("-o")
                .arg(&object),
        )?;
        let kind = command_output(Command::new("file").arg(&object))?;
        if !kind.contains("Mach-O 64-bit object arm64") {
            return Err(format!("unexpected runtime object format: {kind}").into());
        }
        objects.push(object);
    }

    let archive = build_dir.join("libart-platform-darwin.a");
    create_archive(&archive, &objects)?;
    println!(
        "build-runtime-platform: Mach-O arm64 objects={} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(())
}

pub(crate) fn build_runtime_core(root: &Path) -> Result<()> {
    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let patched_artbase_base = patched_artbase.join("base");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let runtime = root.join("_aosp/art/runtime");
    let runtime_base = runtime.join("base");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    let android_jni_include = root.join("_aosp/libnativehelper/include_jni");
    let dlmalloc = root.join("_aosp/external/dlmalloc");

    let build_dir = root.join("_build/runtime-core");
    let patched_runtime = build_dir.join("patched-source/runtime");
    let patched_base = patched_runtime.join("base");
    let patched_mirror = patched_runtime.join("mirror");
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(&patched_base)?;
    fs::create_dir_all(&patched_mirror)?;
    fs::create_dir_all(&object_dir)?;
    fs::copy(
        runtime.join("monitor.cc"),
        patched_runtime.join("monitor.cc"),
    )?;
    fs::copy(runtime.join("base/mutex.h"), patched_base.join("mutex.h"))?;
    fs::copy(
        runtime.join("base/mutex-inl.h"),
        patched_base.join("mutex-inl.h"),
    )?;
    fs::copy(runtime.join("base/mutex.cc"), patched_base.join("mutex.cc"))?;
    fs::copy(
        runtime.join("mirror/object_reference.h"),
        patched_mirror.join("object_reference.h"),
    )?;
    fs::copy(runtime.join("obj_ptr.h"), patched_runtime.join("obj_ptr.h"))?;
    fs::copy(
        runtime.join("obj_ptr-inl.h"),
        patched_runtime.join("obj_ptr-inl.h"),
    )?;
    fs::copy(
        runtime.join("lock_word.h"),
        patched_runtime.join("lock_word.h"),
    )?;
    fs::copy(
        runtime.join("lock_word-inl.h"),
        patched_runtime.join("lock_word-inl.h"),
    )?;

    for patch in [
        "patches/art/0003-darwin-allow-pthread-monitors.patch",
        "patches/art/0004-darwin-uncontended-monitor-lock.patch",
        "patches/art/0004b-darwin-cross-thread-monitor-lock.patch",
        "patches/art/0026-darwin-base-relative-object-references-only.patch",
        "patches/art/0159-darwin-objptr-base-relative-boundary.patch",
        "patches/art/0033-darwin-base-relative-lockword-forwarding.patch",
        "patches/art/0042b-darwin-compressed32-monitor-boundary.patch",
        "patches/art/0078-darwin-pthread-empty-checkpoints.patch",
    ] {
        run_command(
            Command::new("patch")
                .args(["--batch", "--forward", "-p1", "-i"])
                .arg(root.join(patch))
                .current_dir(build_dir.join("patched-source")),
        )?;
    }

    // Keep the Darwin empty-checkpoint adaptation fail-closed.  A pthread
    // waiter must periodically service the ART checkpoint before retrying the
    // lock; silently dropping the callback would deadlock GC/checkpoint
    // coordination even though ordinary monitor tests still pass.
    let patched_mutex = fs::read_to_string(patched_base.join("mutex.cc"))?;
    let patched_mutex_inline = fs::read_to_string(patched_base.join("mutex-inl.h"))?;
    for source in [&patched_mutex, &patched_mutex_inline] {
        if !source.contains("CheckEmptyCheckpointFromMutex()") {
            return Err(
                "Darwin pthread checkpoint waiter lost CheckEmptyCheckpointFromMutex".into(),
            );
        }
        if !source.contains("tv_nsec = 100'000") {
            return Err("Darwin pthread checkpoint waiter lost bounded retry delay".into());
        }
    }
    if !patched_mutex.contains("Expected Darwin mutex waiters use an interruptible try-lock loop")
        || !patched_mutex.contains("Expected Darwin rwlock waiters poll the checkpoint flag")
    {
        return Err("Darwin empty-checkpoint wakeup adaptation is missing".into());
    }

    let includes = [
        patched_runtime.as_path(),
        patched_artbase.as_path(),
        patched_artbase_base.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        runtime.as_path(),
        runtime_base.as_path(),
        palette_include.as_path(),
        libbase_include.as_path(),
        tinyxml2.as_path(),
        android_jni_include.as_path(),
        dlmalloc.as_path(),
        Path::new("/opt/homebrew/include"),
    ];
    let sources = [
        patched_base.join("mutex.cc"),
        patched_runtime.join("monitor.cc"),
    ];
    let mut objects = Vec::new();
    for source in sources {
        let file_name = source
            .file_name()
            .ok_or_else(|| format!("source has no file name: {}", source.display()))?;
        let object = object_dir.join(format!("{}.o", file_name.to_string_lossy()));
        run_command(
            runtime_cpp_command(&includes)
                .args(["-include", "mirror/object_reference.h"])
                .arg("-c")
                .arg(&source)
                .arg("-o")
                .arg(&object),
        )?;
        let kind = command_output(Command::new("file").arg(&object))?;
        if !kind.contains("Mach-O 64-bit object arm64") {
            return Err(format!("unexpected runtime core object format: {kind}").into());
        }
        objects.push(object);
    }

    let archive = build_dir.join("libart-core-darwin.a");
    create_archive(&archive, &objects)?;
    println!(
        "build-runtime-core: pthread monitor bootstrap objects={} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(())
}

pub(crate) fn probe_park(root: &Path) -> Result<()> {
    let mutex_object = root.join("_build/runtime-core/objects/mutex.cc.o");
    let patched_runtime = root.join("_build/runtime-common/patched-source/runtime");
    if !mutex_object.exists() || !patched_runtime.join("thread.h").exists() {
        return Err(
            "Darwin runtime objects are missing; run `build-runtime-bootstrap` first".into(),
        );
    }

    let build_dir = root.join("_build/park-probe");
    fs::create_dir_all(&build_dir)?;
    let object = build_dir.join("park_probe.cc.o");
    let executable = build_dir.join("park-probe");
    let generated = root.join("_build/runtime-arm64/generated");
    let generator = root.join("_aosp/art/tools/cpp-define-generator");
    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let patched_artbase_base = patched_artbase.join("base");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let runtime = root.join("_aosp/art/runtime");
    let runtime_base = runtime.join("base");
    let runtime_arm64 = runtime.join("arch/arm64");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let jni_include = root.join("_aosp/libnativehelper/include_jni");
    let nativehelper_include = root.join("_aosp/libnativehelper/header_only_include");
    let dlmalloc = root.join("_aosp/external/dlmalloc");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    let includes = [
        patched_runtime.as_path(),
        generated.as_path(),
        generator.as_path(),
        patched_artbase.as_path(),
        patched_artbase_base.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        runtime.as_path(),
        runtime_base.as_path(),
        runtime_arm64.as_path(),
        libbase_include.as_path(),
        palette_include.as_path(),
        jni_include.as_path(),
        nativehelper_include.as_path(),
        dlmalloc.as_path(),
        tinyxml2.as_path(),
        Path::new("/opt/homebrew/include"),
    ];
    run_command(
        common_cpp_command(&includes)
            .arg("-Wno-invalid-offsetof")
            .arg("-c")
            .arg(root.join("probes/park_probe.cc"))
            .arg("-o")
            .arg(&object),
    )?;
    run_command(
        Command::new("clang++")
            .arg("-Wl,-dead_strip")
            .arg(&object)
            .arg(&mutex_object)
            .arg(root.join("_build/foundation/libartbase-darwin.a"))
            .arg(root.join("_build/foundation/libandroid-base-darwin.a"))
            .args(["-L/opt/homebrew/lib", "-lfmt"])
            .arg("-o")
            .arg(&executable),
    )?;
    let output = command_output(&mut Command::new(&executable))?;
    let expected = "ART Darwin park: pre-permit=yes wakeups=200 timeout=yes";
    if output.trim() != expected {
        return Err(format!("unexpected Darwin park probe output: {output:?}").into());
    }
    println!("probe-park: {}", output.trim());
    Ok(())
}
