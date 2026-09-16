use super::manifest::{
    PATCHED_RUNTIME_PATCHES, PATCHED_RUNTIME_SOURCES, RUNTIME_SHADOW_IDENTITY_VERSION,
};
use super::*;
use darwin_art_build_contract::{RUNTIME_CACHE_IDENTITY, RuntimeFlavor};

pub(crate) struct RuntimeBootstrapStaging {
    pub(crate) root: PathBuf,
    pub(crate) build_dir: PathBuf,
    pub(crate) object_dir: PathBuf,
    /// Objects for upstream ART runtime TUs are flavor-independent.  Keep
    /// them outside the runtime/graphics output roots so the second flavor
    /// consumes the first flavor's dependency-fingerprinted objects.
    pub(crate) runtime_core_object_dir: PathBuf,
    pub(crate) patched_runtime: PathBuf,
    pub(crate) libprofile: PathBuf,
    pub(crate) runtime: PathBuf,
    pub(crate) android_jni_include: PathBuf,
    pub(crate) nativehelper_full_include: PathBuf,
    pub(crate) nativehelper_platform_headers: PathBuf,
    pub(crate) liblog_include: PathBuf,
    pub(crate) jni_proxy_include: PathBuf,
    pub(crate) jni_proxy_generated: PathBuf,
    pub(crate) android_icu_common: PathBuf,
    pub(crate) android_icu_i18n: PathBuf,
    pub(crate) libcutils_include: PathBuf,
    pub(crate) ndk_include: PathBuf,
    pub(crate) ndk_arch_include: PathBuf,
    pub(crate) includes: Vec<PathBuf>,
    pub(crate) runtime_includes: Vec<PathBuf>,
    pub(crate) operator_source: PathBuf,
}

/// Materialize the flavor-independent ART source shadow before any compiler
/// is allowed to consume it. Patches are applied and audited in a private
/// candidate tree; published files are replaced only after the complete
/// candidate is valid, and the identity marker is published last.
pub(crate) fn prepare_runtime_shadow(root: &Path) -> Result<PathBuf> {
    let runtime = root.join("_aosp/art/runtime");
    let patched_source_dir = root.join("_build/runtime-common/patched-source");
    let patched_runtime = patched_source_dir.join("runtime");
    let shadow_identity = runtime_shadow_identity(&runtime, root)?;
    let shadow_identity_path = patched_source_dir.join(".darwin-art-shadow-identity");

    let shadow_is_current = || {
        fs::read_to_string(&shadow_identity_path)
            .is_ok_and(|cached| cached.trim() == shadow_identity)
            && PATCHED_RUNTIME_SOURCES
                .iter()
                .all(|source| patched_runtime.join(source).is_file())
    };
    if !shadow_is_current() {
        let _shadow_lock = acquire_shadow_lock(&patched_source_dir)?;
        if !shadow_is_current() {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let candidate_dir = patched_source_dir.with_extension(format!(
                "darwin-art-candidate-{}-{nonce}",
                std::process::id()
            ));
            let candidate = ShadowCandidate(candidate_dir);
            let candidate_runtime = candidate.0.join("runtime");
            fs::create_dir_all(&candidate_runtime)?;
            copy_runtime_sources(&runtime, &candidate_runtime)?;
            for patch in PATCHED_RUNTIME_PATCHES {
                apply_patch_if_needed(&root.join(patch), &candidate.0)?;
            }
            audit_nterp_admission(&candidate_runtime)?;

            fs::create_dir_all(&patched_runtime)?;
            for source in PATCHED_RUNTIME_SOURCES {
                let destination = patched_runtime.join(source);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                copy_if_changed(&candidate_runtime.join(source), &destination)?;
            }
            publish_text_file(&shadow_identity_path, &format!("{shadow_identity}\n"))?;
        }
    }
    audit_nterp_admission(&patched_runtime)?;
    Ok(patched_runtime)
}

