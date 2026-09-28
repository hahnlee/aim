use super::*;

pub(crate) fn build_network_dex_probe(root: &Path) -> Result<()> {
    let baseline_dex = root.join("_build/dex-probe/dex/classes.dex");
    let dex_probe = root.join("_build/dex-probe/dex-probe");
    for input in [&baseline_dex, &dex_probe] {
        if !input.is_file() {
            return Err(format!(
                "network DEX baseline is missing: {}; run `build-dex` first",
                input.display()
            )
            .into());
        }
    }
    let tool = root.join("tools/bionic-network-runtime-integration");
    let build_dir = root.join("_build/network-runtime-probe");
    let class_dir = build_dir.join("classes");
    let dex_dir = build_dir.join("dex");
    fs::create_dir_all(&class_dir)?;
    fs::create_dir_all(&dex_dir)?;
    run_command(
        Command::new("javac")
            .args(["--release", "8", "-encoding", "UTF-8", "-d"])
            .arg(&class_dir)
            .arg("-classpath")
            .arg(find_android_platform_jar()?)
            .arg(tool.join("probes/NetworkRuntimeFixture.java"))
            .arg(root.join("probes/button/ProbeAnimationHost.java")),
    )?;
    let classes_dex = dex_dir.join("classes.dex");
    if classes_dex.is_file() {
        fs::remove_file(&classes_dex)?;
    }
    run_command(
        Command::new(find_d8()?)
            .arg("--lib")
            .arg(find_android_platform_jar()?)
            .arg("--min-api")
            .arg("35")
            .arg("--output")
            .arg(&dex_dir)
            .arg(&baseline_dex)
            .arg(class_dir.join("dev/aim/probe/NetworkRuntimeFixture.class"))
            .arg(class_dir.join("dev/aim/probe/ProbeAnimationHost.class"))
            .arg(class_dir.join("dev/aim/probe/ProbeAnimationHost$1.class")),
    )?;
    let output = command_output(Command::new(&dex_probe).arg(&classes_dex))?;
    // The baseline DEX plus the fixture and its animation host (d8 may add
    // synthetic lambda classes for the latter).
    let baseline_output = command_output(Command::new(&dex_probe).arg(&baseline_dex))?;
    let baseline_classes = dex_class_count(&baseline_output)?;
    if dex_class_count(&output)? < baseline_classes + 3
        || !output.contains("Ldev/aim/probe/NetworkRuntimeFixture;")
        || !output.contains("Ldev/aim/probe/ProbeAnimationHost;")
    {
        return Err(format!("unexpected network DEX probe output: {output:?}").into());
    }

    let fixture = build_dir.join("libaim_network_runtime.so");
    let clang = pinned_direct_apk_ndk_bin()?.join("aarch64-linux-android35-clang");
    run_command(
        Command::new(clang)
            .args([
                "-std=c17",
                "-O2",
                "-fno-builtin",
                "-fPIC",
                "-fno-stack-protector",
                "-U_FORTIFY_SOURCE",
                "-D_FORTIFY_SOURCE=0",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-shared",
                "-nostdlib",
                "-fuse-ld=lld",
                "-Wl,--build-id=none",
                "-Wl,--hash-style=sysv",
                "-Wl,-z,now",
                "-Wl,-z,norelro",
                "-Wl,-z,max-page-size=16384",
                "-Wl,-soname,libaim_network_runtime.so",
            ])
            .arg(format!(
                "-Wl,--version-script,{}",
                tool.join("probes/exports.map").display()
            ))
            .arg(tool.join("probes/network_jni.c"))
            .arg("-lc")
            .arg("-o")
            .arg(&fixture),
    )?;
    let kind = command_output(Command::new("file").arg(&fixture))?;
    if !kind.contains("ELF 64-bit LSB shared object, ARM aarch64") {
        return Err(format!("unexpected network fixture format: {kind}").into());
    }
    println!(
        "build-network-dex: classes={} ELF=arm64 imports=8 loopback=only",
        dex_class_count(&output)?
    );
    Ok(())
}
