use super::*;

pub(crate) fn find_ndk_headers() -> Result<(PathBuf, PathBuf)> {
    let mut ndk_roots = Vec::new();
    for variable in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"] {
        if let Some(value) = env::var_os(variable) {
            ndk_roots.push(PathBuf::from(value));
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let ndk_parent = PathBuf::from(home).join("Library/Android/sdk/ndk");
        if let Ok(entries) = fs::read_dir(ndk_parent) {
            let mut installed: Vec<PathBuf> = entries
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            installed.sort();
            installed.reverse();
            ndk_roots.extend(installed);
        }
    }

    for ndk_root in ndk_roots {
        let prebuilt = ndk_root.join("toolchains/llvm/prebuilt");
        let Ok(hosts) = fs::read_dir(prebuilt) else {
            continue;
        };
        for host in hosts.filter_map(std::result::Result::ok) {
            let include = host.path().join("sysroot/usr/include");
            let arch = include.join("aarch64-linux-android");
            if include.join("elf.h").exists() && arch.is_dir() {
                return Ok((include, arch));
            }
        }
    }
    Err("Android NDK headers are required for the Android ELF ABI (<elf.h>)".into())
}
