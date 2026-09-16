//! Validated archive inputs for the real-graphics runtime link.
//!
//! The linker itself should not know how the Android framework archive set is
//! discovered. Keeping this list in one owned value also makes missing-input
//! failures occur before any native translation unit is compiled.

use super::common::require_file;
use super::*;

pub(super) struct GraphicsRuntimeInputs {
    pub(super) graphics_closure: PathBuf,
    pub(super) bootstrap: PathBuf,
    pub(super) icu_jni_archive: PathBuf,
    pub(super) libcore_linux_archive: PathBuf,
    pub(super) os_constants_archive: PathBuf,
    pub(super) unix_filesystem_archive: PathBuf,
    pub(super) openjdkjvm_archive: PathBuf,
    pub(super) openjdkjvmti_archive: PathBuf,
    pub(super) managed_load_archive: PathBuf,
    pub(super) file_input_stream_archive: PathBuf,
    pub(super) file_descriptor_archive: PathBuf,
    pub(super) system_natives_archive: PathBuf,
    pub(super) boringssl_crypto_archive: PathBuf,
    pub(super) unix_native_dispatcher_archive: PathBuf,
    pub(super) fdlibm_archive: PathBuf,
    pub(super) openjdk_nio_mapping_archive: PathBuf,
    pub(super) openjdk_nio_support_archive: PathBuf,
    pub(super) libcore_memory_archive: PathBuf,
    pub(super) libcore_jni_constants_archive: PathBuf,
    pub(super) asynchronous_close_registrar: PathBuf,
    pub(super) asynchronous_close_backend: PathBuf,
    pub(super) resource_jni_archive: PathBuf,
    pub(super) android_util_log_archive: PathBuf,
    pub(super) application_shared_memory_archive: PathBuf,
    pub(super) debugstore_archive: PathBuf,
    pub(super) activity_thread_archive: PathBuf,
    pub(super) bionic_dlwarning_object: PathBuf,
    pub(super) classloader_factory_jni: PathBuf,
    pub(super) trace_archive: PathBuf,
    pub(super) perfetto_library: PathBuf,
    pub(super) virtual_ref_base_ptr_archive: PathBuf,
    pub(super) android_runtime_host: PathBuf,
}

