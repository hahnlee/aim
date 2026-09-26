use super::*;

fn collect_provider_gate_inputs(directory: &Path, inputs: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_provider_gate_inputs(&path, inputs)?;
        } else if path.is_file() {
            inputs.push(path);
        }
    }
    Ok(())
}

fn cached_bionic_provider_gate(root: &Path) -> Result<()> {
    let outputs = [
        "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a",
        "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-native-providers.a",
        "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-float-conversion.a",
        "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-binary128-conversion.a",
    ]
    .map(|path| root.join(path));
    let mut inputs = vec![root.join("upstream/android35-libcxx-provider-coverage.lock")];
    for entry in fs::read_dir(root.join("tools"))? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir()
            && (name.starts_with("bionic-")
                || matches!(
                    name,
                    "android-bionic-pthread-provider"
                        | "android-dl-iterate-phdr-provider"
                        | "android-liblog-exec-provider"
                ))
        {
            collect_provider_gate_inputs(&path, &mut inputs)?;
        }
    }
    build_shell_gate_cached(
        root,
        "build-bionic-runtime-provider-closure.sh",
        &[],
        &inputs,
        &outputs,
    )
}

fn cached_bionic_linker_config_gate(root: &Path) -> Result<()> {
    let outputs = [
        root.join("_build/bionic-linker-config/libbionic-linker-config.a"),
        root.join("_build/bionic-linker-config/linker_dlwarning.o"),
    ];
    let inputs = [
        root.join("tools/build-android16-bionic-linker-config.sh"),
        root.join("upstream/android16-bionic-linker-config.sources"),
        root.join("patches/native-loader-policy/bionic-config-filesystem.patch"),
        root.join("compat/filesystem/guest_config.cc"),
        root.join("compat/filesystem/linker_config_fs.cc"),
        root.join("compat/loader/warning_basename.h"),
        root.join("tools/native-loader-policy/bionic_config_types.h"),
    ];
    build_shell_gate_cached(
        root,
        "build-android16-bionic-linker-config.sh",
        &[],
        &inputs,
        &outputs,
    )
}

fn cached_foundation_gate(root: &Path, script: &str, lock: &str, outputs: &[&str]) -> Result<()> {
    let output_paths = outputs
        .iter()
        .map(|path| root.join(path))
        .collect::<Vec<_>>();
    // A foundation gate owns exactly the outputs declared by its manifest.
    // Scanning every archive below `_build` here made unrelated provider or
    // probe artifacts invalidate all 14 gates, defeating the persistent
    // native graph.  The source-pinned lock/script plus this gate's own
    // outputs are the complete local cache boundary; cross-gate ordering is
    // already explicit in `run_graphics_upstream_gates`.
    let mut inputs = output_paths.clone();
    inputs.push(root.join(lock));
    if script == "build-android16-tracing-perfetto.sh" {
        for path in [
            "upstream/android16-tracing-perfetto.sources",
            "patches/tracing-perfetto/0001-explicit-dynamic-track-name.patch",
            "tools/tracing-perfetto-smoke.cc",
        ] {
            inputs.push(root.join(path));
        }
    }
    if script == "build-android16-debugstore.sh" {
        for path in [
            "tools/debugstore/Cargo.toml",
            "tools/debugstore/Cargo.lock",
            "tools/debugstore/build.rs",
            "tools/debugstore/smoke.cc",
        ] {
            inputs.push(root.join(path));
        }
    }
    if script == "build-android16-activity-thread.sh" {
        inputs.push(root.join("tools/activity-thread/bindings.h"));
        inputs.push(
            root.join("tools/bionic-libc-allocator-facade/include/darwin_art_bionic_allocator.h"),
        );
    }
    if script == "build-android16-application-shared-memory.sh" {
        for path in [
            "compat/memory/application_memory.cc",
            "compat/memory/application_memory.h",
            "compat/memory/application_descriptor.cc",
            "compat/memory/application_descriptor.h",
            "tools/tests/application-descriptor-table.h",
            "compat/memory/system_region.cc",
            "compat/memory/system_region.h",
            "patches/application-shared-memory/0001-darwin-region-capabilities.patch",
            "tools/application-shared-memory-layout-test.cc",
            "tools/system-region-test.cc",
        ] {
            inputs.push(root.join(path));
        }
    }
    build_shell_gate_cached(root, script, &[], &inputs, &output_paths)
}

