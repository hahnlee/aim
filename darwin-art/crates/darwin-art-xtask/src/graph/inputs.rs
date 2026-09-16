use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use darwin_art_build_contract::{GRAPHICS_ADAPTER_SOURCES, HEADLESS_ADAPTER_SOURCES};

use super::super::ninja_path;
use super::atomic;

pub(crate) fn graph_inputs(root: &Path) -> Vec<PathBuf> {
    // This digest is the invalidation boundary for native objects, not for
    // the Rust command that happens to emit the Ninja file.  Rust orchestration
    // changes are intentionally excluded here: changing an xtask, bootstrap
    // CLI, or Cargo manifest must not force hundreds of unchanged C++/ObjC++
    // translation units to rebuild.  Bump GRAPH_VERSION when the graph
    // policy/command generation itself changes.
    let mut paths = vec![
        PathBuf::from("sources.lock"),
        PathBuf::from("compat/filesystem/archive_open.cc"),
        PathBuf::from("bootclasspath.lock"),
        // The canonical builder writes this stamp after preparing the shared
        // runtime cache. Including it prevents a graph emitted before
        // preparation from being reused after the cache becomes promotable.
        PathBuf::from("_build/runtime-common/cache-identity"),
        PathBuf::from("tools/build-android-elf-jni-fixture.sh"),
        PathBuf::from("tools/audit-android16-graphics-closure.sh"),
        PathBuf::from("tools/audit-jit-layout.sh"),
        PathBuf::from("tools/jit-layout-smoke.cc"),
        PathBuf::from("probes/runtime_network_probe.cc"),
        PathBuf::from("probes/runtime_network_probe.h"),
        PathBuf::from("probes/runtime_acceptance_phases.cc"),
        PathBuf::from("probes/runtime_acceptance_phases.h"),
        PathBuf::from("probes/runtime_hwui_probe.cc"),
        PathBuf::from("probes/runtime_hwui_probe.h"),
        PathBuf::from("probes/runtime_entry_probe.cc"),
        PathBuf::from("probes/runtime_registration_phase.cc"),
        PathBuf::from("probes/runtime_registration_phase.h"),
        PathBuf::from("probes/runtime_network_loader.cc"),
        PathBuf::from("probes/runtime_context_loader.cc"),
        PathBuf::from("probes/runtime_app_bootstrap.cc"),
        PathBuf::from("probes/runtime_app_bootstrap.h"),
        PathBuf::from("probes/runtime_app_resources.cc"),
        PathBuf::from("probes/runtime_app_resources.h"),
        PathBuf::from("probes/runtime_app_activity.cc"),
        PathBuf::from("probes/runtime_app_activity.h"),
        PathBuf::from("probes/runtime_app_presentation.cc"),
        PathBuf::from("probes/runtime_app_presentation.h"),
        PathBuf::from("probes/runtime_link_probe.cc"),
        PathBuf::from("probes/runtime_elf_probe.cc"),
        PathBuf::from("probes/runtime_elf_probe.h"),
        PathBuf::from("probes/runtime_abi_probe.cc"),
        PathBuf::from("probes/runtime_abi_probe.h"),
        PathBuf::from("probes/runtime_process_state.cc"),
        PathBuf::from("probes/runtime_process_state.h"),
        PathBuf::from("probes/runtime_process_options.cc"),
        PathBuf::from("probes/runtime_process_options.h"),
        PathBuf::from("probes/runtime_jni_scope.h"),
        PathBuf::from("probes/runtime_upstream_test.h"),
        PathBuf::from("probes/runtime_upstream_arttest.cc"),
        PathBuf::from("_aosp/art/test/common/runtime_state.cc"),
        PathBuf::from("_aosp/art/test/common/stack_inspect.cc"),
        PathBuf::from("probes/runtime_jit_invoke_custom.h"),
        PathBuf::from("probes/runtime_jit_specialized_intrinsics.h"),
        PathBuf::from("probes/runtime_jit_string_intrinsics.h"),
        PathBuf::from("probes/runtime_jit_string_hidden_intrinsics.h"),
        PathBuf::from("probes/runtime_jit_system_arraycopy.h"),
        PathBuf::from("probes/runtime_jit_math_hinvoke.h"),
        PathBuf::from("probes/runtime_jit_crc32.h"),
        PathBuf::from("probes/runtime_jit_memory.h"),
        PathBuf::from("probes/runtime_jit_reference_boxing.h"),
        PathBuf::from("probes/runtime_jit_unsafe_intrinsics.h"),
        PathBuf::from("probes/runtime_shutdown_probe.cc"),
        PathBuf::from("probes/runtime_shutdown_probe.h"),
        PathBuf::from("probes/runtime_frame_probe.cc"),
        PathBuf::from("probes/runtime_frame_probe.h"),
        PathBuf::from("probes/runtime_graphics_probe.cc"),
        PathBuf::from("probes/runtime_graphics_probe.h"),
        PathBuf::from("probes/runtime_graphics_gpu.cc"),
        PathBuf::from("probes/runtime_graphics_gpu.h"),
        PathBuf::from("probes/runtime_graphics_phase.cc"),
        PathBuf::from("probes/runtime_graphics_phase.h"),
        PathBuf::from("probes/runtime_graphics_input.cc"),
        PathBuf::from("probes/runtime_graphics_state.cc"),
        PathBuf::from("probes/runtime_graphics_state.h"),
        PathBuf::from("probes/runtime_graphics_session.cc"),
        PathBuf::from("probes/runtime_graphics_session.h"),
        PathBuf::from("probes/runtime_graphics_cpu_stubs.cc"),
        PathBuf::from("probes/runtime_jni_acceptance_probe.cc"),
        PathBuf::from("probes/runtime_jni_acceptance_probe.h"),
        // The JNI acceptance TU includes this transitive JIT checkpoint
        // fixture; track it explicitly so header-only regression changes
        // invalidate the native graph and cannot leave a stale probe object.
        PathBuf::from("probes/runtime_jit_loop_checkpoint.h"),
        PathBuf::from("probes/runtime_graphics_probe_internal.h"),
        PathBuf::from("probes/runtime_apk_graph.cc"),
        PathBuf::from("probes/runtime_apk_graph.h"),
        PathBuf::from("compat/darwin_surface_bridge.mm"),
        PathBuf::from("compat/darwin_surface_bridge.h"),
        PathBuf::from("compat/darwin_surface_internal.h"),
        PathBuf::from("compat/darwin_surface_gpu_bridge.mm"),
        PathBuf::from("compat/darwin_audio_track.h"),
        PathBuf::from("compat/darwin_android_asset_manager.h"),
        PathBuf::from("compat/darwin_android_platform.h"),
        PathBuf::from("compat/network/multinetwork.h"),
        PathBuf::from("compat/darwin_provider_owners.cc"),
        PathBuf::from("compat/darwin_provider_owners.h"),
        // Remote unwind's Mach task cache is header-only; keep it explicit so
        // retry/permission changes invalidate the provider object and cannot
        // silently reuse a stale graphics archive.
        PathBuf::from("compat/darwin_remote_task_cache.h"),
        PathBuf::from("compat/darwin_unwindstack_linux_abi.h"),
        // The libcore Linux archive is produced by a standalone shell edge,
        // so its C++ inputs must be explicit graph inputs rather than relying
        // on the shell script's timestamp to represent all three objects.
        // This keeps system/metadata edits and syscall edits incremental while
        // preserving the generated 135-entry registrar boundary.
        PathBuf::from("compat/libcore_darwin_linux.cc"),
        PathBuf::from("compat/libcore_darwin_linux_system_natives.cc"),
        PathBuf::from("compat/libcore_darwin_linux_syscalls.cc"),
        PathBuf::from("compat/libcore_darwin_linux.h"),
        PathBuf::from("compat/darwin_asynchronous_close_monitor.cc"),
        PathBuf::from("probes/android16_asynchronous_close_monitor_smoke.cc"),
        PathBuf::from("probes/android16_asynchronous_close_monitor_jni.cc"),
        PathBuf::from("compat/darwin_libcore_filesystem_bridge.c"),
        PathBuf::from("compat/darwin_openjdk_nio_copy.c"),
        PathBuf::from("compat/darwin_openjdk_nio_fs_redirect.h"),
        PathBuf::from("probes/android16_unix_filesystem_jni.c"),
        PathBuf::from("probes/unix-filesystem/UnixFileSystemDarwinSmoke.java"),
        PathBuf::from("upstream/android16-libcore-darwin-linux.lock"),
        PathBuf::from("upstream/android16-asynchronous-close-monitor.lock"),
        PathBuf::from("upstream/android16-os-constants.lock"),
        PathBuf::from("upstream/android16-os-constants-values.tsv"),
        PathBuf::from("upstream/android16-unix-filesystem-darwin.lock"),
        PathBuf::from("upstream/android16-system-natives-darwin.lock"),
        PathBuf::from("tools/bionic-errno-tls/include/darwin_art_bionic_errno.h"),
    ];
    // Keep this graph tied to the production bootstrap closure. Acceptance
    // probes and unrelated native sources must not rotate the persistent
    // runtime cache identity. The adapter manifest is the source of truth for
    // implementation TUs; compatibility headers/includes are still collected
    // recursively because they participate in compiler dependency checks.
    let mut adapter_sources = HEADLESS_ADAPTER_SOURCES
        .iter()
        .chain(GRAPHICS_ADAPTER_SOURCES.iter())
        .map(|source| PathBuf::from("compat").join(source))
        .collect::<Vec<_>>();
    adapter_sources.sort();
    adapter_sources.dedup();
    paths.extend(adapter_sources);
    for directory in [
        "include",
        "patches/art",
        "patches/boringssl",
        "patches/libcore-openjdk",
        "patches/application-shared-memory",
        "patches/ziparchive",
        "patches/package-dex-usage",
        "tools/tests/package-dex-usage",
        "crates/darwin-art-elf-loader/src",
        "tools/android-jni-proxy/include",
        "tools/android-jni-proxy/generated",
        "tools/android-dl-iterate-phdr-provider/include",
        "tools/bionic-dns-facade/include",
        "tools/bionic-dso-lifecycle-facade/include",
        "tools/bionic-fs-facade/include",
        "tools/bionic-ioctl-facade/include",
        "tools/bionic-provider-namespace/include",
        "tools/bionic-sendfile-facade/include",
        "tools/bionic-socket-broker-adapter/include",
        "tools/bionic-stdio-facade/include",
        "tools/bionic-strftime-facade/include",
    ] {
        collect_files(&root.join(directory), root, &mut paths);
    }
    collect_compat_support_files(&root.join("compat"), root, &mut paths);
    collect_files(&root.join("tools/system-properties"), root, &mut paths);
    for script in [
        "build-bionic-runtime-provider-closure.sh",
        "build-android16-android-runtime-host.sh",
        "build-android16-asynchronous-close-monitor.sh",
        "build-android16-libcore-darwin-linux.sh",
        "build-android16-os-constants-darwin.sh",
        "build-android16-unix-filesystem-darwin.sh",
        "build-android16-openjdkjvm-darwin.sh",
        "build-android16-file-input-stream-darwin.sh",
        "build-android16-file-descriptor-darwin.sh",
        "build-android16-system-natives-darwin.sh",
        "build-android16-openjdk-named-jni-owner.sh",
        "build-android16-unix-native-dispatcher-darwin.sh",
        "build-android16-openjdk-nio-mapping.sh",
        "build-android16-libcore-memory-darwin.sh",
        "build-android16-android-util-log.sh",
        "build-android16-virtual-ref-base-ptr.sh",
        "build-android16-application-shared-memory.sh",
        "build-android16-debugstore.sh",
        "build-android16-activity-thread.sh",
        "build-android16-system-properties.sh",
        "build-android16-tracing-perfetto.sh",
    ] {
        paths.push(PathBuf::from("tools").join(script));
    }
    paths.extend([
        PathBuf::from("upstream/android16-application-shared-memory.lock"),
        PathBuf::from("upstream/android16-debugstore.lock"),
        PathBuf::from("upstream/android16-activity-thread.lock"),
        PathBuf::from("upstream/android16-system-properties.lock"),
        PathBuf::from("tools/activity-thread/bindings.h"),
        PathBuf::from("runtime/framework/app/activity_thread_jni.h"),
        PathBuf::from("compat/loader/android_dlwarning.cc"),
        PathBuf::from("upstream/android16-bionic-linker-config.sources"),
        PathBuf::from("upstream/android16-tracing-perfetto.lock"),
        PathBuf::from("upstream/android16-tracing-perfetto.sources"),
        PathBuf::from("patches/tracing-perfetto/0001-explicit-dynamic-track-name.patch"),
        PathBuf::from("tools/tracing-perfetto-smoke.cc"),
        PathBuf::from("tools/debugstore/Cargo.toml"),
        PathBuf::from("tools/debugstore/Cargo.lock"),
        PathBuf::from("tools/debugstore/build.rs"),
        PathBuf::from("tools/debugstore/smoke.cc"),
        PathBuf::from("compat/memory/application_memory.cc"),
        PathBuf::from("compat/memory/application_descriptor.cc"),
        PathBuf::from("compat/memory/application_descriptor.h"),
        PathBuf::from("compat/binder/context_manager.h"),
        PathBuf::from("compat/loader/classloader_identity.h"),
        PathBuf::from("compat/loader/library_search.h"),
        PathBuf::from("compat/loader/elf_graph_cache.h"),
        PathBuf::from("compat/loader/namespace_elf_group.h"),
        PathBuf::from("compat/window/locked_surface.h"),
        PathBuf::from("compat/window/hardware_buffer_jni.h"),
        PathBuf::from("runtime/framework/wm/client_transaction.h"),
        PathBuf::from("runtime/framework/wm/window_title.h"),
        PathBuf::from("runtime/framework/app/main_loop.h"),
        PathBuf::from("runtime/framework/app/process_entry.h"),
        PathBuf::from("runtime/framework/system/process_entry.h"),
        PathBuf::from("runtime/framework/connectivity/network_path_abi.h"),
        PathBuf::from("runtime/framework/connectivity/network_provider_jni.h"),
        PathBuf::from("runtime/framework/power/power_state_platform.h"),
        PathBuf::from("runtime/framework/power/power_state_jni.h"),
        PathBuf::from("runtime/framework/app/process_registration.h"),
        PathBuf::from("runtime/framework/pm/DexInstructionSets.java"),
        PathBuf::from("runtime/framework/pm/DexUsageStore.java"),
        PathBuf::from("runtime/framework/pm/DexLoadReports.java"),
        PathBuf::from("runtime/framework/pm/InstalledApplicationInfo.java"),
        PathBuf::from("runtime/framework/pm/InstalledManifestMetadata.java"),
        PathBuf::from("runtime/framework/pm/InstalledPackageRecord.java"),
        PathBuf::from("runtime/framework/pm/installed_record_source.h"),
        PathBuf::from("runtime/framework/pm/PackageManagerEndpoint.java"),
        PathBuf::from("runtime/framework/pm/PackageRecords.java"),
        PathBuf::from("upstream/android16-package-dex-usage.lock"),
        PathBuf::from("tools/build-android16-package-dex-usage.sh"),
        PathBuf::from("tools/test-android16-package-dex-usage.sh"),
        PathBuf::from("tools/binder-context-manager-test.cc"),
        PathBuf::from("tools/tests/application-descriptor-table.h"),
        PathBuf::from("compat/memory/system_region.cc"),
        PathBuf::from("tools/application-shared-memory-layout-test.cc"),
        PathBuf::from("tools/system-region-test.cc"),
        PathBuf::from("probes/android-elf-jni-fixture/child.c"),
        PathBuf::from("probes/android-elf-jni-fixture/child.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_child.c"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_child.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_grandchild.c"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_grandchild.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_root.c"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_root.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/grandchild.c"),
        PathBuf::from("probes/android-elf-jni-fixture/grandchild.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/host_provider.c"),
        PathBuf::from("probes/android-elf-jni-fixture/native_fixture.c"),
        PathBuf::from("tools/android-jni-proxy/src/proxy.c"),
        PathBuf::from("tools/android-jni-proxy/src/aapcs64_call.S"),
        PathBuf::from("tools/android-jni-proxy/sources.lock"),
    ]);
    paths.sort();
    paths.dedup();
    paths.retain(|path| root.join(path).is_file());
    paths
}

pub(crate) fn is_probe_only_input(path: &Path) -> bool {
    matches!(
        path.to_string_lossy().as_ref(),
        "probes/runtime_network_probe.cc"
            | "probes/runtime_network_probe.h"
            | "probes/runtime_acceptance_phases.cc"
            | "probes/runtime_acceptance_phases.h"
            | "probes/runtime_hwui_probe.cc"
            | "probes/runtime_hwui_probe.h"
            | "probes/runtime_entry_probe.cc"
            | "probes/runtime_registration_phase.cc"
            | "probes/runtime_registration_phase.h"
            | "probes/runtime_app_bootstrap.cc"
            | "probes/runtime_app_bootstrap.h"
            | "probes/runtime_app_resources.cc"
            | "probes/runtime_app_resources.h"
            | "probes/runtime_app_activity.cc"
            | "probes/runtime_app_activity.h"
            | "probes/runtime_app_presentation.cc"
            | "probes/runtime_app_presentation.h"
            | "probes/runtime_link_probe.cc"
            | "probes/runtime_elf_probe.cc"
            | "probes/runtime_elf_probe.h"
            | "probes/runtime_abi_probe.cc"
            | "probes/runtime_abi_probe.h"
            | "probes/runtime_process_state.cc"
            | "probes/runtime_process_state.h"
            | "probes/runtime_process_options.cc"
            | "probes/runtime_process_options.h"
            | "probes/runtime_context_loader.cc"
            | "probes/runtime_jni_scope.h"
            | "probes/runtime_upstream_test.h"
            | "probes/runtime_upstream_arttest.cc"
            | "_aosp/art/test/common/runtime_state.cc"
            | "_aosp/art/test/common/stack_inspect.cc"
            | "probes/runtime_jit_invoke_custom.h"
            | "probes/runtime_jit_specialized_intrinsics.h"
            | "probes/runtime_jit_string_intrinsics.h"
            | "probes/runtime_jit_string_hidden_intrinsics.h"
            | "probes/runtime_jit_system_arraycopy.h"
            | "probes/runtime_jit_math_hinvoke.h"
            | "probes/runtime_jit_crc32.h"
            | "probes/runtime_jit_memory.h"
            | "probes/runtime_jit_reference_boxing.h"
            | "probes/runtime_jit_unsafe_intrinsics.h"
            | "probes/runtime_shutdown_probe.cc"
            | "probes/runtime_shutdown_probe.h"
            | "probes/runtime_frame_probe.cc"
            | "probes/runtime_frame_probe.h"
            | "probes/runtime_graphics_probe.cc"
            | "probes/runtime_graphics_probe.h"
            | "probes/runtime_graphics_gpu.cc"
            | "probes/runtime_graphics_gpu.h"
            | "probes/runtime_graphics_phase.cc"
            | "probes/runtime_graphics_phase.h"
            | "probes/runtime_graphics_input.cc"
            | "probes/runtime_graphics_state.cc"
            | "probes/runtime_graphics_state.h"
            | "probes/runtime_graphics_session.cc"
            | "probes/runtime_graphics_session.h"
            | "probes/runtime_jni_acceptance_probe.cc"
            | "probes/runtime_jni_acceptance_probe.h"
            | "probes/runtime_graphics_probe_internal.h"
            | "probes/runtime_apk_graph.cc"
            | "probes/runtime_apk_graph.h"
            | "compat/darwin_surface_bridge.mm"
            | "compat/darwin_surface_bridge.h"
            | "compat/darwin_surface_internal.h"
            | "compat/darwin_surface_gpu_bridge.mm"
    )
}

pub(crate) fn collect_files(directory: &Path, root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("target" | ".git" | "__pycache__")
            ) {
                continue;
            }
            collect_files(&path, root, output);
        } else if path.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            output.push(relative.to_path_buf());
        }
    }
}

