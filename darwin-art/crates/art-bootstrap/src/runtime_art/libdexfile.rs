use super::*;

/// The pinned AOSP libdexfile provider archive the runtime links.
///
/// The final runtime dylib force-loads it (libunwindstack's dex adapter
/// reaches the ADexFile ABI only indirectly), so it is a production output
/// under `_build/libdexfile-foundation`, built against the production
/// foundation headers, not a probe build artifact.
pub(crate) fn build_libdexfile(root: &Path) -> Result<PathBuf> {
    build_foundation(root)?;
    build_libdexfile_archive(
        root,
        &root.join("_build/libdexfile-foundation"),
        &root.join("_build/foundation"),
        false,
    )
}

/// Build `libdexfile-darwin.a` into `build_dir` against the patched
/// libartbase headers under `foundation`. `static_lib` selects libdexfile's
/// static variant (`-DSTATIC_LIB`: palette tracing disabled), which build
/// tools use; the runtime links the default variant, as libart does.
pub(crate) fn build_libdexfile_archive(
    root: &Path,
    build_dir: &Path,
    foundation: &Path,
    static_lib: bool,
) -> Result<PathBuf> {
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(&object_dir)?;
    fs::create_dir_all(build_dir.join("generated"))?;
    let includes = libdexfile_includes(root, foundation);
    let includes = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let libdexfile = root.join("_aosp/art/libdexfile");
    if !libdexfile.join("Android.bp").exists() {
        return Err("libdexfile sources are missing; run `art-bootstrap sync` first".into());
    }
    let dex_operator_source = build_dir.join("generated/dexfile_operator_out.cc");
    generate_operator_source(
        root,
        &libdexfile,
        &[
            "dex/dex_file.h",
            "dex/dex_file_layout.h",
            "dex/dex_instruction.h",
            "dex/dex_instruction_utils.h",
            "dex/invoke_type.h",
        ],
        &dex_operator_source,
    )?;
    let dex_sources = [
        dex_operator_source,
        // libunwindstack's AOSP dex adapter consumes the public ADexFile C
        // ABI. Keep the external implementation in the same provider archive
        // instead of relying on an accidental host symbol.
        libdexfile.join("external/dex_file_ext.cc"),
        libdexfile.join("dex/dex_file.cc"),
        libdexfile.join("dex/dex_file_loader.cc"),
        libdexfile.join("dex/standard_dex_file.cc"),
        libdexfile.join("dex/compact_dex_file.cc"),
        libdexfile.join("dex/compact_offset_table.cc"),
        libdexfile.join("dex/dex_file_verifier.cc"),
        libdexfile.join("dex/dex_file_exception_helpers.cc"),
        libdexfile.join("dex/dex_file_layout.cc"),
        libdexfile.join("dex/dex_file_tracking_registrar.cc"),
        libdexfile.join("dex/dex_instruction.cc"),
        libdexfile.join("dex/descriptors_names.cc"),
        libdexfile.join("dex/modifiers.cc"),
        libdexfile.join("dex/primitive.cc"),
        libdexfile.join("dex/signature.cc"),
        libdexfile.join("dex/type_lookup_table.cc"),
        libdexfile.join("dex/utf.cc"),
    ];
    let compiler_identity = command_output(
        Command::new(crate::support::support_build_tool("CLANG", "clang++")).arg("--version"),
    )?;
    let mut hash_cache = FileHashCache::default();
    let mut compiled = 0;
    let mut dex_objects = Vec::new();
    for source in dex_sources {
        let object = object_dir.join(format!(
            "{}.o",
            source
                .file_name()
                .ok_or("missing source name")?
                .to_string_lossy()
        ));
        let mut command = common_cpp_command(&includes);
        if static_lib {
            command.arg("-DSTATIC_LIB");
        }
        command.arg("-c").arg(&source).arg("-o").arg(&object);
        if compile_with_dependency_cache(
            &mut command,
            &object,
            &compiler_identity,
            &mut hash_cache,
        )? {
            compiled += 1;
        }
        dex_objects.push(object);
    }
    let dex_archive = build_dir.join("libdexfile-darwin.a");
    create_archive_if_needed(&dex_archive, &dex_objects, compiled)?;
    let mut depfiles = String::new();
    for object in &dex_objects {
        depfiles.push_str(&format!("{}\n", object.with_extension("o.d").display()));
    }
    fs::write(build_dir.join("current-depfiles.txt"), depfiles)?;
    Ok(dex_archive)
}

/// Include roots for compiling libdexfile and its clients against the
/// patched libartbase under `foundation`.
pub(crate) fn libdexfile_includes(root: &Path, foundation: &Path) -> Vec<PathBuf> {
    let libdexfile = root.join("_aosp/art/libdexfile");
    let jni_include = PathBuf::from("/opt/homebrew/opt/openjdk@17/include");
    vec![
        foundation.join("patched-source/libartbase"),
        root.join("_aosp/art/libartbase"),
        libdexfile.clone(),
        libdexfile.join("external/include"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/system/libziparchive/include"),
        root.join("_aosp/art/libartpalette/include"),
        PathBuf::from("/opt/homebrew/include"),
        jni_include.clone(),
        jni_include.join("darwin"),
    ]
}
