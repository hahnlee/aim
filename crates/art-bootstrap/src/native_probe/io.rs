use super::*;

pub(crate) fn compile_runtime_filesystem_probe(root: &Path, build_dir: &Path) -> Result<PathBuf> {
    let object = build_dir.join("aim_runtime_filesystem_probe.cc.o");
    fs::create_dir_all(build_dir)?;
    let cache_path = build_dir.join("runtime-probe-file-hashes.cache");
    let mut cache = FileHashCache::load(&cache_path)?;
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = Command::new("clang++");
    command
        .args(["-std=c++20", "-fPIC", "-Wall", "-Wextra", "-c"])
        .arg(root.join("probes/runtime_filesystem_probe.cc"))
        .arg("-I")
        .arg(root.join("probes"))
        .arg("-I")
        .arg(root.join("tools/bionic-fs-facade/include"))
        .arg("-I")
        .arg(root.join("tools/bionic-ioctl-facade/include"))
        .arg("-o")
        .arg(&object);
    let _ = compile_with_dependency_cache(&mut command, &object, &compiler_identity, &mut cache)?;
    cache.save(&cache_path)?;
    Ok(object)
}

pub(crate) fn compile_runtime_network_probe(root: &Path, build_dir: &Path) -> Result<PathBuf> {
    let object = build_dir.join("aim_runtime_network_probe.cc.o");
    let server_object = build_dir.join("aim_runtime_network_server.cc.o");
    let phase_object = build_dir.join("aim_runtime_network_phase.cc.o");
    fs::create_dir_all(build_dir)?;
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    for (source, output, cache_name) in [
        (
            "runtime_network_probe.cc",
            &server_object,
            "runtime-probe-network-server-hashes.cache",
        ),
        (
            "runtime_acceptance_phases.cc",
            &phase_object,
            "runtime-probe-network-phase-hashes.cache",
        ),
    ] {
        let cache_path = build_dir.join(cache_name);
        let mut cache = FileHashCache::load(&cache_path)?;
        let mut command = Command::new("clang++");
        command
            .args(["-std=c++20", "-fPIC", "-Wall", "-Wextra", "-c"])
            .arg(root.join("probes").join(source))
            .arg("-I")
            .arg(root.join("probes"))
            .arg("-I")
            .arg(root.join("_aosp/libnativehelper/include_jni"))
            .arg("-o")
            .arg(output);
        let _ =
            compile_with_dependency_cache(&mut command, output, &compiler_identity, &mut cache)?;
        cache.save(&cache_path)?;
    }
    run_command(
        Command::new("clang++")
            .args(["-r"])
            .arg(&server_object)
            .arg(&phase_object)
            .arg("-o")
            .arg(&object),
    )?;
    Ok(object)
}

pub(crate) fn build_runtime_network_probe(root: &Path) -> Result<()> {
    let output = env::var_os("AIM_NATIVE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("_build/runtime-link-probe/aim_runtime_network_probe.cc.o"));
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("native probe output has no parent: {}", output.display()))?;
    let object = compile_runtime_network_probe(root, build_dir)?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-network-probe: {}", output.display());
    Ok(())
}

pub(crate) fn build_runtime_filesystem_probe(root: &Path) -> Result<()> {
    let output = env::var_os("AIM_NATIVE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/aim_runtime_filesystem_probe.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("native probe output has no parent: {}", output.display()))?;
    let object = compile_runtime_filesystem_probe(root, build_dir)?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-filesystem-probe: {}", output.display());
    Ok(())
}
