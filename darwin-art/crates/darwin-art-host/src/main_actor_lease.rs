//! Exclusive process-main-thread admission for one macOS actor session.

use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MainActorAdmissionError {
    NotMainThread,
    SessionAlreadyActive,
}

/// Neither transferable nor shareable: only the admitting main thread may
/// release the process actor obligation.
pub(crate) struct MainActorLease(std::marker::PhantomData<std::rc::Rc<()>>);

impl MainActorLease {
    pub(crate) fn acquire() -> Result<Self, MainActorAdmissionError> {
        unsafe extern "C" {
            fn pthread_main_np() -> i32;
        }
        // SAFETY: this process-local Darwin query has no borrowed inputs.
        if unsafe { pthread_main_np() } == 0 {
            return Err(MainActorAdmissionError::NotMainThread);
        }
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| MainActorAdmissionError::SessionAlreadyActive)?;
        Ok(Self(std::marker::PhantomData))
    }
}

impl Drop for MainActorLease {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