/// Build the source-pinned native closure consumed by the graphics link audit.
///
/// This phase is intentionally independent from probe compilation and final
/// symbol verification. The full audit invokes it, while the fast inner-loop
/// audit can reuse the already-materialized archives.
pub(super) fn run_graphics_upstream_gates(root: &Path, incremental: bool) -> Result<()> {
    if incremental {
        cached_bionic_provider_gate(root)?;
    } else {
        build_shell_gate(root, "build-bionic-runtime-provider-closure.sh")?;
    }
    if incremental {
        cached_bionic_linker_config_gate(root)?;
    } else {
        build_shell_gate(root, "build-android16-bionic-linker-config.sh")?;
    }
    build_shell_gate_with_args(
        root,
        "audit-android16-graphics-closure.sh",
        &["--art-runtime"],
    )?;
    // Publish the same process VM seam as headless. The separate upstream
    // HostRuntime audit compiles test fixtures and is not a product prerequisite.
    build_framework_java_vm_provider(root)?;
    let gates = [
        (
            "build-android16-libcore-darwin-linux.sh",
            "upstream/android16-libcore-darwin-linux.lock",
            &["_build/libcore-darwin-linux/libcore-darwin-linux.a"][..],
        ),
        (
            "build-android16-os-constants-darwin.sh",
            "upstream/android16-os-constants.lock",
            &["_build/os-constants/libandroid-system-os-constants-darwin.a"][..],
        ),
        (
            "build-android16-unix-filesystem-darwin.sh",
            "upstream/android16-unix-filesystem-darwin.lock",
            &["_build/unix-filesystem-darwin/libopenjdk-unix-filesystem-darwin.a"][..],
        ),
        (
            "build-android16-openjdkjvm-darwin.sh",
            "upstream/android16-openjdkjvm-darwin.lock",
            &["_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a"][..],
        ),
        (
            "build-android16-file-input-stream-darwin.sh",
            "upstream/android16-file-input-stream-darwin.lock",
            &["_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a"][..],
        ),
        (
            "build-android16-file-descriptor-darwin.sh",
            "upstream/android16-file-descriptor-darwin.lock",
            &["_build/file-descriptor-darwin/libopenjdk-file-descriptor-darwin.a"][..],
        ),
        (
            "build-android16-system-natives-darwin.sh",
            "upstream/android16-system-natives-darwin.lock",
            &[
                "_build/system-natives-darwin/libopenjdk-system-natives-darwin.a",
                "_build/system-natives-darwin/libcrypto-boringssl-darwin.a",
            ][..],
        ),
        (
            "build-android16-unix-native-dispatcher-darwin.sh",
            "upstream/android16-unix-native-dispatcher-darwin.lock",
            &[
                "_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a",
                "_build/unix-native-dispatcher-darwin/libfdlibm-darwin.a",
            ][..],
        ),
        (
            "build-android16-openjdk-nio-mapping.sh",
            "upstream/android16-openjdk-nio-mapping.lock",
            &[
                "_build/openjdk-nio-mapping/libopenjdk-nio-mapping-darwin.a",
                "_build/openjdk-nio-mapping/libopenjdk-nio-support-darwin.a",
            ][..],
        ),
        (
            "build-android16-libcore-memory-darwin.sh",
            "upstream/android16-libcore-memory.lock",
            &[
                "_build/libcore-memory/libcore-memory-darwin.a",
                "_build/libcore-memory/libcore-jni-constants-darwin.a",
                "_build/libcore-memory/libcore-memory-art-runtime-darwin.a",
                "_build/libcore-memory/libcore-jni-constants-art-runtime-darwin.a",
            ][..],
        ),
        (
            "build-android16-android-util-log.sh",
            "upstream/android16-android-util-log.lock",
            &["_build/android-util-log/libandroid-util-log-registrar-darwin.a"][..],
        ),
        (
            "build-android16-native-library-helper.sh",
            "upstream/android16-native-library-helper.lock",
            &["_build/native-library-helper/libandroid-native-library-helper-darwin.a"][..],
        ),
        (
            "build-android16-virtual-ref-base-ptr.sh",
            "upstream/android16-virtual-ref-base-ptr.lock",
            &["_build/virtual-ref-base-ptr/libandroid-virtual-ref-base-ptr-darwin.a"][..],
        ),
        (
            "build-android16-application-shared-memory.sh",
            "upstream/android16-application-shared-memory.lock",
            &["_build/application-shared-memory/libapplication-shared-memory-darwin.a"][..],
        ),
        (
            "build-android16-debugstore.sh",
            "upstream/android16-debugstore.lock",
            &["_build/debugstore/libdebugstore-darwin.a"][..],
        ),
        (
            "build-android16-activity-thread.sh",
            "upstream/android16-activity-thread.lock",
            &["_build/activity-thread/libactivity-thread-darwin.a"][..],
        ),
        (
            "build-android16-tracing-perfetto.sh",
            "upstream/android16-tracing-perfetto.lock",
            &[
                "_build/tracing-perfetto/libandroid-tracing-perfetto-darwin.a",
                "_build/tracing-perfetto/perfetto-out/libperfetto_c.dylib",
            ][..],
        ),
    ];
    for (script, lock, outputs) in gates {
        if incremental {
            cached_foundation_gate(root, script, lock, outputs)?;
        } else {
            build_shell_gate(root, script)?;
        }
    }
    Ok(())
}
