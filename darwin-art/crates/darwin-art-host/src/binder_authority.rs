//! C-ABI adapter for the exact Binder authority lifetime of one app process.
//!
//! The hook table is borrowed only for the synchronous native process call.
//! Each retain callback creates a boxed clone of the atomic-only lifetime
//! metadata; no transport, Binder, or JNI operation is performed here.

use darwin_art_abi::AbiHeader;
use darwin_art_binder_process::AuthorityLifetime;
use darwin_art_engine_sys::{BINDER_AUTHORITY_HOOKS_ABI_VERSION, BinderAuthorityHooks};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub(crate) struct BinderAuthorityBinding {
    owner: Box<AuthorityLifetime>,
    hooks: BinderAuthorityHooks,
}

impl BinderAuthorityBinding {
    pub(crate) fn new(owner: AuthorityLifetime) -> Self {
        let mut binding = Self {
            owner: Box::new(owner),
            hooks: BinderAuthorityHooks {
                header: AbiHeader {
                    struct_size: std::mem::size_of::<BinderAuthorityHooks>() as u32,
                    abi_version: BINDER_AUTHORITY_HOOKS_ABI_VERSION,
                },
                context: std::ptr::null_mut(),
                retain: Some(retain),
                live: Some(live),
                release: Some(release),
            },
        };
        binding.hooks.context = (&*binding.owner as *const AuthorityLifetime)
            .cast_mut()
            .cast();
        binding
    }

    pub(crate) fn hooks(&self) -> &BinderAuthorityHooks {
        &self.hooks
    }
}

unsafe extern "C" fn retain(context: *mut c_void) -> *mut c_void {
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: native passes the stable context from BinderAuthorityBinding
        // for the duration of the borrowed hook table.
        let source = unsafe { &*context.cast::<AuthorityLifetime>() };
        Box::into_raw(Box::new(source.clone())).cast()
    }))
    .unwrap_or(std::ptr::null_mut())
}

unsafe extern "C" fn live(retained: *mut c_void) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if retained.is_null() {
            return 0;
        }
        // SAFETY: retained was returned by retain and remains owned by native
        // until the matching release callback.
        let lifetime = unsafe { &*retained.cast::<AuthorityLifetime>() };
        i32::from(lifetime.live())
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn release(retained: *mut c_void) {
    if retained.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: retained is an exact allocation produced by retain and is
        // released at most once by the native owner.
        drop(unsafe { Box::from_raw(retained.cast::<AuthorityLifetime>()) });
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use darwin_art_binder_device::{
        authority_protocol::{ConnectionToken, Message, TransferToken},
        device::{Device, ProcessIdentity},
        transfer_image::TransferImage,
    };
    use darwin_art_binder_process::{AuthorityTransport, Client, Dispatcher, TransportError};
    use std::sync::{Mutex, mpsc};

    struct FakeTransport {
        connection: ConnectionToken,
        outgoing: mpsc::SyncSender<Message>,
        incoming: Mutex<mpsc::Receiver<Message>>,
    }

    impl AuthorityTransport for FakeTransport {
        fn connection(&self) -> ConnectionToken {
            self.connection
        }

        fn android_uid(&self) -> u32 {
            1000
        }

        fn send(&self, message: Message) -> Result<(), TransportError> {
            self.outgoing
                .send(message)
                .map_err(|_| TransportError("outgoing closed".into()))
        }

        fn receive(&self) -> Result<Message, TransportError> {
            self.incoming
                .lock()
                .map_err(|_| TransportError("incoming poisoned".into()))?
                .recv()
                .map_err(|_| TransportError("incoming closed".into()))
        }

        fn deposit_transfer(
            &self,
            _: TransferToken,
            _: &TransferImage,
        ) -> Result<(), TransportError> {
            Ok(())
        }

        fn take_transfer(
            &self,
            _: ConnectionToken,
            _: TransferToken,
        ) -> Result<TransferImage, TransportError> {
            Err(TransportError("test has no inbound transfer".into()))
        }
    }

    fn endpoint() -> (
        Client<FakeTransport>,
        Dispatcher<FakeTransport>,
        mpsc::Receiver<Message>,
        mpsc::SyncSender<Message>,
    ) {
        let (outgoing_tx, outgoing_rx) = mpsc::sync_channel(1);
        let (incoming_tx, incoming_rx) = mpsc::sync_channel(1);
        let transport = FakeTransport {
            connection: ConnectionToken::from_nonzero(4).unwrap(),
            outgoing: outgoing_tx,
            incoming: Mutex::new(incoming_rx),
        };
        let device = Device::default()
            .open(ProcessIdentity::new(91, 1000).unwrap())
            .unwrap();
        let (client, dispatcher) = Client::new(transport, device);
        (client, dispatcher, outgoing_rx, incoming_tx)
    }

    #[test]
    fn binding_context_survives_move_and_owner_drop() {
        let (client, _dispatcher, _outgoing, _incoming) = endpoint();
        let mut binding = Some(BinderAuthorityBinding::new(client.authority_lifetime()));
        let context = binding.as_ref().unwrap().hooks().context;
        let retained = unsafe { (binding.as_ref().unwrap().hooks().retain.unwrap())(context) };
        assert!(!retained.is_null());
        assert_eq!(
            unsafe { (binding.as_ref().unwrap().hooks().live.unwrap())(retained) },
            1
        );

        let moved = binding.take().unwrap();
        let moved_context = moved.hooks().context;
        assert_eq!(context, moved_context);
        drop(moved);

        // The retained clone owns only terminal metadata and therefore remains
        // valid after the hook-table owner is moved and dropped.
        assert_eq!(unsafe { live(retained) }, 1);
        unsafe { release(retained) };
        drop(client);
    }

    #[test]
    fn close_and_dispatcher_eof_make_retained_token_sticky_dead() {
        let (client, _dispatcher, outgoing, _incoming) = endpoint();
        let binding = BinderAuthorityBinding::new(client.authority_lifetime());
        let context = binding.hooks().context;
        let retained = unsafe { retain(context) };
        assert_eq!(unsafe { live(retained) }, 1);

        client.close_authority().unwrap();
        assert_eq!(outgoing.recv().unwrap(), Message::CloseConnection);
        assert_eq!(unsafe { live(retained) }, 0);
        assert_eq!(unsafe { live(retained) }, 0);
        unsafe { release(retained) };

        let (client, dispatcher, _outgoing, incoming) = endpoint();
        let binding = BinderAuthorityBinding::new(client.authority_lifetime());
        let retained = unsafe { retain(binding.hooks().context) };
        drop(incoming);
        assert!(dispatcher.dispatch_next().is_err());
        assert_eq!(unsafe { live(retained) }, 0);
        unsafe { release(retained) };
    }

    #[test]
    fn callbacks_reject_null_and_release_owned_clone_once() {
        assert!(unsafe { retain(std::ptr::null_mut()) }.is_null());
        assert_eq!(unsafe { live(std::ptr::null_mut()) }, 0);
        unsafe { release(std::ptr::null_mut()) };

        let (client, _dispatcher, _outgoing, _incoming) = endpoint();
        let binding = BinderAuthorityBinding::new(client.authority_lifetime());
        let retained = unsafe { retain(binding.hooks().context) };
        assert!(!retained.is_null());
        unsafe { release(retained) };
    }
}
