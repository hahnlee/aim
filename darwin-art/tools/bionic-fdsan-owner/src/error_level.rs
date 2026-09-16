//! Process-wide FDSAN policy. Descriptor ownership remains in process_store.
//! Values match Android's android_fdsan_error_level ABI.
use std::sync::atomic::{AtomicI32, Ordering};

struct Policy(AtomicI32);
impl Policy {
    const fn new(level: i32) -> Self {
        Self(AtomicI32::new(level))
    }
    fn get(&self) -> i32 {
        self.0.load(Ordering::SeqCst)
    }
    fn set(&self, level: i32) -> i32 {
        self.0.swap(level, Ordering::SeqCst)
    }
    fn report(&self) -> i32 {
        let level = self.get();
        if level == 1 {
            // Preserve a concurrent explicit policy update, as AOSP's CAS does.
            let _ = self
                .0
                .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
        level
    }
}
static POLICY: Policy = Policy::new(3);

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_fdsan_get_error_level() -> i32 {
    POLICY.get()
}
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_fdsan_set_error_level(level: i32) -> i32 {
    POLICY.set(level)
}
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_fdsan_report_error_level() -> i32 {
    POLICY.report()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn levels_and_previous_value() {
        let p = Policy::new(3);
        assert_eq!(p.report(), 3);
        assert_eq!(p.set(1), 3);
        assert_eq!(p.report(), 1);
        assert_eq!(p.get(), 0);
        assert_eq!(p.report(), 0);
        assert_eq!(p.set(2), 0);
        assert_eq!(p.report(), 2);
        assert_eq!(p.report(), 2);
        assert_eq!(p.get(), 2);
    }
}