impl GraphicsRuntimeInputs {
    pub(super) fn load(root: &Path, build_paths: &BuildPaths) -> Result<Self> {
        let inputs = Self {
            graphics_closure: root
                .join("_build/graphics-runtime-closure-audit/android16-graphics-runtime-closure.o"),
            bootstrap: build_paths
                .native_output("runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a"),
            icu_jni_archive: root.join("_build/icu-jni-foundation/libicu-jni-darwin.a"),
            libcore_linux_archive: root.join("_build/libcore-darwin-linux/libcore-darwin-linux.a"),
            os_constants_archive: root
                .join("_build/os-constants/libandroid-system-os-constants-darwin.a"),
            unix_filesystem_archive: root
                .join("_build/unix-filesystem-darwin/libopenjdk-unix-filesystem-darwin.a"),
            openjdkjvm_archive: root.join("_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a"),
            openjdkjvmti_archive: root
                .join("_build/openjdkjvmti-darwin/libopenjdkjvmti-darwin.a"),
            managed_load_archive: root.join(
                "_build/android-managed-native-load/libopenjdk-runtime-managed-load-darwin.a",
            ),
            file_input_stream_archive: root
                .join("_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a"),
            file_descriptor_archive: root
                .join("_build/file-descriptor-darwin/libopenjdk-file-descriptor-darwin.a"),
            system_natives_archive: root
                .join("_build/system-natives-darwin/libopenjdk-system-natives-darwin.a"),
            boringssl_crypto_archive: root
                .join("_build/system-natives-darwin/libcrypto-boringssl-darwin.a"),
            unix_native_dispatcher_archive: root.join(
                "_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a",
            ),
            fdlibm_archive: root
                .join("_build/unix-native-dispatcher-darwin/libfdlibm-darwin.a"),
            openjdk_nio_mapping_archive: root
                .join("_build/openjdk-nio-mapping/libopenjdk-nio-mapping-darwin.a"),
            openjdk_nio_support_archive: root
                .join("_build/openjdk-nio-mapping/libopenjdk-nio-support-darwin.a"),
            libcore_memory_archive: root
                .join("_build/libcore-memory/libcore-memory-art-runtime-darwin.a"),
            libcore_jni_constants_archive: root
                .join("_build/libcore-memory/libcore-jni-constants-art-runtime-darwin.a"),
            asynchronous_close_registrar: root.join(
                "_build/asynchronous-close-monitor/libcore-io-asynchronous-close-monitor-registrar-darwin.a",
            ),
            asynchronous_close_backend: root
                .join("_build/asynchronous-close-monitor/libandroidio-darwin.a"),
            resource_jni_archive: root
                .join("_build/resource-jni-foundation/libandroid-resource-jni-darwin.a"),
            android_util_log_archive: root
                .join("_build/android-util-log/libandroid-util-log-registrar-darwin.a"),
            application_shared_memory_archive: root
                .join("_build/application-shared-memory/libapplication-shared-memory-darwin.a"),
            debugstore_archive: root.join("_build/debugstore/libdebugstore-darwin.a"),
            activity_thread_archive: root.join("_build/activity-thread/libactivity-thread-darwin.a"),
            bionic_dlwarning_object: root
                .join("_build/bionic-linker-config/linker_dlwarning.o"),
            classloader_factory_jni: root
                .join("_build/native-loader-policy/classloader_factory_jni.o"),
            trace_archive: root.join("_build/tracing-perfetto/libandroid-tracing-perfetto-darwin.a"),
            perfetto_library: root.join("_build/tracing-perfetto/perfetto-out/libperfetto_c.dylib"),
            virtual_ref_base_ptr_archive: root
                .join("_build/virtual-ref-base-ptr/libandroid-virtual-ref-base-ptr-darwin.a"),
            android_runtime_host: root
                .join("_build/android-runtime-host/libandroid-runtime-darwin-host.a"),
        };

        for input in [
            &inputs.graphics_closure,
            &inputs.bootstrap,
            &inputs.icu_jni_archive,
            &inputs.libcore_linux_archive,
            &inputs.os_constants_archive,
            &inputs.unix_filesystem_archive,
            &inputs.openjdkjvm_archive,
            &inputs.openjdkjvmti_archive,
            &inputs.managed_load_archive,
            &inputs.file_input_stream_archive,
            &inputs.file_descriptor_archive,
            &inputs.system_natives_archive,
            &inputs.boringssl_crypto_archive,
            &inputs.unix_native_dispatcher_archive,
            &inputs.fdlibm_archive,
            &inputs.openjdk_nio_mapping_archive,
            &inputs.openjdk_nio_support_archive,
            &inputs.libcore_memory_archive,
            &inputs.libcore_jni_constants_archive,
            &inputs.asynchronous_close_registrar,
            &inputs.asynchronous_close_backend,
            &inputs.resource_jni_archive,
            &inputs.android_util_log_archive,
            &inputs.application_shared_memory_archive,
            &inputs.debugstore_archive,
            &inputs.activity_thread_archive,
            &inputs.bionic_dlwarning_object,
            &inputs.classloader_factory_jni,
            &inputs.trace_archive,
            &inputs.perfetto_library,
            &inputs.virtual_ref_base_ptr_archive,
            &inputs.android_runtime_host,
        ] {
            require_file(input, "real-graphics runtime input is missing")?;
        }

        let icu_jni_members =
            command_output(Command::new("ar").arg("-t").arg(&inputs.icu_jni_archive))?
                .lines()
                .filter(|member| *member != "__.SYMDEF")
                .count();
        if icu_jni_members != 15 {
            return Err(format!(
                "module-complete Android ICU JNI archive has {icu_jni_members} members, expected 15"
            )
            .into());
        }
        Ok(inputs)
    }
}
