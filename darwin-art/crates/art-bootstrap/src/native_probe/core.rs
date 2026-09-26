//! Shared production translation units and separately compiled test fixtures.
//!
//! Keeping production objects independent of fixtures means the CPU,
//! graphics, and APK graph actions can use one dependency-fingerprinted cache
//! instead of each command owning a copy of the compilation policy.

use super::*;
use crate::build_context::BuildPaths;

/// Canonical ART include search path for shared native translation units.
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
        root.join("_aosp/art/libarttools/include"),
        root.join("_aosp/system/core/fs_mgr/libfstab/include"),
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
        // JNIHelp.h for libartservice. After header_only_include: on a
        // case-insensitive volume its Utils.h would shadow nativehelper/utils.h.
        root.join("_aosp/libnativehelper-full/include"),
        root.join("_aosp/system/logging/liblog/include"),
        // The pinned fmt the runtime links, ahead of any host installation.
        root.join("_aosp/external/fmtlib/include"),
        PathBuf::from("/opt/homebrew/include"),
    ]
}

pub(crate) struct RuntimeCoreObjects {
    pub(crate) boot_native_registration: PathBuf,
    pub(crate) boot_native_libraries: PathBuf,
    /// libartservice: service-art.jar's ArtJni natives (ADR 0009).
    pub(crate) art_service: PathBuf,
    /// libarttools EnsureNoProcessInDir for Darwin processes.
    pub(crate) art_tools_process: PathBuf,
    pub(crate) vm_bootstrap: PathBuf,
    pub(crate) process_entry: PathBuf,
    pub(crate) process_state: PathBuf,
    pub(crate) process_config: PathBuf,
    pub(crate) process_shutdown: PathBuf,
    pub(crate) vm_shutdown: PathBuf,
    pub(crate) app_process_shutdown: PathBuf,
}

pub(crate) struct FixtureCoreObjects {
    pub(crate) elf: PathBuf,
    pub(crate) abi: PathBuf,
    pub(crate) acceptance: PathBuf,
    pub(crate) fixture_options: PathBuf,
    pub(crate) graphics_fixture_state: PathBuf,
    pub(crate) shutdown: PathBuf,
    pub(crate) frame: PathBuf,
}

pub(crate) fn compile_runtime_core_objects(
    root: &Path,
    build_dir: &Path,
    include_refs: &[&Path],
    probe_cache: &Path,
    compiler_identity: &str,
) -> Result<RuntimeCoreObjects> {
    Ok(RuntimeCoreObjects {
        boot_native_registration: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/art/boot_native_registration.cc",
            "darwin_art_boot_native_registration.cc.o",
        )?,
        boot_native_libraries: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "compat/art/boot_native_libraries.cc",
            "darwin_art_boot_native_libraries.cc.o",
        )?,
        art_service: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "_aosp/art/libartservice/service/native/service.cc",
            "art_libartservice_service.cc.o",
        )?,
        art_tools_process: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "compat/art/process_dir_monitor.cc",
            "darwin_art_process_dir_monitor.cc.o",
        )?,
        vm_bootstrap: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/art/vm_bootstrap.cc",
            "darwin_art_vm_bootstrap.cc.o",
        )?,
        process_entry: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/embedding/process_entry.cc",
            "darwin_art_process_entry.cc.o",
        )?,
        process_state: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/art/process_state.cc",
            "darwin_art_process_state.cc.o",
        )?,
        process_config: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/embedding/process_config.cc",
            "darwin_art_process_config.cc.o",
        )?,
        process_shutdown: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/embedding/process_shutdown.cc",
            "darwin_art_process_shutdown.cc.o",
        )?,
        vm_shutdown: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/art/vm_shutdown.cc",
            "darwin_art_vm_shutdown.cc.o",
        )?,
        app_process_shutdown: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "runtime/art/shutdown_readiness.cc",
            "darwin_art_app_process_shutdown.cc.o",
        )?,
    })
}

pub(crate) fn compile_fixture_core_objects(
    root: &Path,
    build_dir: &Path,
    include_refs: &[&Path],
    probe_cache: &Path,
    compiler_identity: &str,
) -> Result<FixtureCoreObjects> {
    Ok(FixtureCoreObjects {
        elf: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/runtime_elf_probe.cc",
            "darwin_art_runtime_elf_probe.cc.o",
        )?,
        abi: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/runtime_abi_probe.cc",
            "darwin_art_runtime_abi_probe.cc.o",
        )?,
        acceptance: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/runtime_acceptance_state.cc",
            "darwin_art_runtime_acceptance_state.cc.o",
        )?,
        fixture_options: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/runtime_fixture_options.cc",
            "darwin_art_runtime_fixture_options.cc.o",
        )?,
        graphics_fixture_state: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/graphics_fixture_state.cc",
            "darwin_art_graphics_fixture_state.cc.o",
        )?,
        shutdown: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/runtime_shutdown_probe.cc",
            "darwin_art_runtime_shutdown_probe.cc.o",
        )?,
        frame: compile_probe(
            root,
            build_dir,
            include_refs,
            probe_cache,
            compiler_identity,
            "probes/runtime_frame_probe.cc",
            "darwin_art_runtime_frame_probe.cc.o",
        )?,
    })
}

pub(crate) fn compile_native_registration(
    root: &Path,
    build_dir: &Path,
    include_refs: &[&Path],
    cache_path: &Path,
    compiler_identity: &str,
    definitions: &[&str],
) -> Result<PathBuf> {
    let object = build_dir.join("darwin_art_native_registration.cc.o");
    let mut command = runtime_cpp_command(include_refs);
    command
        .args(definitions)
        .arg("-c")
        .arg(root.join("runtime/art/native_registration.cc"));
    command.arg("-o").arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, cache_path, compiler_identity)?;
    Ok(object)
}

pub(crate) fn build_runtime_native_registration(root: &Path) -> Result<()> {
    let build_paths = BuildPaths::from_root(root);
    let build_dir = build_paths.native_output("runtime-link-probe");
    fs::create_dir_all(&build_dir)?;
    let runtime = root.join("_aosp/art/runtime");
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let cache_path = build_dir.join("native-registration-hashes.cache");
    let object = compile_native_registration(
        root,
        &build_dir,
        &include_refs,
        &cache_path,
        &compiler_identity,
        &[],
    )?;
    println!("build-runtime-native-registration: {}", object.display());
    Ok(())
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
        .arg(root.join(source))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, probe_cache, compiler_identity)?;
    Ok(object)
}

/// JNI entry points (`_Java_*`) an object defines, for the runtime export list.
pub(crate) fn jni_entrypoints(object: &Path) -> Result<Vec<String>> {
    let symbols = command_output(Command::new("nm").args(["-gU"]).arg(object))?;
    Ok(symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|symbol| symbol.starts_with("_Java_"))
        .map(str::to_owned)
        .collect())
}
