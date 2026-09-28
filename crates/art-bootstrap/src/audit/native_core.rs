//! Narrow product-owned core compile edge, independent of test producers and
//! expensive platform archive audits. Final runtime linkage remains its own gate.
use super::graphics_core_probes::{compile_runtime_core_objects, core_probe_includes};
use super::*;

pub(crate) fn build_runtime_native_core(root: &Path) -> Result<()> {
    prepare_runtime_shadow(root)?;
    let paths = BuildPaths::from_root(root);
    let output = paths.native_output("native-runtime/core");
    fs::create_dir_all(&output)?;
    let includes = core_probe_includes(root, &paths, &root.join("_aosp/art/runtime"));
    let refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let identity = command_output(Command::new("clang++").arg("--version"))?;
    compile_runtime_core_objects(
        root,
        &output,
        &refs,
        &output.join("core-probe-hashes.cache"),
        &identity,
    )?;
    println!("product-native-core: PASS output={}", output.display());
    Ok(())
}
