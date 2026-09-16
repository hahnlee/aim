use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::build_context::BuildPaths;
use crate::help;
use crate::native_build::{
    FileHashCache, PendingNativeCompile, build_elf_loader, common_cpp_command, compile_cpp,
    compile_pending_native, compile_with_dependency_cache, create_archive, link_with_cache,
};
use crate::support::{command_output, describe_command, run_command};

use crate::Result;

#[cfg(target_os = "macos")]
use crate::getpagesize;

pub(crate) fn run() -> Result<()> {
    let command = env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let root = workspace_root()?;

    match command.as_str() {
        "doctor" => doctor(),
        "sync" => sync_sources(&root),
        "sync-jit-sources" => sync_jit_sources(&root),
        "probe-asm" => probe_asm(&root),
        "probe-pagesize" => probe_page_size(&root),
        "build-foundation" => build_foundation(&root),
        "build-skia" => build_skia(&root),
        "build-hwui-canvas" => build_shell_gate(&root, "compile-android16-hwui-canvas-gate.sh"),
        "build-android-graphics-jni" => build_shell_gate_with_args(
            &root,
            "build-android16-android-graphics-jni.sh",
            &["--object-audit"],
        ),
        "build-hwui-static" => build_shell_gate(&root, "build-android16-hwui-static-foundation.sh"),
        "build-androidfw" => build_shell_gate(&root, "build-android16-androidfw-foundation.sh"),
        "build-resource-jni" => build_shell_gate(&root, "build-android16-resource-jni.sh"),
        "build-android-util-log" => build_shell_gate(&root, "build-android16-android-util-log.sh"),
        "build-android-runtime-host" => {
            build_shell_gate(&root, "build-android16-android-runtime-host.sh")
        }
        "build-binder-jni" => build_shell_gate(&root, "build-android16-binder-jni.sh"),
        "build-libcore-linux" => build_shell_gate(&root, "build-android16-libcore-darwin-linux.sh"),
        "build-os-constants" => build_shell_gate(&root, "build-android16-os-constants-darwin.sh"),
        "build-unix-filesystem" => {
            build_shell_gate(&root, "build-android16-unix-filesystem-darwin.sh")
        }
        "build-openjdkjvm" => build_shell_gate(&root, "build-android16-openjdkjvm-darwin.sh"),
        "build-file-input-stream" => {
            build_shell_gate(&root, "build-android16-file-input-stream-darwin.sh")
        }
        "build-file-descriptor" => {
            build_shell_gate(&root, "build-android16-file-descriptor-darwin.sh")
        }
        "build-system-natives" => {
            build_shell_gate(&root, "build-android16-system-natives-darwin.sh")
        }
        "build-unix-native-dispatcher" => {
            build_shell_gate(&root, "build-android16-unix-native-dispatcher-darwin.sh")
        }
        "build-openjdk-nio-mapping" => {
            build_shell_gate(&root, "build-android16-openjdk-nio-mapping.sh")
        }
        "build-libcore-memory" => {
            build_shell_gate(&root, "build-android16-libcore-memory-darwin.sh")
        }
        "build-ziparchive-incfs" => build_shell_gate(&root, "build-android16-ziparchive-incfs.sh"),
        "build-hostgraphics" => build_shell_gate(&root, "build-android16-hostgraphics.sh"),
        "build-skia-hwui" => build_shell_gate(&root, "build-android16-skia-hwui-force-load.sh"),
        "build-graphics-codec-modules" => {
            build_shell_gate(&root, "build-android16-codec-foundation.sh")
        }
        "build-libbase" => build_shell_gate(&root, "build-android16-libbase-foundation.sh"),
        "build-icu-runtime-adapters" => {
            build_shell_gate(&root, "build-android16-icu-runtime-adapters.sh")
        }
        "build-graphics-foundations" => {
            build_shell_gate(&root, "build-android16-graphics-foundations.sh")
        }
        "build-nativehelper" => {
            build_shell_gate(&root, "build-android16-nativehelper-foundation.sh")
        }
        "build-ui-types" => build_shell_gate(&root, "build-android16-ui-types-foundation.sh"),
        "build-graphics-codecs" => build_shell_gate(&root, "build-android16-graphics-codecs.sh"),
        "build-harfbuzz" => build_shell_gate_with_args(
            &root,
            "build-android16-harfbuzz-foundation.sh",
            &["--archive-only"],
        ),
        "build-minikin" => build_shell_gate(&root, "build-android16-minikin-foundation.sh"),
        "build-skia-text" => build_shell_gate(&root, "build-android16-skia-text-raster.sh"),
        "build-icu" => build_shell_gate(&root, "build-android16-icu-foundation.sh"),
        "probe-minikin-shaping" => {
            build_shell_gate(&root, "run-android16-minikin-shaping-acceptance.sh")
        }
        "check-text-shaping" => build_shell_gate(&root, "check-android16-text-shaping-inputs.sh"),
        "build-dex" => build_dex_probe(&root),
        "build-elf-jni-dex" => build_elf_jni_dex_probe(&root),
        "build-network-dex" => build_network_dex_probe(&root),
        "build-button-dex" => build_button_dex_probe(&root),
        "build-runtime-platform" => build_runtime_platform(&root),
        "build-runtime-core" => build_runtime_core(&root),
        "probe-park" => probe_park(&root),
        "build-runtime-arm64" => build_runtime_arm64(&root),
        "build-runtime-unwindstack-core" => build_runtime_unwindstack_core(&root).map(|_| ()),
        "build-interpreter-core" => build_interpreter_core(&root),
        "build-nterp-arm64ng" => build_nterp_arm64ng(&root).map(|_| ()),
        "build-jit-compiler" => build_jit_compiler(&root),
        "build-dex2oat" => build_dex2oat(&root).map(|_| ()),
        "build-android16-boot-image" => build_android16_boot_image(&root),
        "build-jit-libelffile" => build_jit_libelffile(&root).map(|_| ()),
        "prepare-runtime-common-shadow" => prepare_runtime_shadow(&root).map(|_| ()),
        "build-runtime-bootstrap" => build_runtime_bootstrap(&root),
        "build-runtime-bootstrap-internal" => build_runtime_bootstrap_inner(&root),
        "build-runtime-graphics-bootstrap" => build_runtime_graphics_bootstrap(&root),
        "build-graphics-foundation" => build_graphics_foundation(&root),
        "build-runtime-graphics-bootstrap-internal" => {
            build_runtime_graphics_bootstrap_inner(&root)
        }
        "build-runtime-filesystem-probe" => build_runtime_filesystem_probe(&root),
        "build-runtime-network-probe" => build_runtime_network_probe(&root),
        "build-runtime-graphics-phase-probe" => build_runtime_graphics_phase_probe(&root),
        "build-runtime-graphics-gpu-probe" => build_runtime_graphics_gpu_probe(&root),
        "build-runtime-graphics-input-probe" => build_runtime_graphics_input_probe(&root),
        "build-runtime-graphics-state-probe" => build_runtime_graphics_state_probe(&root),
        "build-runtime-graphics-session-probe" => build_runtime_graphics_session_probe(&root),
        "build-runtime-jni-acceptance-probe" => build_runtime_jni_acceptance_probe(&root),
        "build-runtime-app-bootstrap-probe" => build_runtime_app_bootstrap_probe(&root),
        "build-runtime-app-resources-probe" => build_runtime_app_resources_probe(&root),
        "build-runtime-app-activity-probe" => build_runtime_app_activity_probe(&root),
        "build-runtime-app-presentation-probe" => build_runtime_app_presentation_probe(&root),
        "build-runtime-hwui-probe" => build_runtime_hwui_probe(&root),
        "audit-runtime-link" => audit_runtime_link(&root),
        "audit-runtime-graphics-link" => audit_runtime_graphics_link(&root),
        "audit-runtime-graphics-link-fast" => audit_runtime_graphics_link_fast(&root),
        "audit-runtime-graphics-link-incremental" => audit_runtime_graphics_link_incremental(&root),
        "audit-graphics-closure" => build_shell_gate(&root, "audit-android16-graphics-closure.sh"),
        "probe-runtime-dex" => probe_runtime_dex(&root, false),
        "probe-runtime-elf-jni" => probe_runtime_elf_jni(&root),
        "probe-runtime-network" => probe_runtime_network(&root),
        "probe-runtime-apk-direct" => probe_runtime_apk_direct(&root),
        "probe-window" => probe_runtime_dex(&root, true),
        "probe-runtime-graphics" => probe_runtime_graphics(&root),
        "probe-runtime-graphics-window" => probe_runtime_graphics_window(&root),
        "probe-runtime-button" => probe_runtime_button(&root, false),
        "probe-runtime-button-window" => probe_runtime_button(&root, true),
        "probe-runtime-apk-app" => probe_runtime_apk_app(&root, false),
        "probe-runtime-apk-app-window" => probe_runtime_apk_app(&root, true),
        "probe-runtime-apk-jni-app" => probe_runtime_apk_jni_app(&root, false),
        "probe-runtime-apk-jni-app-window" => probe_runtime_apk_jni_app(&root, true),
        "verify-bootclasspath" => verify_bootclasspath(&root),
        "all" => {
            doctor()?;
            sync_sources(&root)?;
            probe_asm(&root)?;
            probe_page_size(&root)?;
            build_skia(&root)?;
            build_dex_probe(&root)?;
            build_runtime_platform(&root)?;
            build_runtime_core(&root)?;
            build_runtime_arm64(&root)?;
            build_interpreter_core(&root)?;
            build_runtime_bootstrap(&root)?;
            audit_runtime_link(&root)?;
            probe_runtime_dex(&root, false)?;
            probe_park(&root)
        }
        "help" | "--help" | "-h" => {
            help::print_help();
            Ok(())
        }
        other => Err(format!("unknown command: {other}").into()),
    }
}

