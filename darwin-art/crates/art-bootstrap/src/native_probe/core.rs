//! Flavor-neutral probe translation units shared by every runtime consumer.
//!
//! Keeping these six objects outside an audit/probe command means the CPU,
//! graphics, and APK graph actions can use one dependency-fingerprinted cache
//! instead of each command owning a copy of the compilation policy.

use super::*;
use crate::build_context::BuildPaths;

/// Canonical include search path for the six flavor-neutral probe TUs.
///
/// Keeping this list in one module is important: an object cache is only
/// useful when CPU, Metal, and APK actions fingerprint the same compiler
/// inputs. Flavor-specific probes may append their own headers, but they must
/// not silently change this core path.
pub(crate) fn core_probe_includes(
    root: &Path,
    build_paths: &BuildPaths,
    runtime: &Path,
) -> Vec<PathBuf> {
    vec![
        root.join("include"),
        root.join("compat"),
        build_paths.native_output("runtime-arm64/generated"),
        build_paths.native_output("runtime-common/patched-source/runtime"),
        build_paths.native_output("foundation/patched-source/libartbase"),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/art/cmdline"),
        root.join("_aosp/art/libdexfile"),
        root.join("_aosp/art/libelffile"),
        root.join("_aosp/art/libprofile"),
        root.join("_aosp/art/libnativebridge/include"),
        root.join("_aosp/art/libnativeloader/include"),
        runtime.to_path_buf(),
        runtime.join("base"),
        runtime.join("arch/arm64"),
        root.join("_aosp/art/libartpalette/include"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/system/unwinding/libunwindstack/include"),
        root.join("_aosp/external/tinyxml2"),
        root.join("_aosp/libnativehelper/include_jni"),
        root.join("_aosp/libnativehelper/header_only_include"),
        root.join("_aosp/libnativehelper/platform_header_only_include"),
        root.join("_aosp/external/dlmalloc"),
        root.join("tools/bionic-dns-facade/include"),
        root.join("tools/bionic-fs-facade/include"),
        root.join("tools/bionic-ioctl-facade/include"),
        root.join("tools/bionic-socket-broker-adapter/include"),
        PathBuf::from("/opt/homebrew/include"),
    ]
}

pub(crate) struct CoreProbeObjects {
    pub(crate) elf: PathBuf,
    pub(crate) abi: PathBuf,
    pub(crate) process_state: PathBuf,
    pub(crate) process_options: PathBuf,
    pub(crate) shutdown: PathBuf,
    pub(crate) frame: PathBuf,
}

pub(crate) fn compile_core_probe_objects(
    root: &Path,
    build_dir: &Path,
    include_refs: &[&Path],
    probe_cache: &Path,
    compiler_identity: &str,
) -> Result<CoreProbeObjects> {
    Ok(CoreProbeObjects {
        elf: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime_elf_probe.cc",
            "darwin_art_runtime_elf_probe.cc.o",
        )?,
        abi: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime_abi_probe.cc",
            "darwin_art_runtime_abi_probe.cc.o",
        )?,
        process_state: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime_process_state.cc",
            "darwin_art_runtime_process_state.cc.o",
        )?,
        process_options: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime_process_options.cc",
            "darwin_art_runtime_process_options.cc.o",
        )?,
        shutdown: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime_shutdown_probe.cc",
            "darwin_art_runtime_shutdown_probe.cc.o",
        )?,
        frame: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime_frame_probe.cc",
            "darwin_art_runtime_frame_probe.cc.o",
        )?,
    })
}

fn compile_probe(
    root: &Path,
    build_dir: &Path,
    include_refs: &[&Path],
    probe_cache: &Path,
    compiler_identity: &str,
    source: &str,
    object_name: &str,
) -> Result<PathBuf> {
    let object = build_dir.join(object_name);
    let mut command = runtime_cpp_command(include_refs);
    command
        .arg("-c")
        .arg(root.join("probes").join(source))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, probe_cache, compiler_identity)?;
    Ok(object)
}
