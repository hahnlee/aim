//! Darwin process boundary: quiesce tag mutations before fork, unlock in both
//! processes, and preserve the child's copy instead of discarding ownership.
use super::Tags;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::mem::MaybeUninit;
use std::sync::Once;

struct Store {
    mutex: UnsafeCell<MaybeUninit<libc::pthread_mutex_t>>,
    tags: UnsafeCell<Tags>,
}
// All tag accesses are serialized by this native mutex. Initialization is
// process startup work; fork callbacks are registered only after it is ready.
unsafe impl Sync for Store {}
static STORE: Store = Store {
    mutex: UnsafeCell::new(MaybeUninit::uninit()),
    tags: UnsafeCell::new(Tags(BTreeMap::new())),
};
static INITIALIZE: Once = Once::new();
fn mutex() -> *mut libc::pthread_mutex_t {
    STORE.mutex.get().cast()
}
fn check(result: i32) {
    if result != 0 {
        std::process::abort();
    }
}
unsafe extern "C" fn prepare() {
    check(unsafe { libc::pthread_mutex_lock(mutex()) });
}
unsafe extern "C" fn release() {
    check(unsafe { libc::pthread_mutex_unlock(mutex()) });
}
pub(super) fn initialize() {
    INITIALIZE.call_once(|| unsafe {
        let mut attributes = MaybeUninit::uninit();
        check(libc::pthread_mutexattr_init(attributes.as_mut_ptr()));
        let mut attributes = attributes.assume_init();
        // Register at process activation before guest atfork callbacks. A normal
        // mutex can be released by the surviving fork thread in the child;
        // Darwin recursive mutexes retain parent thread-owner bookkeeping.
        check(libc::pthread_mutexattr_settype(
            &mut attributes,
            libc::PTHREAD_MUTEX_NORMAL,
        ));
        check(libc::pthread_mutex_init(mutex(), &attributes));
        check(libc::pthread_mutexattr_destroy(&mut attributes));
        check(libc::pthread_atfork(
            Some(prepare),
            Some(release),
            Some(release),
        ));
    });
}
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            release();
        }
    }
}
pub(super) fn with_tags<R>(operation: impl FnOnce(&mut Tags) -> R) -> R {
    initialize();
    unsafe {
        prepare();
    }
    let _guard = Guard;
    // No pointer/reference escapes this call; callbacks never retain tag data.
    operation(unsafe { &mut *STORE.tags.get() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    static PHASES: AtomicU32 = AtomicU32::new(0);
    extern "C" fn user_prepare() {
        with_tags(|tags| tags.exchange(9043, 0, 0).unwrap());
        PHASES.fetch_or(1, Ordering::SeqCst);
    }
    extern "C" fn user_parent() {
        with_tags(|tags| tags.exchange(9043, 0, 0).unwrap());
        PHASES.fetch_or(2, Ordering::SeqCst);
    }
    extern "C" fn user_child() {
        with_tags(|tags| tags.exchange(9043, 0, 0).unwrap());
        PHASES.fetch_or(4, Ordering::SeqCst);
    }
    fn exercise_fork(child_phases: u32, parent_phases: u32) {
        with_tags(|tags| tags.exchange(9042, 0, 77).unwrap());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            with_tags(|_| {
                ready_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(150));
            })
        });
        ready_rx.recv().unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            unsafe {
                libc::alarm(3);
            }
            let success = with_tags(|tags| tags.exchange(9042, 77, 99).is_ok())
                && PHASES.load(Ordering::SeqCst) == child_phases;
            unsafe {
                libc::_exit(if success { 0 } else { 91 });
            }
        }
        worker.join().unwrap();
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status), "child wait status: {status}");
        assert_eq!(libc::WEXITSTATUS(status), 0);
        assert_eq!(PHASES.load(Ordering::SeqCst), parent_phases);
        with_tags(|tags| tags.exchange(9042, 77, 0).unwrap());
    }
    #[test]
    fn fork_waits_for_active_writer_and_child_keeps_independent_tags() {
        initialize();
        // First prove our prepare handler alone waits for the other writer.
        exercise_fork(0, 0);
        assert_eq!(
            unsafe {
                libc::pthread_atfork(Some(user_prepare), Some(user_parent), Some(user_child))
            },
            0
        );
        // Then verify registered guest callbacks can access tags in all phases.
        exercise_fork(5, 3);
    }
}
