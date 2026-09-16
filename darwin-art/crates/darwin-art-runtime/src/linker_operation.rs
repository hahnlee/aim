//! Recursive linker-operation serialization, distinct from registry data locks.
//! Equivalent ownership scope to Bionic g_dl_mutex: foreign code runs while the
//! operation is owned, but no Rust MutexGuard crosses that code.
use super::*;
use std::{marker::PhantomData, rc::Rc, sync::Condvar, thread::ThreadId};
#[derive(Default)]
struct State {
    owner: Option<ThreadId>,
    depth: usize,
}
#[derive(Default)]
pub(super) struct OperationGate {
    state: Mutex<State>,
    changed: Condvar,
}
pub struct LinkerOperation {
    gate: Arc<OperationGate>,
    owner: ThreadId,
    _thread_bound: PhantomData<Rc<()>>,
}
impl OperationGate {
    fn enter(self: &Arc<Self>) -> Option<LinkerOperation> {
        let owner = std::thread::current().id();
        let mut state = self.state.lock().ok()?;
        while state.owner.is_some() && state.owner != Some(owner) {
            state = self.changed.wait(state).ok()?;
        }
        state.depth = state.depth.checked_add(1)?;
        state.owner = Some(owner);
        Some(LinkerOperation {
            gate: Arc::clone(self),
            owner,
            _thread_bound: PhantomData,
        })
    }
}
impl Drop for LinkerOperation {
    fn drop(&mut self) {
        assert_eq!(self.owner, std::thread::current().id());
        let mut state = self.gate.state.lock().expect("operation owner state");
        assert_eq!(state.owner, Some(self.owner));
        state.depth -= 1;
        if state.depth == 0 {
            state.owner = None;
            self.gate.changed.notify_one();
        }
    }
}
/// # Safety
/// Registry live during entry. Return token must be left once on this thread.
/// Shutdown drains operations before destroying registry. Returns NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_operation_enter(
    registry: *const LinkerRegistry,
) -> *mut LinkerOperation {
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return std::ptr::null_mut();
    };
    registry
        .1
        .enter()
        .map(|token| Box::into_raw(Box::new(token)))
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// Live token, no simultaneous use. Wrong-thread leave rejects without consuming
/// the token, so its owning thread must still release it. Never call twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_operation_leave(token: *mut LinkerOperation) -> i32 {
    let Some(operation) = (unsafe { token.as_ref() }) else {
        return -1;
    };
    if operation.owner != std::thread::current().id() {
        return -1;
    }
    drop(unsafe { Box::from_raw(token) });
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    #[test]
    fn nested_owner_excludes_other_thread_until_outer_release() {
        let gate = Arc::new(OperationGate::default());
        let outer = gate.enter().unwrap();
        let nested = gate.enter().unwrap();
        let other = gate.clone();
        let (started, waiting) = mpsc::channel();
        let (entered, observed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            let _token = other.enter().unwrap();
            entered.send(()).unwrap();
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(observed.recv_timeout(Duration::from_millis(20)).is_err());
        drop(nested);
        assert!(observed.recv_timeout(Duration::from_millis(20)).is_err());
        drop(outer);
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
