//! Version-pinned, exact embedding ABI for secondary execution clients.
//! ART/process/session state stays in the sole product provider image.
use std::process::Command;

const EXPORTS: &[&str] = &[
    // Rust Engine installs balanced Binder FD transport ownership through
    // these actual broker providers before entering the Android runtime.
    "_aim_bionic_binder_fd_install_owner",
    "_aim_bionic_binder_fd_publish",
    "_aim_bionic_binder_fd_uninstall_owner",
    // Exact AOSP-version execution-client interfaces; policy and process-wide
    // state remain in these product owners, not in copied client archives.
    "__ZN10aim7process19InstallHostServicesEPK24aim_host_services",
    "__ZN10aim8graphics28ConfigureGraphicsEnvironmentEb",
    "__ZN10aim34InitializeFrameworkGraphicsRuntimeEv",
    "__ZN10aim9framework2os31RegisterServiceProcessTransportEP7_JNIEnvP7_jclass",
    "__ZN10aim9framework3app21RunApplicationProcessEP7_JNIEnvPN3art6ThreadE",
    "__ZN10aim9framework3app27FinishFrameworkRegistrationEP7_JNIEnvb",
    "__ZN10aim9framework6system16RunSystemProcessEP7_JNIEnvPKc",
    "__ZN10aim9framework6system20BuildSystemClassPathEPKcS3_PNSt3__112basic_stringIcNS4_11char_traitsIcEENS4_9allocatorIcEEEE",
    "__ZN10aim11runtime_art23StartNativeRegistrationEP7_JNIEnvPN3art6ThreadE",
    "__ZN10aim9embedding17LoadProcessConfigEPNS0_20ProcessConfigOptionsEPNSt3__112basic_stringIcNS3_11char_traitsIcEENS3_9allocatorIcEEEE",
    "__ZN10aim9embedding18RunProcessShutdownEPb",
    "__ZN10aim9embedding21ValidateProcessConfigEPK25aim_process_configPK25aim_process_resultPNS0_19ProcessConfigBoundsEPNSt3__112basic_stringIcNS9_11char_traitsIcEENS9_9allocatorIcEEEE",
    "__ZN10aim9embedding23CompleteProcessShutdownEv",
    "__ZN18aim_process17ScopedRunBoundary14set_art_threadEPN3art6ThreadE",
    "__ZN18aim_process17ScopedRunBoundaryD1Ev",
    "__ZN18aim_process21record_graphics_stateEPN19aim_graphics13GraphicsStateE",
    "__ZN18aim_process22record_created_runtimeEPN3art6ThreadE",
    "__ZN18aim_process23record_dalvikvm_processEv",
    "__ZN18aim_process25owner_thread_for_callbackEv",
    "__ZN18aim_process27graphics_state_for_callbackEv",
    "__ZN18aim_process9begin_runEPK26aim_lifecycle_hooks",
    "__ZN19aim_graphics17state_for_contextEPv",
    "__ZN19aim_graphics23bind_session_art_threadEPN3art6ThreadE",
    "__ZN19aim_graphics24bind_session_for_processEPv",
    "_aim_install_context_loader",
    "_aim_register_code_address",
];

pub(super) fn apply(command: &mut Command) {
    for symbol in EXPORTS {
        command.arg(format!("-Wl,-exported_symbol,{symbol}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedding_surface_has_no_fixture_policy_or_wildcards() {
        let mut seen = std::collections::BTreeSet::new();
        for symbol in EXPORTS {
            assert!(seen.insert(symbol));
            assert!(!symbol.contains('*') && !symbol.contains("fixture"));
            assert!(!symbol.contains("retain_interactive") && !symbol.contains("Probe"));
        }
    }
}
