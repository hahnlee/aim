use super::*;
use std::collections::BTreeMap;

fn sync_jit_sources_locked(root: &Path, lock: &BTreeMap<String, String>) -> Result<()> {
    let art_revision = lock_value(lock, "ART_REVISION")?;
    let compiler_revision = lock_value(lock, "ART_COMPILER_REVISION")?;
    if compiler_revision != art_revision {
        return Err(format!(
            "ART compiler revision {compiler_revision} does not match ART revision {art_revision}"
        )
        .into());
    }
    materialize_archive(
        root,
        "platform/art",
        compiler_revision,
        "art-compiler",
        "compiler",
        "_aosp/art/compiler",
        "Android.bp",
        lock_value(lock, "ART_COMPILER_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        compiler_revision,
        "art-dex2oat",
        "dex2oat",
        "_aosp/art/dex2oat",
        "Android.bp",
        lock_value(lock, "ART_DEX2OAT_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        compiler_revision,
        "art-profman",
        "profman",
        "_aosp/art/profman",
        "Android.bp",
        lock_value(lock, "ART_PROFMAN_ANDROID_BP_SHA256")?,
    )?;

    materialize_archive(
        root,
        "platform/art",
        compiler_revision,
        "art-disassembler",
        "disassembler",
        "_aosp/art/disassembler",
        "Android.bp",
        lock_value(lock, "ART_DISASSEMBLER_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        compiler_revision,
        "art-tests",
        "test",
        "_aosp/art/test",
        "Android.bp",
        lock_value(lock, "ART_TEST_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/vixl",
        lock_value(lock, "VIXL_REVISION")?,
        "external-vixl",
        "",
        "_aosp/external/vixl",
        "Android.bp",
        lock_value(lock, "VIXL_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/lzma",
        lock_value(lock, "LZMA_REVISION")?,
        "external-lzma",
        "",
        "_aosp/external/lzma",
        "Android.bp",
        lock_value(lock, "LZMA_ANDROID_BP_SHA256")?,
    )?;
    Ok(())
}

/// Materialize only the ART JIT compiler and its pinned VIXL assembler source.
/// This is intentionally separate from the full source sync because the
/// compiler closure is large and can be fetched independently by JIT work.
pub(crate) fn sync_jit_sources(root: &Path) -> Result<()> {
    let lock = read_lock(root)?;
    sync_jit_sources_locked(root, &lock)
}

pub(crate) fn sync_sources(root: &Path) -> Result<()> {
    let lock = read_lock(root)?;
    let revision = lock_value(&lock, "ART_REVISION")?;
    materialize_archive(
        root,
        "platform/external/skia",
        lock_value(&lock, "SKIA_REVISION")?,
        "external-skia",
        "",
        "_aosp/external/skia",
        "BUILD.gn",
        lock_value(&lock, "SKIA_BUILD_GN_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/system/core",
        lock_value(&lock, "SYSTEM_CORE_REVISION")?,
        "system-core-libcutils",
        "libcutils",
        "_aosp/system/core/libcutils",
        "Android.bp",
        lock_value(&lock, "LIBCUTILS_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/system/core",
        lock_value(&lock, "SYSTEM_CORE_REVISION")?,
        "system-core-libutils",
        "libutils",
        "_aosp/system/core/libutils",
        "Android.bp",
        lock_value(&lock, "LIBUTILS_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/system/core",
        lock_value(&lock, "SYSTEM_CORE_REVISION")?,
        "system-core-libsystem",
        "libsystem",
        "_aosp/system/core/libsystem",
        "Android.bp",
        lock_value(&lock, "LIBSYSTEM_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/system/logging",
        lock_value(&lock, "SYSTEM_LOGGING_REVISION")?,
        "system-logging-liblog",
        "liblog",
        "_aosp/system/logging/liblog",
        "Android.bp",
        lock_value(&lock, "LIBLOG_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/libnativehelper",
        lock_value(&lock, "LIBNATIVEHELPER_REVISION")?,
        "libnativehelper-full",
        "",
        "_aosp/libnativehelper-full",
        "Android.bp",
        lock_value(&lock, "LIBNATIVEHELPER_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/libcore",
        lock_value(&lock, "LIBCORE_REVISION")?,
        "ojluni/src/main/native/Android.bp",
        "_aosp/libcore/ojluni/src/main/native/Android.bp",
        lock_value(&lock, "LIBCORE_OPENJDK_NATIVE_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/libcore",
        lock_value(&lock, "LIBCORE_REVISION")?,
        "ojluni/src/main/native/Math.c",
        "_aosp/libcore/ojluni/src/main/native/Math.c",
        lock_value(&lock, "LIBCORE_OPENJDK_MATH_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/frameworks/base",
        lock_value(&lock, "FRAMEWORKS_BASE_REVISION")?,
        "frameworks-base-nativehelper-jvm",
        "libs/nativehelper_jvm",
        "_aosp/frameworks/base/libs/nativehelper_jvm",
        "Android.bp",
        lock_value(&lock, "NATIVEHELPER_JVM_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/frameworks/native",
        lock_value(&lock, "FRAMEWORKS_NATIVE_REVISION")?,
        "frameworks-native-ui",
        "libs/ui",
        "_aosp/frameworks/native/libs/ui",
        "Android.bp",
        lock_value(&lock, "FRAMEWORKS_NATIVE_UI_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/frameworks/native",
        lock_value(&lock, "FRAMEWORKS_NATIVE_REVISION")?,
        "frameworks-native-arect",
        "libs/arect",
        "_aosp/frameworks/native/libs/arect",
        "Android.bp",
        lock_value(&lock, "FRAMEWORKS_NATIVE_ARECT_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/frameworks/native",
        lock_value(&lock, "FRAMEWORKS_NATIVE_REVISION")?,
        "frameworks-native-math",
        "libs/math",
        "_aosp/frameworks/native/libs/math",
        "Android.bp",
        lock_value(&lock, "FRAMEWORKS_NATIVE_MATH_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/zlib",
        lock_value(&lock, "ZLIB_REVISION")?,
        "external-zlib",
        "",
        "_aosp/external/zlib",
        "Android.bp",
        lock_value(&lock, "ZLIB_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/libpng",
        lock_value(&lock, "LIBPNG_REVISION")?,
        "external-libpng",
        "",
        "_aosp/external/libpng",
        "Android.bp",
        lock_value(&lock, "LIBPNG_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/freetype",
        lock_value(&lock, "FREETYPE_REVISION")?,
        "external-freetype",
        "",
        "_aosp/external/freetype",
        "Android.bp",
        lock_value(&lock, "FREETYPE_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/external/harfbuzz_ng",
        lock_value(&lock, "HARFBUZZ_NG_REVISION")?,
        "Android.bp",
        "_aosp/external/harfbuzz_ng/Android.bp",
        lock_value(&lock, "HARFBUZZ_NG_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/harfbuzz_ng",
        lock_value(&lock, "HARFBUZZ_NG_REVISION")?,
        "external-harfbuzz-src",
        "src",
        "_aosp/external/harfbuzz_ng/src",
        "hb.h",
        lock_value(&lock, "HARFBUZZ_NG_HB_HEADER_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/frameworks/minikin",
        lock_value(&lock, "MINIKIN_REVISION")?,
        "frameworks-minikin",
        "",
        "_aosp/frameworks/minikin",
        "Android.bp",
        lock_value(&lock, "MINIKIN_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "Android.bp",
        "_aosp/external/icu-graphics/Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_ROOT_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "android_icu4c/Android.bp",
        "_aosp/external/icu-graphics/android_icu4c/Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_ANDROID_ICU4C_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "icu4c/source/Android.bp",
        "_aosp/external/icu-graphics/icu4c/source/Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_SOURCE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "graphics-icu-android-include",
        "android_icu4c/include",
        "_aosp/external/icu-graphics/android_icu4c/include",
        "uconfig_local.h",
        lock_value(&lock, "GRAPHICS_ICU_UCONFIG_LOCAL_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "graphics-icu-common",
        "icu4c/source/common",
        "_aosp/external/icu-graphics/icu4c/source/common",
        "Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_COMMON_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "graphics-icu-i18n",
        "icu4c/source/i18n",
        "_aosp/external/icu-graphics/icu4c/source/i18n",
        "Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_I18N_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "graphics-icu-stubdata",
        "icu4c/source/stubdata",
        "_aosp/external/icu-graphics/icu4c/source/stubdata",
        "Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_STUBDATA_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "graphics-icu-init",
        "libandroidicuinit",
        "_aosp/external/icu-graphics/libandroidicuinit",
        "Android.bp",
        lock_value(&lock, "GRAPHICS_ICU_INIT_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-runtime",
        "runtime",
        "_aosp/art/runtime",
        "Android.bp",
        lock_value(&lock, "ART_RUNTIME_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libartbase",
        "libartbase",
        "_aosp/art/libartbase",
        "Android.bp",
        lock_value(&lock, "ART_LIBARTBASE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libdexfile",
        "libdexfile",
        "_aosp/art/libdexfile",
        "Android.bp",
        lock_value(&lock, "ART_LIBDEXFILE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libprofile",
        "libprofile",
        "_aosp/art/libprofile",
        "Android.bp",
        lock_value(&lock, "ART_LIBPROFILE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-cmdline",
        "cmdline",
        "_aosp/art/cmdline",
        "Android.bp",
        lock_value(&lock, "ART_CMDLINE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libelffile",
        "libelffile",
        "_aosp/art/libelffile",
        "Android.bp",
        lock_value(&lock, "ART_LIBELFFILE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-odrefresh-include",
        "odrefresh/include",
        "_aosp/art/odrefresh/include",
        "odr_statslog/odr_statslog.h",
        lock_value(&lock, "ART_ODR_STATSLOG_HEADER_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-sigchainlib",
        "sigchainlib",
        "_aosp/art/sigchainlib",
        "Android.bp",
        lock_value(&lock, "ART_SIGCHAIN_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libnativebridge-include",
        "libnativebridge/include",
        "_aosp/art/libnativebridge/include",
        "nativebridge/native_bridge.h",
        lock_value(&lock, "ART_NATIVEBRIDGE_HEADER_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libnativeloader-include",
        "libnativeloader/include",
        "_aosp/art/libnativeloader/include",
        "nativeloader/native_loader.h",
        lock_value(&lock, "ART_NATIVELOADER_HEADER_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-libartpalette-include",
        "libartpalette/include",
        "_aosp/art/libartpalette/include",
        "palette/palette.h",
        lock_value(&lock, "ART_PALETTE_HEADER_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/art",
        revision,
        "art-cpp-define-generator",
        "tools/cpp-define-generator",
        "_aosp/art/tools/cpp-define-generator",
        "asm_defines.def",
        lock_value(&lock, "ART_ASM_DEFINES_DEF_SHA256")?,
    )?;
    // The ART module's service-art.jar JNI (libartservice), built into this
    // runtime's ART (ADR 0009).
    for (remote, key) in [
        (
            "libartservice/service/Android.bp",
            "ART_LIBARTSERVICE_ANDROID_BP_SHA256",
        ),
        (
            "libartservice/service/native/service.cc",
            "ART_LIBARTSERVICE_SERVICE_CC_SHA256",
        ),
        (
            "libartservice/service/native/service.h",
            "ART_LIBARTSERVICE_SERVICE_H_SHA256",
        ),
        (
            "libarttools/include/tools/tools.h",
            "ART_LIBARTTOOLS_TOOLS_H_SHA256",
        ),
    ] {
        materialize_file(
            root,
            "platform/art",
            revision,
            remote,
            &format!("_aosp/art/{remote}"),
            lock_value(&lock, key)?,
        )?;
    }
    materialize_file(
        root,
        "platform/system/core",
        lock_value(&lock, "SYSTEM_CORE_REVISION")?,
        "fs_mgr/libfstab/include/fstab/fstab.h",
        "_aosp/system/core/fs_mgr/libfstab/include/fstab/fstab.h",
        lock_value(&lock, "SYSTEM_CORE_FSTAB_H_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/art",
        revision,
        "tools/generate_operator_out.py",
        "_aosp/art/tools/generate_operator_out.py",
        lock_value(&lock, "ART_OPERATOR_GENERATOR_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/system/libbase",
        lock_value(&lock, "LIBBASE_REVISION")?,
        "system-libbase",
        "",
        "_aosp/system/libbase",
        "Android.bp",
        lock_value(&lock, "LIBBASE_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/tinyxml2",
        lock_value(&lock, "TINYXML2_REVISION")?,
        "external-tinyxml2",
        "",
        "_aosp/external/tinyxml2",
        "Android.bp",
        lock_value(&lock, "TINYXML2_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/libnativehelper",
        lock_value(&lock, "LIBNATIVEHELPER_REVISION")?,
        "libnativehelper-jni",
        "include_jni",
        "_aosp/libnativehelper/include_jni",
        "jni.h",
        lock_value(&lock, "LIBNATIVEHELPER_JNI_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/libnativehelper",
        lock_value(&lock, "LIBNATIVEHELPER_REVISION")?,
        "libnativehelper-header-only",
        "header_only_include",
        "_aosp/libnativehelper/header_only_include",
        "nativehelper/scoped_local_ref.h",
        lock_value(&lock, "LIBNATIVEHELPER_SCOPED_LOCAL_REF_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/libnativehelper",
        lock_value(&lock, "LIBNATIVEHELPER_REVISION")?,
        "libnativehelper-platform-header-only",
        "include_platform_header_only",
        "_aosp/libnativehelper/platform_header_only_include",
        "nativehelper/jni_macros.h",
        lock_value(&lock, "LIBNATIVEHELPER_JNI_MACROS_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/external/dlmalloc",
        lock_value(&lock, "DLMALLOC_REVISION")?,
        "external-dlmalloc",
        "",
        "_aosp/external/dlmalloc",
        "dlmalloc.h",
        lock_value(&lock, "DLMALLOC_HEADER_SHA256")?,
    )?;
    let unwindstack = root.join("_aosp/system/unwinding/libunwindstack");
    let legacy_unwindstack_marker = unwindstack.join("include/.source-revision");
    let unwinding_revision = lock_value(&lock, "UNWINDING_REVISION")?;
    if !unwindstack.join("Android.bp").exists()
        && fs::read_to_string(&legacy_unwindstack_marker)
            .is_ok_and(|value| value.trim() == unwinding_revision)
    {
        // The earlier bootstrap synchronized only public headers. The full
        // pinned implementation supersedes that generated subtree atomically.
        fs::remove_dir_all(&unwindstack)?;
    }
    materialize_archive(
        root,
        "platform/system/unwinding",
        unwinding_revision,
        "system-libunwindstack",
        "libunwindstack",
        "_aosp/system/unwinding/libunwindstack",
        "Android.bp",
        lock_value(&lock, "UNWINDSTACK_ANDROID_BP_SHA256")?,
    )?;
    materialize_archive(
        root,
        "platform/system/libziparchive",
        lock_value(&lock, "LIBZIPARCHIVE_REVISION")?,
        "system-libziparchive",
        "",
        "_aosp/system/libziparchive",
        "Android.bp",
        lock_value(&lock, "LIBZIPARCHIVE_ANDROID_BP_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/prebuilts/runtime",
        lock_value(&lock, "RUNTIME_PREBUILTS_REVISION")?,
        "mainline/i18n/sdk/java/core-icu4j.jar",
        "_prebuilt/android-16/bootclasspath/core-icu4j.jar",
        lock_value(&lock, "CORE_ICU4J_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/prebuilts/runtime",
        lock_value(&lock, "RUNTIME_PREBUILTS_REVISION")?,
        "mainline/i18n/apex/com.android.i18n-arm64.apex",
        "_prebuilt/android-16/i18n/com.android.i18n-arm64.apex",
        lock_value(&lock, "I18N_ARM64_APEX_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "android_icu4j/libcore_bridge/src/java/com/android/i18n/util/ATrace.java",
        "_aosp/external/icu-runtime-bridge/com/android/i18n/util/ATrace.java",
        lock_value(&lock, "ICU_ATRACE_JAVA_SHA256")?,
    )?;
    materialize_file(
        root,
        "platform/external/icu",
        lock_value(&lock, "GRAPHICS_ICU_REVISION")?,
        "android_icu4j/libcore_bridge/src/java/com/android/icu/util/UResourceBundleNative.java",
        "_aosp/external/icu-runtime-bridge/com/android/icu/util/UResourceBundleNative.java",
        lock_value(&lock, "ICU_URESOURCE_BUNDLE_NATIVE_JAVA_SHA256")?,
    )?;
    sync_jit_sources_locked(root, &lock)?;
    Ok(())
}
