//! Shared ABI belongs to a provider, not to every object in its archive.
//! Mixed Darwin archives deliberately have no implicit public ABI.
#[derive(Clone, Copy)]
pub(super) enum ProviderClass {
    MixedArt,
    PinnedUpstream,
    DarwinBionic,
}

// AOSP16 OpenJDK's separate named-JNI image imports this exact Darwin facade.
// New upstream imports must be reviewed here, rather than exposing an archive.
const BIONIC: &[&str] = &[
    "_darwin_art_bionic_access",
    "_darwin_art_bionic_chmod",
    "_darwin_art_bionic_close",
    "_darwin_art_bionic_closedir",
    "_darwin_art_bionic_fchmod",
    "_darwin_art_bionic_fchown",
    "_darwin_art_bionic_fd_export_for_scm",
    "_darwin_art_bionic_fs_fcntl_core",
    "_darwin_art_bionic_fs_owns_fd_core",
    "_darwin_art_bionic_fs_resolve_private_host_path",
    "_darwin_art_bionic_fs_statvfs_core",
    "_darwin_art_bionic_fstat",
    "_darwin_art_bionic_fsync",
    "_darwin_art_bionic_ftruncate",
    "_darwin_art_bionic_getcwd",
    "_darwin_art_bionic_ioctl",
    "_darwin_art_bionic_link",
    "_darwin_art_bionic_lseek",
    "_darwin_art_bionic_lstat",
    "_darwin_art_bionic_mkdir",
    "_darwin_art_bionic_open",
    "_darwin_art_bionic_opendir",
    "_darwin_art_bionic_pathconf",
    "_darwin_art_bionic_read",
    "_darwin_art_bionic_readdir",
    "_darwin_art_bionic_readlink",
    "_darwin_art_bionic_realpath",
    "_darwin_art_bionic_rename",
    "_darwin_art_bionic_socket_broker_close",
    "_darwin_art_bionic_socket_broker_read",
    "_darwin_art_bionic_socket_broker_write",
    "_darwin_art_bionic_stat",
    "_darwin_art_bionic_statvfs",
    "_darwin_art_bionic_symlink",
    "_darwin_art_bionic_unlinkat",
    "_darwin_art_bionic_utimensat",
    "_darwin_art_bionic_write",
];

fn art_nested(name: &str) -> bool {
    name.strip_prefix('N')
        .is_some_and(|rest| rest.trim_start_matches(['K', 'V', 'r']).starts_with("3art"))
}

fn offset(name: &str) -> Option<&str> {
    let (kind, rest) = (name.chars().next()?, &name[1..]);
    let fields = match kind {
        'h' => 1,
        'v' => 2,
        _ => return None,
    };
    let mut rest = rest;
    for _ in 0..fields {
        let (number, next) = rest.split_once('_')?;
        if number.is_empty()
            || !number
                .trim_start_matches('n')
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        rest = next;
    }
    Some(rest)
}

fn art_owner(symbol: &str) -> bool {
    if ["_art_", "_JVM_", "_jio_"]
        .iter()
        .any(|p| symbol.starts_with(p))
    {
        return true;
    }
    if [
        "_JNI_CreateJavaVM",
        "_JNI_GetCreatedJavaVMs",
        "_JNI_GetDefaultJavaVMInitArgs",
        "_ArtPlugin_Initialize",
        "_ArtPlugin_Deinitialize",
        "___jit_debug_descriptor",
        "___dex_debug_descriptor",
        // Pinned ART inline allocation and loader/bridge interfaces are C
        // providers, not declaring-namespace C++ methods or fixture helpers.
        "_mspace_malloc",
        "_mspace_usable_size",
        "_InitializeNativeLoader",
        "_OpenNativeLibrary",
        "_CloseNativeLibrary",
        "_NativeLoaderFreeErrorMessage",
        "_NativeBridgeGetTrampoline2",
    ]
    .contains(&symbol)
    {
        return true;
    }
    let Some(name) = symbol.strip_prefix("__Z") else {
        return false;
    };
    if art_nested(name) {
        return true;
    }
    // RTTI, vtables, construction tables, guards and TLS wrappers retain the
    // declaring ART namespace; an art:: argument in a host method is irrelevant.
    for prefix in ["TV", "TI", "TS", "TT", "TC", "GV", "TH", "TW"] {
        if name.strip_prefix(prefix).is_some_and(art_nested) {
            return true;
        }
    }
    if let Some(thunk) = name.strip_prefix('T') {
        if let Some(rest) = thunk.strip_prefix('c') {
            return offset(rest).and_then(offset).is_some_and(art_nested);
        }
        return offset(thunk).is_some_and(art_nested);
    }
    false
}

pub(super) fn allows(class: ProviderClass, symbol: &str) -> bool {
    if !symbol.starts_with('_')
        || symbol.contains('$')
        || symbol.contains("GLOBAL__sub_I_")
        || symbol.contains("cxx_global_var_init")
    {
        return false;
    }
    match class {
        ProviderClass::MixedArt => art_owner(symbol),
        ProviderClass::PinnedUpstream => true,
        ProviderClass::DarwinBionic => BIONIC.contains(&symbol),
    }
}

pub(super) fn validate_bionic_imports(symbols: &str) -> Result<(), String> {
    for symbol in symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
    {
        if symbol.starts_with("_darwin_art_bionic_") && !BIONIC.contains(&symbol) {
            return Err(format!(
                "unreviewed OpenJDK cross-image provider import: {symbol}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declaring_owner_not_argument_or_observation() {
        for symbol in [
            "__ZN3art7Runtime5StartEv",
            "__ZNK3art7Runtime3FooEv",
            "__ZTVN3art7RuntimeE",
            "__ZTIN3art7RuntimeE",
            "__ZThn8_N3art7Runtime3FooEv",
            "__ZTv0_n24_N3art7Runtime3FooEv",
            "_art_quick_to_interpreter_bridge",
            "_mspace_malloc",
            "_mspace_usable_size",
            "_InitializeNativeLoader",
            "_OpenNativeLibrary",
            "_CloseNativeLibrary",
            "_NativeLoaderFreeErrorMessage",
            "_NativeBridgeGetTrampoline2",
        ] {
            assert!(allows(ProviderClass::MixedArt, symbol), "{symbol}");
        }
        for symbol in [
            "__ZN10darwin_art3FooEPN3art6ThreadE",
            "__ZN10darwin_art11android_jni19TrampolineLiveCountEv",
            "_darwin_art_bionic_dns_reset_for_test",
            "_Java_Main_hasJit",
            "_darwin_art_debug_entrypoint",
            "__ZN10darwin_art38VerifyDarwinMediaCodecSurfaceLifecycleEP7_JNIEnv",
        ] {
            assert!(!allows(ProviderClass::MixedArt, symbol));
            assert!(!allows(ProviderClass::DarwinBionic, symbol));
        }
    }
    #[test]
    fn bionic_imports_are_exact_and_fail_closed() {
        assert!(validate_bionic_imports("_darwin_art_bionic_open\n_JVM_Sync").is_ok());
        assert!(validate_bionic_imports("_darwin_art_bionic_unreviewed").is_err());
        assert!(!allows(
            ProviderClass::DarwinBionic,
            "_darwin_art_bionic_socket_broker_is_active"
        ));
        assert!(!allows(
            ProviderClass::PinnedUpstream,
            "___cxx_global_var_init"
        ));
    }
}
