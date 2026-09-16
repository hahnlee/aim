//! Receiver binder_proc descriptors retained by the transaction buffer.
//! The process boundary installs each descriptor; this owner closes it exactly
//! when BC_FREE_BUFFER, cancellation, failure, or process teardown releases the
//! corresponding buffer.

pub struct InstalledFd {
    offset: usize,
    object: [u8; 24],
    number: u32,
    close: Option<Box<dyn FnOnce() + Send>>,
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
            close: Some(Box::new(close)),
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
            close();
        }
    }
}
