use super::*;
use crate::native_build::PendingNativeCompile;

/// Build the small bootstrap-owned object set that is not part of the shared
/// ART runtime or compat adapter phases. These jobs are independent and are
/// executed by the same dependency-fingerprinted worker pool as the larger
/// phases.
pub(super) fn bootstrap_jobs(
    staged: &RuntimeBootstrapStaging,
    real_graphics: bool,
    includes: &[&Path],
    runtime_includes: &[&Path],
) -> Vec<PendingNativeCompile> {
    let mut jobs = Vec::new();

    // The Darwin MAP_JIT implementation is a small compat TU rather than an
    // upstream ART source. Keep it in the runtime bootstrap archive so the
    // patched JIT headers can resolve their thread-local write-scope helpers.
    #[cfg(target_os = "macos")]
    {
        let jit_memory_object = staged
            .runtime_core_object_dir
            .join("darwin_jit_memory.cc.o");
        let mut jit_memory_command = runtime_bootstrap_cpp_command(runtime_includes);
        jit_memory_command
            .arg("-c")
            .arg(staged.root.join("compat/darwin_jit_memory.cc"))
            .arg("-o")
            .arg(&jit_memory_object);
        jobs.push(PendingNativeCompile {
            command: jit_memory_command,
            object: jit_memory_object,
        });

        let resolver_object = staged
            .runtime_core_object_dir
            .join("aim_stack_resolver.cc.o");
        let mut resolver_command = runtime_bootstrap_cpp_command(runtime_includes);
        resolver_command
            .arg("-c")
            .arg(staged.root.join("compat/aim_stack_resolver.cc"))
            .arg("-o")
            .arg(&resolver_object);
        jobs.push(PendingNativeCompile {
            command: resolver_command,
            object: resolver_object,
        });
    }

    let operator_object = staged.object_dir.join("generated_operator_out.cc.o");
    let mut operator_command = runtime_bootstrap_cpp_command(runtime_includes);
    operator_command
        .arg("-idirafter")
        .arg(&staged.ndk_arch_include)
        .arg("-idirafter")
        .arg(&staged.ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-c")
        .arg(&staged.operator_source)
        .arg("-o")
        .arg(&operator_object);
    jobs.push(PendingNativeCompile {
        command: operator_command,
        object: operator_object,
    });

    for profile_source in [
        "profile/profile_boot_info.cc",
        "profile/profile_compilation_info.cc",
    ] {
        let profile_object = staged
            .object_dir
            .join(format!("libprofile_{}.o", profile_source.replace('/', "_")));
        let mut profile_command = runtime_bootstrap_cpp_command(runtime_includes);
        profile_command
            .arg("-idirafter")
            .arg(&staged.ndk_arch_include)
            .arg("-idirafter")
            .arg(&staged.ndk_include)
            .arg("-Wno-macro-redefined")
            .arg("-c")
            .arg(staged.libprofile.join(profile_source))
            .arg("-o")
            .arg(&profile_object);
        jobs.push(PendingNativeCompile {
            command: profile_command,
            object: profile_object,
        });
    }

    if real_graphics {
        let os_linux_object = staged.object_dir.join("artbase_os_linux_aosp_fmt.cc.o");
        let mut os_linux_command = runtime_cpp_command(includes);
        os_linux_command
            .arg("-I")
            .arg(staged.root.join("_aosp/art/libartbase/base"))
            .arg("-c")
            .arg(
                staged
                    .root
                    .join("_build/foundation/patched-source/libartbase/base/os_linux.cc"),
            )
            .arg("-o")
            .arg(&os_linux_object);
        jobs.push(PendingNativeCompile {
            command: os_linux_command,
            object: os_linux_object,
        });
    }

    let openjdk_math_object = staged.object_dir.join("libcore_openjdk_Math.c.o");
    let mut openjdk_math_command = Command::new("clang");
    openjdk_math_command
        .args([
            "-std=gnu11",
            "-O2",
            "-DNDEBUG",
            "-ftrivial-auto-var-init=zero",
            "-ffunction-sections",
            "-fdata-sections",
        ])
        .arg(format!("-I{}", staged.android_jni_include.display()))
        .arg(format!("-I{}", staged.nativehelper_full_include.display()))
        .arg(format!(
            "-I{}",
            staged.nativehelper_platform_headers.display()
        ))
        .arg(format!("-I{}", staged.liblog_include.display()))
        .arg("-c")
        .arg(
            staged
                .root
                .join("_aosp/libcore/ojluni/src/main/native/Math.c"),
        )
        .arg("-o")
        .arg(&openjdk_math_object);
    jobs.push(PendingNativeCompile {
        command: openjdk_math_command,
        object: openjdk_math_object,
    });

    let jni_proxy_object = staged.object_dir.join("aim_jni_proxy.c.o");
    let mut jni_proxy_command = Command::new("clang");
    jni_proxy_command
        .args([
            "-std=c17",
            "-O2",
            "-DNDEBUG",
            "-fvisibility=hidden",
            "-ffunction-sections",
            "-fdata-sections",
            "-Wall",
            "-Wextra",
            "-Werror",
        ])
        .arg(format!("-I{}", staged.jni_proxy_include.display()))
        .arg(format!("-I{}", staged.jni_proxy_generated.display()))
        .arg("-c")
        .arg(staged.root.join("tools/android-jni-proxy/src/proxy.c"))
        .arg("-o")
        .arg(&jni_proxy_object);
    jobs.push(PendingNativeCompile {
        command: jni_proxy_command,
        object: jni_proxy_object,
    });
    let jni_proxy_call_object = staged.object_dir.join("aim_jni_proxy_call.S.o");
    let mut jni_proxy_call_command = Command::new("clang");
    jni_proxy_call_command
        .args(["-arch", "arm64", "-c"])
        .arg(
            staged
                .root
                .join("tools/android-jni-proxy/src/aapcs64_call.S"),
        )
        .arg("-o")
        .arg(&jni_proxy_call_object);
    jobs.push(PendingNativeCompile {
        command: jni_proxy_call_command,
        object: jni_proxy_call_object,
    });
    jobs
}
