//! Atomic resolved search-path replacement; path parsing/resolution stays AOSP.
use super::*;

/// # Safety
/// Live registry, NUL-terminated resolved guest paths (or null to clear).
/// Caller holds linker operation across AOSP resolution and this mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_replace_search_paths(
    registry: *mut LinkerRegistry,
    id: u64,
    resolved: *const c_char,
) -> i32 {
    if registry.is_null() {
        return -1;
    }
    let paths = unsafe { paths(resolved) };
    let Ok(mut owner) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    match owner.replace_search_paths(NamespaceId::from_raw(id), paths) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_is_atomic_and_preserves_defaults_and_shared_children() {
        let mut owner = NamespaceRegistry::<()>::default();
        let parent = owner
            .create(
                NamespaceConfig {
                    search_paths: vec!["/old".into()],
                    default_search_paths: vec!["/default".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let child = owner
            .create(NamespaceConfig::default(), Some(parent))
            .unwrap();
        owner
            .replace_search_paths(parent, vec!["/new".into()])
            .unwrap();
        assert!(
            owner
                .replace_search_paths(parent, vec!["relative".into()])
                .is_err()
        );
        let mut visited = Vec::new();
        let plan = owner.search_plan(parent, "libx.so", &[]).unwrap();
        let _ = plan.open(|path| {
            visited.push(path.to_path_buf());
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert_eq!(
            visited,
            vec![
                PathBuf::from("/new/libx.so"),
                PathBuf::from("/default/libx.so")
            ]
        );
        visited.clear();
        let _ = owner
            .search_plan(child, "libx.so", &[])
            .unwrap()
            .open(|path| {
                visited.push(path.to_path_buf());
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            });
        assert_eq!(
            visited,
            vec![
                PathBuf::from("/old/libx.so"),
                PathBuf::from("/default/libx.so")
            ]
        );
        owner.replace_search_paths(parent, vec![]).unwrap();
        assert_eq!(
            owner.default_library_paths(parent).unwrap(),
            vec![PathBuf::from("/default")]
        );
    }
}
