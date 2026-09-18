//! Narrow framework JavaVM publication provider, independent of graphics and
//! the upstream HostRuntime/resource audit. The product has one backing store.
use super::*;

pub(crate) fn build_framework_java_vm_provider(root: &Path) -> Result<PathBuf> {
    let lock = fs::read_to_string(root.join("upstream/android16-android-runtime-host.lock"))?;
    for (source, key) in [
        (
            "platform/darwin/android_runtime_host.cc",
            "DARWIN_ANDROID_RUNTIME_HOST_CPP_SHA256",
        ),
        (
            "include/darwin_art/android_runtime_host.h",
            "DARWIN_ANDROID_RUNTIME_HOST_H_SHA256",
        ),
        (
            "_aosp/frameworks/base/core/jni/include/android_runtime/AndroidRuntime.h",
            "ANDROID_RUNTIME_H_SHA256",
        ),
    ] {
        let expected = lock
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .ok_or_else(|| format!("missing JavaVM provider pin {key}"))?;
        let actual = format!("{:x}", Sha256::digest(fs::read(root.join(source))?));
        if actual != expected {
            return Err(format!("JavaVM provider checksum mismatch: {source}").into());
        }
    }
    let output = root.join("_build/android-runtime-host");
    fs::create_dir_all(&output)?;
    let includes = [
        "include",
        "_aosp/frameworks/base/core/jni/include",
        "_aosp/system/core/libutils/include",
        "_aosp/system/core/libsystem/include",
        "_aosp/system/logging/liblog/include",
        "_aosp/system/libbase/include",
        "_aosp/libnativehelper-full/include_jni",
    ]
    .map(|path| root.join(path));
    let refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let object = output.join("android_runtime_vm_provider.o");
    // This provider does not consume ART's object/reference ABI. In particular
    // do not inject ART globals or compressed-reference compiler definitions.
    let mut command = Command::new("clang++");
    command.args(["-std=c++20", "-arch", "arm64", "-O2", "-DNDEBUG"]);
    for include in refs {
        command.arg("-I").arg(include);
    }
    command
        .args([
            "-fPIC",
            "-fno-rtti",
            "-fvisibility=hidden",
            "-DANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION",
        ])
        .arg("-c")
        .arg(root.join("platform/darwin/android_runtime_host.cc"))
        .arg("-o")
        .arg(&object);
    let identity = command_output(Command::new("clang++").arg("--version"))?;
    let compiled = compile_cached_probe_tu(
        &mut command,
        &object,
        &output.join("java-vm-provider.cache"),
        &identity,
    )?;
    let library = output.join("libandroid-runtime-darwin-host.a");
    if compiled
        || !library.is_file()
        || fs::metadata(&object)?.modified()? > fs::metadata(&library)?.modified()?
    {
        let candidate = output.join("libandroid-runtime-darwin-host.candidate.a");
        create_archive(&candidate, &[object])?;
        fs::rename(candidate, &library)?;
    }
    println!("framework-java-vm-provider: PASS compiled={compiled} graphics-archives=0");
    Ok(library)
}
