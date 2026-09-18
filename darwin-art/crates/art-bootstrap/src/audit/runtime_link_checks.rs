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
            "_darwin_art_run_process",
            "_darwin_art_shutdown_process",
            "_darwin_art_dispatch_pointer",
            "_darwin_art_dispatch_pointer_v2",
            "_darwin_art_surface_create",
            "_darwin_art_surface_resize",
            "_darwin_art_surface_get_size",
            "_darwin_art_surface_update",
            "_darwin_art_surface_map_producer",
            "_darwin_art_surface_unmap_producer",
            "_darwin_art_surface_present",
            "_darwin_art_surface_present_async",
            "_darwin_art_surface_pump_events",
            "_darwin_art_surface_close_requested",
            "_darwin_art_appkit_pump_events",
            "_darwin_art_surface_set_input_sink",
            "_darwin_art_android_input_sink_install",
            "_darwin_art_surface_destroy",
            "_darwin_art_provider_install_hooks",
            "_darwin_art_bionic_install_fd_inheritance_boundary",
            "_darwin_art_bionic_install_scm_endpoint_provider",
            "_darwin_art_bionic_uninstall_scm_endpoint_provider",
            "_darwin_art_bionic_socket_broker_is_active",
            "_darwin_art_provider_clear_hooks",
            "_darwin_art_provider_native_acquire",
            "_darwin_art_provider_native_release",
            "_darwin_art_runtime_native_owner_create",
            "_darwin_art_runtime_native_owner_attach",
            "_darwin_art_runtime_native_owner_lookup",
            "_darwin_art_runtime_native_owner_destroy",
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
            "darwin_art_elf_graph_load",
            "darwin_art_elf_graph_lookup_root",
            "darwin_art_elf_graph_unload",
            "darwin_art_jni_proxy_init",
            "darwin_art_jni_proxy_java_vm",
            "darwin_art_bionic_namespace_bind_builtins",
            "darwin_art_bionic_binary128_conversion_resolve",
            "darwin_art_bionic_strtold",
            "darwin_art_bionic_strtold_l",
            "darwin_art_bionic_wcstold",
            "darwin_art_bionic_syslog_resolve",
            "darwin_art_bionic_syscall_resolve",
            "darwin_art_bionic_fs_process_install",
            "darwin_art_bionic_fs_process_uninstall",
            "darwin_art_bionic_fs_seed_private_directory",
            "darwin_art_bionic_socket_broker_activate",
            "darwin_art_bionic_socket_broker_deactivate",
            "darwin_art_bionic_socket_broker_resolve",
            "darwin_art_bionic_socket_broker_dns_resolve",
            "darwin_art_bionic_dns_reset_for_test",
            "darwin_art_bionic_stdio_process_install",
            "darwin_art_bionic_stdio_process_uninstall",
            "darwin_art_bionic_formatted_stdio_resolve",
            "darwin_art_bionic_scanf_resolve",
            "darwin_art_bionic_sscanf",
            "darwin_art_bionic_vsscanf",
            "darwin_art_bionic_swprintf_resolve",
            "darwin_art_bionic_swprintf",
            "darwin_art_bionic_ioctl_resolve",
            "darwin_art_bionic_ioctl_activate",
            "darwin_art_bionic_ioctl_deactivate",
            "darwin_art_bionic_sendfile_resolve",
            "darwin_art_bionic_sendfile_activate",
            "darwin_art_bionic_sendfile_deactivate",
            "darwin_art_bionic_sendfile",
            "darwin_art_bionic_strftime_resolve",
            "darwin_art_bionic_strftime_activate",
            "darwin_art_bionic_strftime_deactivate",
            "darwin_art_bionic_wide_stdio_resolve",
            "darwin_art_bionic_fputwc",
            "darwin_art_bionic_getwc",
            "darwin_art_bionic_ungetwc",
            "darwin_art_bionic_wide_float_resolve",
            "darwin_art_bionic_rust_provider_closure_anchor",
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
