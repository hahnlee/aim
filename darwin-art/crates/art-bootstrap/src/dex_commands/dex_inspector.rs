use super::*;

// Build-tool ownership: pinned AOSP verifier, independent of fixture Java/DEX
// compilation. No production module reads a dex-probe output to use this tool.
pub(crate) fn build_dex_inspector(root: &Path) -> Result<PathBuf> {
    let build_dir = root.join("_build/dex-inspector");
    let foundation = build_dir.join("foundation");
    build_foundation_archives_at(root, &foundation)?;
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(&object_dir)?;
    fs::create_dir_all(build_dir.join("generated"))?;
    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = foundation.join("patched-source/libartbase");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let libdexfile_external_include = libdexfile.join("external/include");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let libziparchive_include = root.join("_aosp/system/libziparchive/include");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let java_home = PathBuf::from("/opt/homebrew/opt/openjdk@17");
    let jni_include = java_home.join("include");
    let jni_darwin_include = jni_include.join("darwin");
    let includes = [
        patched_artbase.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        libdexfile_external_include.as_path(),
        libbase_include.as_path(),
        libziparchive_include.as_path(),
        palette_include.as_path(),
        Path::new("/opt/homebrew/include"),
        jni_include.as_path(),
        jni_darwin_include.as_path(),
    ];
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
    let mut dex_objects = Vec::new();
    let compiler_identity = command_output(
        Command::new(crate::support::support_build_tool("CLANG", "clang++")).arg("--version"),
    )?;
    let mut hash_cache = FileHashCache::default();
    let mut compiled = 0;
    for source in dex_sources {
        let object = object_dir.join(format!(
            "{}.o",
            source
                .file_name()
                .ok_or("missing source name")?
                .to_string_lossy()
        ));
        let mut command = common_cpp_command(&includes);
        // Pinned AOSP libdexfile's static variant disables palette tracing;
        // use that upstream build contract, not runtime success stubs.
        command
            .arg("-DSTATIC_LIB")
            .arg("-c")
            .arg(&source)
            .arg("-o")
            .arg(&object);
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

    let inspector = build_dir.join("dex-inspect");
    let inspector_object = object_dir.join("dex-inspect.cc.o");
    compile_with_dependency_cache(
        common_cpp_command(&includes)
            .arg("-DSTATIC_LIB")
            .arg("-c")
            .arg(root.join("tools/dex-inspect.cc"))
            .arg("-o")
            .arg(&inspector_object),
        &inspector_object,
        &compiler_identity,
        &mut hash_cache,
    )?;
    let linked = link_with_cache(
        common_cpp_command(&includes)
            .arg(&inspector_object)
            .arg(&dex_archive)
            .arg(foundation.join("libartbase-darwin.a"))
            .arg(foundation.join("libandroid-base-darwin.a"))
            .arg(foundation.join("libziparchive-darwin.a"))
            .args(["-Wl,-dead_strip", "-lz", "-o"])
            .arg(&inspector),
        &inspector,
        &build_dir.join("link.fingerprint"),
    )?;
    if !linked.status.success() {
        return Err(format!(
            "DEX inspector link failed: {}",
            String::from_utf8_lossy(&linked.stderr)
        )
        .into());
    }
    let mut depfiles = fs::read_to_string(foundation.join("current-depfiles.txt"))?;
    for object in dex_objects.iter().chain(std::iter::once(&inspector_object)) {
        depfiles.push_str(&format!("{}\n", object.with_extension("o.d").display()));
    }
    fs::write(build_dir.join("current-depfiles.txt"), depfiles)?;
    Ok(inspector)
}