pub(crate) fn prepare(root: &Path, flavor: RuntimeFlavor) -> Result<RuntimeBootstrapStaging> {
    let real_graphics = flavor.real_graphics();
    build_shell_gate(root, "build-android-elf-jni-fixture.sh")?;
    let build_paths = BuildPaths::from_root(root);
    build_elf_loader(root)?;

    let artbase = root.join("_aosp/art/libartbase");
    let artbase_base = artbase.join("base");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let patched_artbase_base = patched_artbase.join("base");
    let cmdline = root.join("_aosp/art/cmdline");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let libelffile = root.join("_aosp/art/libelffile");
    let libprofile = root.join("_aosp/art/libprofile");
    let nativebridge_include = root.join("_aosp/art/libnativebridge/include");
    let nativeloader_include = root.join("_aosp/art/libnativeloader/include");
    let runtime = root.join("_aosp/art/runtime");
    let runtime_base = runtime.join("base");
    let runtime_arm64 = runtime.join("arch/arm64");
    let runtime_gc = runtime.join("gc");
    let runtime_gc_collector = runtime.join("gc/collector");
    let runtime_gc_space = runtime.join("gc/space");
    let runtime_quick_entrypoints = runtime.join("entrypoints/quick");
    let runtime_mterp = runtime.join("interpreter/mterp");
    let runtime_oat = runtime.join("oat");
    let generator = root.join("_aosp/art/tools/cpp-define-generator");
    let generated_dir = root.join("_build/runtime-arm64/generated");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    let android_jni_include = root.join("_aosp/libnativehelper/include_jni");
    let nativehelper_headers = root.join("_aosp/libnativehelper/header_only_include");
    let nativehelper_platform_headers =
        root.join("_aosp/libnativehelper/platform_header_only_include");
    let dlmalloc = root.join("_aosp/external/dlmalloc");
    let odrefresh_include = root.join("_aosp/art/odrefresh/include");
    let sigchain = root.join("_aosp/art/sigchainlib");
    let unwindstack_include = root.join("_aosp/system/unwinding/libunwindstack/include");
    let compat = root.join("compat");
    let public_include = root.join("include");
    let graphics_registration_include = root.join("_build/android-graphics-jni/generated");
    let android_icu_include = root.join("_aosp/external/icu-graphics/libandroidicuinit/include");
    let android_icu_common = root.join("_aosp/external/icu-graphics/icu4c/source/common");
    let android_icu_i18n = root.join("_aosp/external/icu-graphics/icu4c/source/i18n");
    let android_graphics_apex_include = root.join("_aosp/frameworks/base/libs/hwui/apex/include");
    let aosp_fmt_include = root.join("_aosp/external/fmtlib/include");
    let libcutils_include = root.join("_aosp/system/core/libcutils/include");
    let liblog_include = root.join("_aosp/system/logging/liblog/include");
    let nativehelper_full_include = root.join("_aosp/libnativehelper-full/include");
    let elf_loader_include = root.join("crates/darwin-art-elf-loader/include");
    let jni_proxy_include = root.join("tools/android-jni-proxy/include");
    let jni_proxy_generated = root.join("tools/android-jni-proxy/generated");
    let elf_fixture_generated = root.join("_build/android-elf-jni-fixture/generated");
    let openjdk_math_source = root.join("_aosp/libcore/ojluni/src/main/native/Math.c");
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;

    if !generated_dir.join("asm_defines.h").exists() {
        return Err(
            "generated ART ABI constants are missing; run `build-runtime-arm64` first".into(),
        );
    }
    if !openjdk_math_source.is_file() {
        return Err(format!(
            "Android libcore Math source is missing: {}; run `art-bootstrap sync` first",
            openjdk_math_source.display()
        )
        .into());
    }

    let build_dir = build_paths.native_output(flavor.output_dir());
    let runtime_generated_dir = build_dir.join("generated");
    // The patched ART runtime sources are identical between the headless and
    // graphics flavors.  A shared shadow is the source-level half of the
    // cross-flavor object cache; adapter sources remain flavor-local below.
    let patched_runtime = prepare_runtime_shadow(root)?;
    let object_dir = build_dir.join("objects");
    let runtime_core_object_dir = root.join("_build/runtime-common/objects");
    fs::create_dir_all(&object_dir)?;
    fs::create_dir_all(&runtime_core_object_dir)?;
    fs::write(
        root.join("_build/runtime-common/cache-identity"),
        format!("{RUNTIME_CACHE_IDENTITY}\n"),
    )?;
    fs::create_dir_all(&runtime_generated_dir)?;

    let runtime_includes = vec![
        public_include,
        compat,
        elf_fixture_generated,
        elf_loader_include,
        jni_proxy_include.clone(),
        generated_dir,
        generator,
        patched_runtime.clone(),
        patched_artbase,
        patched_artbase_base,
        artbase.clone(),
        artbase_base,
        cmdline,
        libdexfile,
        libelffile,
        libprofile.clone(),
        nativebridge_include,
        nativeloader_include,
        odrefresh_include,
        sigchain,
        runtime.clone(),
        runtime_base,
        runtime_arm64,
        runtime_gc,
        runtime_gc_collector,
        runtime_gc_space,
        runtime_quick_entrypoints,
        runtime_mterp,
        runtime_oat,
        palette_include,
        libbase_include,
        unwindstack_include,
        tinyxml2,
        android_jni_include.clone(),
        nativehelper_headers,
        nativehelper_platform_headers.clone(),
        dlmalloc,
        aosp_fmt_include.clone(),
        PathBuf::from("/opt/homebrew/opt/icu4c@78/include"),
        PathBuf::from("/opt/homebrew/include"),
    ];
    let mut includes = runtime_includes.clone();
    if real_graphics {
        let generated_registration =
            graphics_registration_include.join("darwin_android_graphics_registration.h");
        if !generated_registration.is_file() {
            return Err(format!(
                "generated Android graphics registration header is missing: {}; run `build-android-graphics-jni` first",
                generated_registration.display()
            )
            .into());
        }
        if !android_icu_include
            .join("androidicuinit/android_icu_init.h")
            .is_file()
        {
            return Err("Android ICU init headers are missing; run `build-icu` first".into());
        }
        includes.insert(0, graphics_registration_include);
        includes.insert(1, android_icu_include);
        includes.insert(2, android_graphics_apex_include);
        includes.insert(3, aosp_fmt_include);
    }

    let operator_source = runtime_generated_dir.join("operator_out.cc");
    if !operator_source.is_file() {
        let mut generate = Command::new("python3");
        generate
            .arg(root.join("_aosp/art/tools/generate_operator_out.py"))
            .arg(&runtime);
        for header in [
            "base/callee_save_type.h",
            "base/locks.h",
            "class_status.h",
            "compilation_kind.h",
            "gc/allocator/rosalloc.h",
            "gc/allocator_type.h",
            "gc/collector/gc_type.h",
            "gc/collector/mark_compact.h",
            "gc/collector_type.h",
            "gc/space/region_space.h",
            "gc/space/space.h",
            "gc/weak_root_state.h",
            "gc_root.h",
            "indirect_reference_table.h",
            "instrumentation.h",
            "jdwp_provider.h",
            "jni_id_type.h",
            "linear_alloc.h",
            "lock_word.h",
            "oat/image.h",
            "oat/oat.h",
            "oat/oat_file.h",
            "process_state.h",
            "reflective_value_visitor.h",
            "stack.h",
            "suspend_reason.h",
            "thread.h",
            "thread_state.h",
            "trace.h",
            "trace_profile.h",
            "verifier/verifier_enums.h",
        ] {
            generate.arg(runtime.join(header));
        }
        fs::write(&operator_source, command_output(&mut generate)?)?;
    }

    Ok(RuntimeBootstrapStaging {
        root: root.to_owned(),
        build_dir,
        object_dir,
        runtime_core_object_dir,
        patched_runtime,
        libprofile,
        runtime,
        android_jni_include,
        nativehelper_full_include,
        nativehelper_platform_headers,
        liblog_include,
        jni_proxy_include,
        jni_proxy_generated,
        android_icu_common,
        android_icu_i18n,
        libcutils_include,
        ndk_include,
        ndk_arch_include,
        includes,
        runtime_includes,
        operator_source,
    })
}

