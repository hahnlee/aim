use super::*;

pub(crate) fn build_elf_jni_dex_probe(root: &Path) -> Result<()> {
    let baseline_dex = root.join("_build/dex-probe/dex/classes.dex");
    let dex_probe = root.join("_build/dex-probe/dex-probe");
    for input in [&baseline_dex, &dex_probe] {
        if !input.is_file() {
            return Err(format!(
                "ELF JNI DEX baseline is missing: {}; run `build-dex` first",
                input.display()
            )
            .into());
        }
    }
    let build_dir = root.join("_build/elf-jni-dex");
    let class_dir = build_dir.join("classes");
    let dex_dir = build_dir.join("dex");
    fs::create_dir_all(&class_dir)?;
    fs::create_dir_all(&dex_dir)?;
    run_command(
        Command::new("javac")
            .args(["--release", "8", "-encoding", "UTF-8", "-d"])
            .arg(&class_dir)
            .arg(root.join("probes/android-elf-jni-fixture/NativeFixture.java")),
    )?;
    let classes_dex = dex_dir.join("classes.dex");
    if classes_dex.is_file() {
        fs::remove_file(&classes_dex)?;
    }
    run_command(
        Command::new(find_d8()?)
            .arg("--lib")
            .arg(find_android_platform_jar()?)
            .arg("--output")
            .arg(&dex_dir)
            .arg(&baseline_dex)
            .arg(class_dir.join("darwin/art/nativefixture/NativeFixture.class")),
    )?;
    let output = command_output(Command::new(&dex_probe).arg(&classes_dex))?;
    let inventory = output.trim();
    let required = [
        "verified=yes",
        "version=38",
        "class[0]=",
        "Ldarwin/art/nativefixture/NativeFixture;",
        "Ldev/aim/probe/Hello;",
        "Ldev/aim/probe/ProbeActivity;",
        "Ldev/aim/probe/ProbeView;",
        "Ldev/aim/probe/ProbeXmlResourceParser;",
    ];
    if !inventory.starts_with("AOSP DEX: ")
        || required.iter().any(|needle| !inventory.contains(needle))
    {
        return Err(format!("unexpected ELF JNI DEX probe output: {output:?}").into());
    }
    println!("build-elf-jni-dex: {inventory}");
    Ok(())
}