#[path = "audit_commands.rs"]
mod audit_commands;
#[path = "bootclasspath.rs"]
mod bootclasspath;
#[path = "dex_commands.rs"]
mod dex_commands;
#[path = "fixture_commands.rs"]
mod fixture_commands;
#[path = "native_probe_commands.rs"]
mod native_probe_commands;
#[path = "probe_commands.rs"]
mod probe_commands;
#[path = "runtime_art_build.rs"]
mod runtime_art_build;
#[path = "runtime_bootstrap/mod.rs"]
mod runtime_bootstrap;
#[path = "runtime_commands.rs"]
mod runtime_commands;
#[path = "runtime_toolchain.rs"]
mod runtime_toolchain;
#[path = "source_commands.rs"]
mod source_commands;
#[path = "source_materialization.rs"]
mod source_materialization;

pub(crate) use audit_commands::*;
pub(crate) use bootclasspath::*;
pub(crate) use dex_commands::*;
pub(crate) use fixture_commands::*;
pub(crate) use native_probe_commands::*;
pub(crate) use probe_commands::*;
pub(crate) use runtime_art_build::*;
pub(crate) use runtime_bootstrap::*;
pub(crate) use runtime_commands::*;
pub(crate) use runtime_toolchain::*;
pub(crate) use source_commands::*;
pub(crate) use source_materialization::*;

#[cfg(test)]
mod tests {
    use crate::native_cache::parse_makefile_words;

    #[test]
    fn parses_escaped_makefile_dependency_paths() {
        assert_eq!(
            parse_makefile_words(" source.cc include/header.h path\\ with\\ spaces/header.h "),
            ["source.cc", "include/header.h", "path with spaces/header.h"]
        );
    }
}
