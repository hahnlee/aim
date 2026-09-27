use super::*;

// Build-tool ownership: pinned AOSP verifier, independent of fixture Java/DEX
// compilation. No production module reads a dex-probe output to use this tool.
pub(crate) fn build_dex_inspector(root: &Path) -> Result<PathBuf> {
    let build_dir = root.join("_build/dex-inspector");
    let foundation = build_dir.join("foundation");
    build_foundation_archives_at(root, &foundation)?;
    let object_dir = build_dir.join("objects");
    // Pinned AOSP libdexfile's static variant disables palette tracing;
    // use that upstream build contract, not runtime success stubs.
    let dex_archive = build_libdexfile_archive(root, &build_dir, &foundation, true)?;
    let includes = libdexfile_includes(root, &foundation);
    let includes = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let compiler_identity = command_output(
        Command::new(crate::support::support_build_tool("CLANG", "clang++")).arg("--version"),
    )?;
    let mut hash_cache = FileHashCache::default();

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
    depfiles.push_str(&fs::read_to_string(build_dir.join("current-depfiles.txt"))?);
    depfiles.push_str(&format!(
        "{}\n",
        inspector_object.with_extension("o.d").display()
    ));
    fs::write(build_dir.join("current-depfiles.txt"), depfiles)?;
    Ok(inspector)
}
