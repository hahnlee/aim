//! Receiver binder_proc descriptors retained by the transaction buffer.
//! The process boundary installs each descriptor; this owner closes it exactly
//! when BC_FREE_BUFFER, cancellation, failure, or process teardown releases the
//! corresponding buffer.

pub struct InstalledFd {
    offset: usize,
    object: [u8; 24],
    number: u32,
    close: Option<CloseAction>,
}

enum CloseAction {
    Closure(Box<dyn FnOnce() + Send>),
    Native(unsafe extern "C" fn(i32) -> i32),
}

impl InstalledFd {
    pub fn new(
        offset: usize,
        object: [u8; 24],
        number: u32,
        close: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            offset,
            object,
            number,
            close: Some(CloseAction::Closure(Box::new(close))),
        }
    }

    /// Own one provider descriptor without allocating a callback after claim.
    ///
    /// # Safety
    /// `close` must accept this owned descriptor number, must not unwind, and
    /// its provider/code must remain live until this owner is dropped. The
    /// caller transfers descriptor ownership exactly once.
    pub unsafe fn new_native(
        offset: usize,
        object: [u8; 24],
        number: u32,
        close: unsafe extern "C" fn(i32) -> i32,
    ) -> Self {
        Self {
            offset,
            object,
            number,
            close: Some(CloseAction::Native(close)),
        }
    }

    pub fn number(&self) -> u32 {
        self.number
    }

    pub(crate) fn rewrite(&self) -> crate::transaction_objects::Rewrite {
        crate::transaction_objects::Rewrite::replace_object(self.offset, self.object)
    }
}

impl Drop for InstalledFd {
    fn drop(&mut self) {
        if let Some(close) = self.close.take() {
            match close {
                CloseAction::Closure(close) => close(),
                CloseAction::Native(close) => {
                    // SAFETY: new_native transfers the callback lifetime and
                    // owned-number contract; taking the action ensures once.
                    let _ = unsafe { close(self.number as i32) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CLOSES: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn close(number: i32) -> i32 {
        assert_eq!(number, 42);
        CLOSES.fetch_add(1, Ordering::SeqCst);
        0
    }
    #[test]
    fn native_callback_ownership_survives_moves_and_closes_once() {
        CLOSES.store(0, Ordering::SeqCst);
        let fd = unsafe { InstalledFd::new_native(0, [0; 24], 42, close) };
        let fds = vec![fd];
        assert_eq!(CLOSES.load(Ordering::SeqCst), 0);
        drop(fds);
        assert_eq!(CLOSES.load(Ordering::SeqCst), 1);
    }
}