struct ShadowLock(PathBuf);

impl Drop for ShadowLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

struct ShadowCandidate(PathBuf);

impl Drop for ShadowCandidate {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn acquire_shadow_lock(shadow_dir: &Path) -> Result<ShadowLock> {
    let lock = shadow_dir.with_extension("lock");
    fs::create_dir_all(shadow_dir)?;
    loop {
        match fs::create_dir(&lock) {
            Ok(()) => return Ok(ShadowLock(lock)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn copy_runtime_sources(runtime: &Path, patched_runtime: &Path) -> Result<()> {
    for source in PATCHED_RUNTIME_SOURCES {
        let destination = patched_runtime.join(source);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_if_changed(&runtime.join(source), &destination)?;
    }
    Ok(())
}

/// Fail closed if a legacy Darwin-only Nterp disable patch survives in the
/// shared runtime shadow. The ARM64ng object is part of the production runtime
/// archive, so silently selecting the switch interpreter would make corpus
/// results look like Nterp coverage without executing the native interpreter.
fn audit_nterp_admission(patched_runtime: &Path) -> Result<()> {
    let nterp = fs::read_to_string(patched_runtime.join("interpreter/mterp/nterp.cc"))?;
    let helpers = fs::read_to_string(patched_runtime.join("nterp_helpers.cc"))?;

    for (function, forbidden) in [
        ("IsNterpSupported", "#if defined(__APPLE__)"),
        ("GetNterpEntryPoint", "return nullptr;"),
        ("NterpImpl", "return {};"),
        ("GetNterpWithClinitEntryPoint", "return nullptr;"),
        ("NterpWithClinitImpl", "return {};"),
    ] {
        let body = function_body(&nterp, function)?;
        if body.contains(forbidden) {
            return Err(
                format!("legacy Darwin Nterp gate remains in {function}: {forbidden}").into(),
            );
        }
    }
    if function_body(&helpers, "NterpGetCatchHandler")?.contains("return 0u;") {
        return Err("legacy Darwin Nterp catch-entry gate remains".into());
    }
    Ok(())
}

fn function_body<'a>(source: &'a str, function: &str) -> Result<&'a str> {
    let start = source
        .find(&format!("{function}("))
        .ok_or_else(|| format!("missing pinned ART function {function}"))?;
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .ok_or_else(|| format!("missing body for pinned ART function {function}"))?;
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&source[open..=open + offset]);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated body for pinned ART function {function}").into())
}

fn runtime_shadow_identity(runtime: &Path, root: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(RUNTIME_SHADOW_IDENTITY_VERSION.as_bytes());
    digest.update([0]);
    for path in PATCHED_RUNTIME_SOURCES
        .iter()
        .map(|source| runtime.join(source))
        .chain(PATCHED_RUNTIME_PATCHES.iter().map(|patch| root.join(patch)))
    {
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Preserve the staged source mtime when the source bytes are unchanged.
///
/// The native dependency cache fingerprints metadata for every depfile edge.
/// An unconditional `fs::copy` therefore made every canonical fallback look
/// like a full ART source edit and recompiled hundreds of unchanged TUs.
/// Compare bytes first, then publish changed content through a sibling temp
/// file so an interrupted preparation cannot leave a truncated header/source.
fn copy_if_changed(source: &Path, destination: &Path) -> Result<()> {
    let source_bytes = fs::read(source)?;
    if destination.is_file() && fs::read(destination)? == source_bytes {
        return Ok(());
    }
    let temporary =
        destination.with_extension(format!("darwin-art-copy-tmp-{}", std::process::id()));
    fs::write(&temporary, source_bytes)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

fn publish_text_file(destination: &Path, contents: &str) -> Result<()> {
    let temporary =
        destination.with_extension(format!("darwin-art-publish-tmp-{}", std::process::id()));
    fs::write(&temporary, contents)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

/// Apply one ART shadow-tree patch exactly once.
///
/// Bootstrap commands can be launched concurrently by the native graph and by
/// a direct audit command. The shadow tree is deliberately shared so its
/// generated headers and object paths remain stable, which means a second
/// preparer may observe an already-applied patch. A plain `patch --forward`
/// treats that valid state as an error and leaves a `.rej` file behind. Probe
/// both directions first: forward means apply, reverse means already applied,
/// and neither means the pinned source drifted and must fail closed.
fn apply_patch_if_needed(patch_file: &Path, patched_source_dir: &Path) -> Result<()> {
    let mut forward = Command::new("patch");
    forward
        .args(["--batch", "--forward", "--dry-run", "-p1", "-i"])
        .arg(patch_file)
        .current_dir(patched_source_dir);
    if forward.status()?.success() {
        return run_command(
            Command::new("patch")
                .args(["--batch", "--forward", "-p1", "-i"])
                .arg(patch_file)
                .current_dir(patched_source_dir),
        );
    }

    let mut reverse = Command::new("patch");
    reverse
        .args(["--batch", "--reverse", "--dry-run", "-p1", "-i"])
        .arg(patch_file)
        .current_dir(patched_source_dir);
    if reverse.status()?.success() {
        return Ok(());
    }

    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(patch_file)
            .current_dir(patched_source_dir),
    )
}
