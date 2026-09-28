use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use crate::Result;
use crate::support::command_output;

/// Validate the completed headless runtime link after its producer is refreshed
/// by the audit entry point.
///
/// Keeping symbol policy in its own module means changing an acceptance list
/// does not change the link-command orchestration or its native cache inputs.
pub(crate) fn validate_runtime_link(
    build_dir: &Path,
    runtime_library: &Path,
    output: Output,
    description: &str,
) -> Result<()> {
    if output.status.success() {
        let symbols = command_output(Command::new("nm").args(["-gU"]).arg(runtime_library))?;
        for required in [
            "_aim_run_process",
            "_aim_shutdown_process",
            "_aim_dispatch_pointer",
            "_aim_dispatch_pointer_v2",
            "_aim_surface_create",
            "_aim_surface_resize",
            "_aim_surface_get_size",
            "_aim_surface_update",
            "_aim_surface_map_producer",
            "_aim_surface_unmap_producer",
            "_aim_surface_present",
            "_aim_surface_present_async",
            "_aim_surface_pump_events",
            "_aim_surface_close_requested",
            "_aim_appkit_pump_events",
            "_aim_surface_set_input_sink",
            "_aim_android_input_sink_install",
            "_aim_surface_destroy",
            "_aim_provider_install_hooks",
            "_aim_bionic_install_fd_inheritance_boundary",
            "_aim_bionic_install_scm_endpoint_provider",
            "_aim_bionic_uninstall_scm_endpoint_provider",
            "_aim_bionic_socket_broker_is_active",
            "_aim_provider_clear_hooks",
            "_aim_provider_native_acquire",
            "_aim_provider_native_release",
            "_aim_runtime_native_owner_create",
            "_aim_runtime_native_owner_attach",
            "_aim_runtime_native_owner_lookup",
            "_aim_runtime_native_owner_destroy",
        ] {
            if !symbols.contains(required) {
                return Err(format!(
                    "runtime C ABI library does not export required symbol {required}"
                )
                .into());
            }
        }
        let all_symbols = command_output(Command::new("nm").args(["-aC"]).arg(runtime_library))?;
        for required in [
            "aim_elf_graph_load",
            "aim_elf_graph_lookup_root",
            "aim_elf_graph_unload",
            "aim_jni_proxy_init",
            "aim_jni_proxy_java_vm",
            "aim_bionic_namespace_bind_builtins",
            "aim_bionic_binary128_conversion_resolve",
            "aim_bionic_strtold",
            "aim_bionic_strtold_l",
            "aim_bionic_wcstold",
            "aim_bionic_syslog_resolve",
            "aim_bionic_syscall_resolve",
            "aim_bionic_fs_process_install",
            "aim_bionic_fs_process_uninstall",
            "aim_bionic_fs_seed_private_directory",
            "aim_bionic_socket_broker_activate",
            "aim_bionic_socket_broker_deactivate",
            "aim_bionic_socket_broker_resolve",
            "aim_bionic_socket_broker_dns_resolve",
            "aim_bionic_dns_reset_for_test",
            "aim_bionic_stdio_process_install",
            "aim_bionic_stdio_process_uninstall",
            "aim_bionic_formatted_stdio_resolve",
            "aim_bionic_scanf_resolve",
            "aim_bionic_sscanf",
            "aim_bionic_vsscanf",
            "aim_bionic_swprintf_resolve",
            "aim_bionic_swprintf",
            "aim_bionic_ioctl_resolve",
            "aim_bionic_ioctl_activate",
            "aim_bionic_ioctl_deactivate",
            "aim_bionic_sendfile_resolve",
            "aim_bionic_sendfile_activate",
            "aim_bionic_sendfile_deactivate",
            "aim_bionic_sendfile",
            "aim_bionic_strftime_resolve",
            "aim_bionic_strftime_activate",
            "aim_bionic_strftime_deactivate",
            "aim_bionic_wide_stdio_resolve",
            "aim_bionic_fputwc",
            "aim_bionic_getwc",
            "aim_bionic_ungetwc",
            "aim_bionic_wide_float_resolve",
            "aim_bionic_rust_provider_closure_anchor",
            "ElfJniOnLoadTrampoline",
            "CreateRegularTrampolines",
            "TrampolineEntryMask",
        ] {
            if !all_symbols.contains(required) {
                return Err(
                    format!("runtime ELF JNI bridge lacks required symbol {required}").into(),
                );
            }
        }
        println!("audit-runtime-link: product C ABI/ELF closure verified undefined=0");
        return Ok(());
    }

    let stderr = String::from_utf8(output.stderr)?;
    fs::write(build_dir.join("link.err"), &stderr)?;
    if !stderr.contains("Undefined symbols for architecture arm64") {
        return Err(format!("unexpected Runtime link failure: {description}\n{stderr}").into());
    }
    let undefined = stderr
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix('"')?
                .split_once("\", referenced from:")
                .map(|(symbol, _)| symbol.to_owned())
        })
        .collect::<BTreeSet<_>>();
    let symbol_list = undefined.iter().cloned().collect::<Vec<_>>().join("\n") + "\n";
    fs::write(build_dir.join("link.undefined"), symbol_list)?;
    let quick = undefined
        .iter()
        .filter(|symbol| symbol.starts_with("_art_quick_"))
        .count();
    let jni = undefined
        .iter()
        .filter(|symbol| symbol.starts_with("_art_jni_"))
        .count();
    let context = usize::from(undefined.contains("_artContextCopyForLongJump"));
    if quick != 0 || jni != 0 || context != 0 {
        return Err(format!(
            "ARM64 entrypoint link regression: quick={quick} jni={jni} context={context}"
        )
        .into());
    }
    Err(format!(
        "Runtime link closure incomplete: undefined={} quick=0 jni=0 context=0",
        undefined.len()
    )
    .into())
}
