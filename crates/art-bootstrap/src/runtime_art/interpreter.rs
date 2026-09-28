use super::*;

pub(crate) fn build_interpreter_core(root: &Path) -> Result<()> {
    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let libdexfile = root.join("_aosp/art/libdexfile");
    // Same-directory quoted headers must resolve through the sole canonical
    // ART overlay, including the interpreter-specific patches in its manifest.
    let runtime = runtime_bootstrap::prepare_runtime_shadow(root)?;
    let runtime_base = runtime.join("base");
    let runtime_arm64 = runtime.join("arch/arm64");
    let generator = root.join("_aosp/art/tools/cpp-define-generator");
    let generated_dir = root.join("_build/runtime-arm64/generated");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    let android_jni_include = root.join("_aosp/libnativehelper/include_jni");
    let nativehelper_headers = root.join("_aosp/libnativehelper/header_only_include");
    let dlmalloc = root.join("_aosp/external/dlmalloc");
    if !generated_dir.join("asm_defines.h").exists() {
        return Err(
            "generated ART ABI constants are missing; run `build-runtime-arm64` first".into(),
        );
    }

    let includes = [
        generated_dir.as_path(),
        generator.as_path(),
        patched_artbase.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        runtime.as_path(),
        runtime_base.as_path(),
        runtime_arm64.as_path(),
        palette_include.as_path(),
        libbase_include.as_path(),
        tinyxml2.as_path(),
        android_jni_include.as_path(),
        nativehelper_headers.as_path(),
        dlmalloc.as_path(),
        Path::new("/opt/homebrew/include"),
    ];
    let build_dir = root.join("_build/interpreter-core");
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(&object_dir)?;
    let sources = [
        runtime.join("interpreter/interpreter.cc"),
        runtime.join("interpreter/interpreter_cache.cc"),
        runtime.join("interpreter/interpreter_common.cc"),
        runtime.join("interpreter/interpreter_switch_impl0.cc"),
        runtime.join("interpreter/lock_count_data.cc"),
        runtime.join("interpreter/shadow_frame.cc"),
        runtime.join("interpreter/unstarted_runtime.cc"),
    ];
    let cache = build_dir.join("interpreter-objects.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut objects = Vec::new();
    let mut compiled = 0usize;
    for source in sources {
        let file_name = source
            .file_name()
            .ok_or_else(|| format!("source has no file name: {}", source.display()))?;
        let object = object_dir.join(format!("{}.o", file_name.to_string_lossy()));
        let mut command = runtime_cpp_command(&includes);
        command
            .arg(format!("-I{}", runtime.join("interpreter").display()))
            .args(["-include", "mirror/object_reference.h"])
            .arg("-c")
            .arg(&source)
            .arg("-o")
            .arg(&object);
        if compile_cached_probe_tu(&mut command, &object, &cache, &compiler_identity)? {
            compiled += 1;
        }
        let kind = command_output(Command::new("file").arg(&object))?;
        if !kind.contains("Mach-O 64-bit object arm64") {
            return Err(format!("unexpected interpreter object format: {kind}").into());
        }
        objects.push(object);
    }

    let archive = build_dir.join("libart-interpreter-darwin.a");
    if compiled != 0 || !archive.is_file() {
        create_archive(&archive, &objects)?;
    }
    println!(
        "build-interpreter-core: AOSP C++ interpreter Mach-O objects={} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(())
}
