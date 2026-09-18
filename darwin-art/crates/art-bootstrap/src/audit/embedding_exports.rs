//! Version-pinned, exact embedding ABI for secondary execution clients.
//! ART/process/session state stays in the sole product provider image.
use std::process::Command;

const EXPORTS: &[&str] = &[
    // Rust Engine installs balanced Binder FD transport ownership through
    // these actual broker providers before entering the Android runtime.
    "_darwin_art_bionic_binder_fd_install_owner",
    "_darwin_art_bionic_binder_fd_publish",
    "_darwin_art_bionic_binder_fd_uninstall_owner",
    // Exact AOSP-version execution-client interfaces; policy and process-wide
    // state remain in these product owners, not in copied client archives.
    "__ZN10darwin_art7process19InstallHostServicesEPK24darwin_art_host_services",
    "__ZN10darwin_art8graphics28ConfigureGraphicsEnvironmentEb",
    "__ZN10darwin_art34InitializeFrameworkGraphicsRuntimeEv",
    "__ZN10darwin_art9framework2os31RegisterServiceProcessTransportEP7_JNIEnvP7_jclass",
    "__ZN10darwin_art9framework2pm20QueryInstalledRecordEP7_JNIEnvPKcP8_jstring",
    "__ZN10darwin_art9framework3app21RunApplicationProcessEP7_JNIEnvPN3art6ThreadE",
    "__ZN10darwin_art9framework3app27FinishFrameworkRegistrationEP7_JNIEnvb",
    "__ZN10darwin_art9framework6system16RunSystemProcessEP7_JNIEnvPKcPFP8_jstringS3_P7_jclassS7_E",
    "__ZN10darwin_art9framework6system20BuildSystemClassPathEPKcS3_PNSt3__112basic_stringIcNS4_11char_traitsIcEENS4_9allocatorIcEEEE",
    "__ZN10darwin_art11runtime_art23StartNativeRegistrationEP7_JNIEnvPN3art6ThreadE",
    "__ZN10darwin_art9embedding17LoadProcessConfigEPNS0_20ProcessConfigOptionsEPNSt3__112basic_stringIcNS3_11char_traitsIcEENS3_9allocatorIcEEEE",
    "__ZN10darwin_art9embedding18RunProcessShutdownEPb",
    "__ZN10darwin_art9embedding21ValidateProcessConfigEPK25darwin_art_process_configPK25darwin_art_process_resultPNS0_19ProcessConfigBoundsEPNSt3__112basic_stringIcNS9_11char_traitsIcEENS9_9allocatorIcEEEE",
    "__ZN10darwin_art9embedding23CompleteProcessShutdownEv",
    "__ZN18darwin_art_process17ScopedRunBoundary14set_art_threadEPN3art6ThreadE",
    "__ZN18darwin_art_process17ScopedRunBoundaryD1Ev",
    "__ZN18darwin_art_process21record_graphics_stateEPN19darwin_art_graphics13GraphicsStateE",
    "__ZN18darwin_art_process22record_created_runtimeEPN3art6ThreadE",
    "__ZN18darwin_art_process23record_dalvikvm_processEv",
    "__ZN18darwin_art_process25owner_thread_for_callbackEv",
    "__ZN18darwin_art_process27graphics_state_for_callbackEv",
    "__ZN18darwin_art_process9begin_runEPK26darwin_art_lifecycle_hooks",
    "__ZN19darwin_art_graphics17state_for_contextEPv",
    "__ZN19darwin_art_graphics23bind_session_art_threadEPN3art6ThreadE",
    "__ZN19darwin_art_graphics24bind_session_for_processEPv",
    "_darwin_art_install_context_loader",
    "_darwin_art_register_code_address",
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