/// Collect compatibility headers and generated include fragments without
/// reintroducing every native implementation into the global graph digest.
/// Implementation TUs are listed explicitly by the shared adapter contract;
/// depfiles remain authoritative for any transitive include discovered by the
/// compiler.
fn collect_compat_support_files(directory: &Path, root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_compat_support_files(&path, root, output);
            continue;
        }
        let is_support = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "h" | "hh" | "hpp" | "inc"));
        if is_support && let Ok(relative) = path.strip_prefix(root) {
            output.push(relative.to_path_buf());
        }
    }
}

pub(crate) fn probe_inputs(root: &Path, paths: &[&str]) -> String {
    probe_input_paths(root, paths)
        .into_iter()
        .map(|path| ninja_path(&path))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn probe_input_paths(root: &Path, paths: &[&str]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| root.join(path))
        .filter(|path| path.is_file())
        .collect()
}

/// Materialize a stable content identity for one narrow probe phase.
///
/// The stamp is updated only when the bytes or input path set changes, so a
/// repeated graph generation remains a true Ninja warm no-op.  Keeping one
/// stamp per phase is important: changing graphics state must not make the
/// graphics input/session objects dirty merely because the global graph
/// digest changed.
pub(crate) fn probe_content_stamp(root: &Path, name: &str, paths: &[&str]) -> io::Result<PathBuf> {
    let inputs = probe_input_paths(root, paths);
    let mut digest = Sha256::new();
    digest.update(b"darwin-art-probe-content-v1\0");
    for path in &inputs {
        let relative = path.strip_prefix(root).unwrap_or(path);
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    let content = format!("{:x}\n", digest.finalize());
    let stamp = root
        .join("_build/runtime-probes/content-stamps")
        .join(format!("{name}.sha256"));
    if fs::read_to_string(&stamp).ok().as_deref() != Some(&content) {
        if let Some(parent) = stamp.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic::write(&stamp, content.as_bytes())?;
    }
    Ok(stamp)
}
