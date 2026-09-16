//! Byte-preserving Android 16 RUNPATH token expansion. These are unresolved
//! candidates, NOT search grants. The guest filesystem must resolve/check
//! directories (and APK entries) before passing them to namespace search.
use super::*;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

/// Resolve expanded directories through guest filesystem authority. Unusable
/// directories are ignored as by AOSP resolve_paths. APK entry paths need the
/// separate archive resolver and are not supported by this directory API.
pub fn resolve_runpath_directories<F>(
    raw: &[u8],
    image: &Path,
    mut open: F,
) -> Result<Vec<PathBuf>, NamespaceError>
where
    F: FnMut(&Path) -> Option<(PathBuf, std::fs::File)>,
{
    let mut result = Vec::new();
    for path in expand_runpath(raw, image)? {
        if let Some((canonical, file)) = open(&path)
            && resolved(&canonical)
            && file.metadata().is_ok_and(|metadata| metadata.is_dir())
        {
            result.push(canonical);
        }
    }
    Ok(result)
}

/// Mirrors linker_soinfo.cpp set_dt_runpath / linker_utils.cpp format_string
/// for the arm64 runtime. No host environment variables are consulted. The
/// source image path must be its canonical guest path, never a host path.
pub fn expand_runpath(raw: &[u8], image_path: &Path) -> Result<Vec<PathBuf>, NamespaceError> {
    if raw.contains(&0) || !resolved(image_path) {
        return Err(NamespaceError::InvalidRunpath);
    }
    let origin = image_path.parent().ok_or(NamespaceError::InvalidRunpath)?;
    let replacements: [(&[u8], &[u8]); 2] = [
        (b"ORIGIN", origin.as_os_str().as_bytes()),
        (b"LIB", b"lib64"),
    ];
    let mut paths = Vec::new();
    for entry in raw
        .split(|byte| *byte == b':')
        .filter(|part| !part.is_empty())
    {
        let mut expanded = Vec::new();
        let mut position = 0;
        while position < entry.len() {
            if entry[position] == b'$' {
                let tail = &entry[position + 1..];
                let mut matched = false;
                for (token, value) in replacements {
                    let consumed = if tail.starts_with(token) {
                        Some(token.len())
                    } else if tail.starts_with(b"{")
                        && tail[1..].starts_with(token)
                        && tail.get(token.len() + 1) == Some(&b'}')
                    {
                        Some(token.len() + 2)
                    } else {
                        None
                    };
                    if let Some(consumed) = consumed {
                        expanded.extend_from_slice(value);
                        position += consumed + 1;
                        matched = true;
                        break;
                    }
                }
                if matched {
                    continue;
                }
            }
            expanded.push(entry[position]);
            position += 1;
        }
        paths.push(PathBuf::from(std::ffi::OsString::from_vec(expanded)));
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expanded_paths_resolve_through_actual_guest_directory_owner() {
        use std::ffi::{CStr, CString, c_char};
        use std::fs::{self, File};
        use std::os::fd::FromRawFd;
        unsafe extern "C" {
            fn darwin_art_fs_open_native_directory(
                path: *const c_char,
                canonical: *mut c_char,
                capacity: usize,
                fd: *mut i32,
            ) -> i32;
        }
        let root = std::env::temp_dir().join(format!(
            "darwin-runpath-dir-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("apps/bin")).unwrap();
        fs::create_dir_all(root.join("apps/lib64")).unwrap();
        fs::create_dir(root.join("relative")).unwrap();
        fs::write(root.join("file"), b"not-directory").unwrap();
        let facade = Arc::new(
            bionic_fs_facade::Facade::new(File::open(&root).unwrap(), b"/", b"/").unwrap(),
        );
        let active = facade.activate();
        let result = resolve_runpath_directories(
            b"$ORIGIN/../$LIB:/missing:/file:relative",
            Path::new("/apps/bin/root.so"),
            |path| {
                let path = CString::new(path.as_os_str().as_bytes()).unwrap();
                let mut canonical = [0i8; 4096];
                let mut fd = -1;
                if unsafe {
                    darwin_art_fs_open_native_directory(
                        path.as_ptr(),
                        canonical.as_mut_ptr(),
                        canonical.len(),
                        &mut fd,
                    )
                } != 0
                {
                    return None;
                }
                let file = unsafe { File::from_raw_fd(fd) };
                let path = unsafe { CStr::from_ptr(canonical.as_ptr()) }
                    .to_bytes()
                    .to_vec();
                Some((PathBuf::from(std::ffi::OsString::from_vec(path)), file))
            },
        )
        .unwrap();
        assert_eq!(
            result,
            vec![PathBuf::from("/apps/lib64"), PathBuf::from("/relative")]
        );
        drop(active);
        drop(facade);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn android_tokens_preserve_bytes_and_do_not_expand_replacements() {
        let paths = expand_runpath(
            b":$ORIGIN/../${LIB}:$LIB:$PLATFORM:$ORIGIN_SUFFIX:/raw/\xff::",
            Path::new("/apps/$LIB/libroot.so"),
        )
        .unwrap();
        let bytes: Vec<&[u8]> = paths
            .iter()
            .map(|path| path.as_os_str().as_bytes())
            .collect();
        assert_eq!(
            bytes,
            vec![
                &b"/apps/$LIB/../lib64"[..],
                b"lib64",
                b"$PLATFORM",
                b"/apps/$LIB_SUFFIX",
                b"/raw/\xff"
            ]
        );
        assert!(expand_runpath(b"bad\0path", Path::new("/app/root.so")).is_err());
        assert!(expand_runpath(b"$ORIGIN", Path::new("relative.so")).is_err());
        assert!(expand_runpath(b"$ORIGIN", Path::new("/")).is_err());
        assert!(
            expand_runpath(b"", Path::new("/root.so"))
                .unwrap()
                .is_empty()
        );
    }
}
