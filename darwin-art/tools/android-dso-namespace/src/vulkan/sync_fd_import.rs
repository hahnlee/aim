//! Ownership bridge for Vulkan's consuming SYNC_FD import operation.

pub(super) const VK_ERROR_FORMAT_NOT_SUPPORTED: i32 = -11;

// vkImportSemaphoreFdKHR consumes a valid SYNC_FD handle. The application
// still owns its original guest descriptor until a successful provider import,
// so import through a broker duplicate and close the original only on success.
pub(super) fn import_consuming_sync_fd<D, I, C>(
    fd: i32,
    mut duplicate: D,
    mut import: I,
    mut close: C,
) -> i32
where
    D: FnMut(i32) -> i32,
    I: FnMut(i32) -> i32,
    C: FnMut(i32) -> i32,
{
    // The Android sync-fence convention uses -1 for an already-signaled
    // fence. The provider explicitly accepts that sentinel; never duplicate
    // or close it as if it were a guest descriptor.
    if fd == -1 {
        return import(fd);
    }
    if fd < -1 {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let imported_fd = duplicate(fd);
    if imported_fd < 0 || imported_fd == fd {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let result = import(imported_fd);
    if result == 0 {
        let _ = close(fd);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{import_consuming_sync_fd, VK_ERROR_FORMAT_NOT_SUPPORTED};

    #[test]
    fn success_closes_only_original_after_duplicate_import() {
        let mut imported = Vec::new();
        let mut closed = Vec::new();
        assert_eq!(
            import_consuming_sync_fd(
                7,
                |fd| fd + 100,
                |fd| {
                    imported.push(fd);
                    0
                },
                |fd| {
                    closed.push(fd);
                    0
                },
            ),
            0
        );
        assert_eq!(imported, vec![107]);
        assert_eq!(closed, vec![7]);
    }

    #[test]
    fn provider_failure_leaves_original_owned() {
        let mut imported = Vec::new();
        let mut closed = Vec::new();
        assert_eq!(
            import_consuming_sync_fd(
                8,
                |fd| fd + 100,
                |fd| {
                    imported.push(fd);
                    -3
                },
                |fd| {
                    closed.push(fd);
                    0
                },
            ),
            -3
        );
        assert_eq!(imported, vec![108]);
        assert!(closed.is_empty());
    }

    #[test]
    fn duplicate_failure_does_not_call_provider_or_close_original() {
        let mut imported = 0;
        let mut closed = 0;
        assert_eq!(
            import_consuming_sync_fd(
                9,
                |_| -1,
                |_| {
                    imported += 1;
                    0
                },
                |_| {
                    closed += 1;
                    0
                },
            ),
            VK_ERROR_FORMAT_NOT_SUPPORTED
        );
        assert_eq!(imported, 0);
        assert_eq!(closed, 0);
    }

    #[test]
    fn sentinel_is_passed_without_duplicate_or_close() {
        let mut imported = Vec::new();
        let mut duplicated = 0;
        let mut closed = 0;
        assert_eq!(
            import_consuming_sync_fd(
                -1,
                |_| {
                    duplicated += 1;
                    0
                },
                |fd| {
                    imported.push(fd);
                    0
                },
                |_| {
                    closed += 1;
                    0
                },
            ),
            0
        );
        assert_eq!(imported, vec![-1]);
        assert_eq!(duplicated, 0);
        assert_eq!(closed, 0);
    }
}
