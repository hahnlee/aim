//! Rust ownership behind the private native endpoint installer ABI.
//! It owns authenticated transfer preparation/admission/settlement; guest
//! publication remains with the native broker owner.
use super::{client_transport, failed, wire::Request};
use crate::ProfileError;
use darwin_art_engine_sys::*;
use std::{ffi::c_void, io::Write, os::fd::IntoRawFd, path::PathBuf, sync::Arc};

#[path = "native_binder_endpoint.rs"]
mod binder_native;
#[path = "native_endpoint_metadata.rs"]
mod native_endpoint_metadata;
#[path = "native_endpoint_ops.rs"]
mod native_endpoint_ops;

struct Context {
    socket: PathBuf,
}

pub struct NativeScmEndpointProvider {
    context: Arc<Context>,
}

impl NativeScmEndpointProvider {
    pub fn new(socket: PathBuf) -> Result<Self, ProfileError> {
        if !socket.is_absolute() {
            return Err(failed("provider requires trusted absolute profile socket"));
        }
        Ok(Self {
            context: Arc::new(Context { socket }),
        })
    }

    /// Raw table borrows this owner until native installation retains it.
    /// It alone cannot extend lifetime; all callbacks are unsafe private ABI.
    pub fn hooks(&self) -> ScmEndpointProviderV1 {
        ScmEndpointProviderV1 {
            abi_version: SCM_ENDPOINT_ABI_VERSION,
            struct_size: size_of::<ScmEndpointProviderV1>() as u32,
            context: Arc::as_ptr(&self.context).cast_mut().cast(),
            retain: Some(retain),
            release: Some(release),
            register_pair: Some(register_pair),
            release_holder: Some(release_holder),
            prepare: Some(prepare),
            admit: Some(admit),
            settle: Some(settle),
            bind_binder: Some(binder_native::bind_binder),
            cancel_binder: Some(binder_native::cancel_binder),
            claim_binder: Some(binder_native::claim_binder),
        }
    }
}

// All pointers originate at the trusted runtime installer. No guest pointer,
// PID, native socket identity or path is accepted by operation callbacks.
unsafe extern "C" fn retain(context: *mut c_void) -> *mut c_void {
    if context.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { Arc::increment_strong_count(context.cast::<Context>()) };
    context
}
unsafe extern "C" fn release(context: *mut c_void) {
    if !context.is_null() {
        unsafe { Arc::decrement_strong_count(context.cast::<Context>()) };
    }
}

fn status(operation: impl FnOnce() -> Result<(), ProfileError>) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(())) => 0,
        Ok(Err(ProfileError::Io(error))) => error.raw_os_error().unwrap_or(libc::EIO),
        Ok(Err(_)) => libc::EPROTO,
        Err(_) => libc::EIO,
    }
}

struct Installation<'a> {
    context: &'a Context,
    installer: ScmPairInstallerV1,
    holders: [u128; 2],
    armed: bool,
}
impl Drop for Installation<'_> {
    fn drop(&mut self) {
        if self.armed {
            // Clear partial native state first. No RPC or grant release in it.
            unsafe { (self.installer.clear.unwrap())(self.installer.target) };
            // Explicit release also covers a lost final confirmation AFTER the
            // daemon committed. Its connection-EOF rollback is insufficient.
            for holder in self.holders {
                if let Err(error) = release_id(self.context, holder) {
                    // Preserve the initiating error, but never report cleanup
                    // failure as success or hide a possibly committed grant.
                    let _ = writeln!(std::io::stderr().lock(), "SCM endpoint rollback: {error}");
                }
            }
        }
    }
}

unsafe extern "C" fn register_pair(context: *mut c_void, target: *const ScmPairInstallerV1) -> i32 {
    if context.is_null() || target.is_null() {
        return libc::EINVAL;
    }
    // Private caller guarantees aligned/readable target for this synchronous
    // operation and retained context/code for every callback until return.
    let installer = unsafe { *target };
    if installer.abi_version != SCM_ENDPOINT_ABI_VERSION
        || installer.struct_size != size_of::<ScmPairInstallerV1>() as u32
        || installer.target.is_null()
        || installer.install.is_none()
        || installer.clear.is_none()
    {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    status(|| {
        let stream = client_transport::connect(&context.socket)?;
        let mut installed = client_transport::register_pair(stream, |offer| {
            let receipt = Installation {
                context,
                installer,
                holders: [offer.holder_a, offer.holder_b],
                armed: true,
            };
            let raw = ScmPairOfferV1 {
                authority: offer.authority.to_le_bytes(),
                carrier: offer.carrier,
                holder_a: offer.holder_a.to_le_bytes(),
                holder_b: offer.holder_b.to_le_bytes(),
            };
            let result = unsafe { installer.install.unwrap()(installer.target, &raw) };
            if result != 0 {
                return Err(std::io::Error::from_raw_os_error(if result > 0 {
                    result
                } else {
                    libc::EIO
                })
                .into());
            }
            Ok(receipt)
        })?;
        // Caller-owned native receipt STILL owns rollback through publication.
        // Only daemon-grant responsibility transfers to the installed objects.
        installed.armed = false;
        Ok(())
    })
}

fn release_id(context: &Context, holder: u128) -> Result<(), ProfileError> {
    client_transport::settle_or_release(
        client_transport::connect(&context.socket)?,
        &Request::ReleaseHolder { holder },
    )
}

unsafe extern "C" fn release_holder(context: *mut c_void, holder: *const u8) -> i32 {
    if context.is_null() || holder.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let mut bytes = [0; 16];
    unsafe { std::ptr::copy_nonoverlapping(holder, bytes.as_mut_ptr(), bytes.len()) };
    status(|| release_id(context, u128::from_le_bytes(bytes)))
}

unsafe extern "C" fn prepare(
    context: *mut c_void,
    request: *const ScmPrepareRequestV2,
    output: *mut ScmPreparedV2,
) -> i32 {
    if output.is_null() {
        return libc::EINVAL;
    }
    unsafe {
        *output = ScmPreparedV2 {
            authority: [0; 16],
            ticket: 0,
            metadata_fd: -1,
            guardian_fd: -1,
            payload_count: 0,
            reserved: 0,
        };
    }
    if context.is_null() || request.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let request = unsafe { &*request };
    if request.payload_count as usize > SCM_MAX_PAYLOADS
        || (request.payload_count != 0 && request.payload_fds.is_null())
        || request.managed_count as usize > SCM_MAX_PAYLOADS
        || (request.managed_count != 0 && request.managed.is_null())
    {
        return libc::EINVAL;
    }
    status(|| {
        let payloads = if request.payload_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(request.payload_fds, request.payload_count as usize) }
        };
        let managed_raw = if request.managed_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(request.managed, request.managed_count as usize) }
        };
        let mut managed = Vec::new();
        managed
            .try_reserve_exact(managed_raw.len())
            .map_err(|_| failed("native managed manifest allocation failed"))?;
        managed.extend(
            managed_raw
                .iter()
                .map(|entry| (entry.ordinal, u128::from_le_bytes(entry.holder))),
        );
        let prepared = native_endpoint_ops::prepare(
            &context.socket,
            u128::from_le_bytes(request.carrier_holder),
            payloads,
            &managed,
        )?;
        let metadata_fd = prepared.metadata.into_raw_fd();
        let guardian_fd = prepared.guardian.into_raw_fd();
        unsafe {
            *output = ScmPreparedV2 {
                authority: prepared.authority,
                ticket: prepared.ticket,
                metadata_fd,
                guardian_fd,
                payload_count: prepared.payload_count,
                reserved: 0,
            };
        }
        Ok(())
    })
}

unsafe extern "C" fn admit(
    context: *mut c_void,
    request: *const ScmAdmitRequestV2,
    output: *mut ScmAdmissionV2,
) -> i32 {
    if output.is_null() {
        return libc::EINVAL;
    }
    unsafe {
        *output = ScmAdmissionV2::default();
    }
    if context.is_null() || request.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let request = unsafe { &*request };
    if request.reserved != 0
        || request.metadata_fd < 0
        || request.payload_count as usize > SCM_MAX_PAYLOADS
        || request.publish_count as usize > SCM_MAX_PAYLOADS
        || (request.publish_count != 0 && request.publish_ordinals.is_null())
    {
        return libc::EINVAL;
    }
    status(|| {
        let ordinals = if request.publish_count == 0 {
            &[][..]
        } else {
            unsafe {
                std::slice::from_raw_parts(request.publish_ordinals, request.publish_count as usize)
            }
        };
        let admission = native_endpoint_ops::admit(
            &context.socket,
            u128::from_le_bytes(request.carrier_holder),
            request.metadata_fd,
            request.payload_count as usize,
            ordinals,
        )?;
        if admission.claims.len() > SCM_MAX_PAYLOADS {
            return Err(failed("native admission claim bound exceeded"));
        }
        unsafe {
            (*output).authority = admission.authority;
            (*output).ticket = admission.ticket;
            (*output).claim_count = admission.claims.len() as u32;
            (*output).credentials = ScmCredentialsV2 {
                process_id: admission.credentials.pid,
                user_id: admission.credentials.uid,
                group_id: admission.credentials.gid,
            };
            for (slot, claim) in (*output).claims.iter_mut().zip(admission.claims) {
                *slot = ScmClaimV2 {
                    ordinal: claim.ordinal,
                    grant: ScmGrantV2 {
                        authority: claim.authority,
                        carrier: claim.carrier,
                        holder: claim.holder,
                        side: claim.side,
                        reserved: 0,
                    },
                };
            }
        }
        Ok(())
    })
}

unsafe extern "C" fn settle(
    context: *mut c_void,
    authority: *const u8,
    ticket: u64,
    disposition: u32,
) -> i32 {
    if context.is_null() || authority.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let mut authority_bytes = [0; 16];
    unsafe {
        std::ptr::copy_nonoverlapping(authority, authority_bytes.as_mut_ptr(), 16);
    }
    status(|| native_endpoint_ops::settle(&context.socket, authority_bytes, ticket, disposition))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process_incarnation::ProcessIncarnation, process_registry::ProcessRegistry, protocol,
        scm_service::ScmService,
    };
    use std::{
        fs::File,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::net::UnixListener,
        },
        sync::{Arc, Mutex},
        thread::{self, JoinHandle},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct Fixture {
        path: PathBuf,
        listener: UnixListener,
        service: Arc<ScmService>,
        processes: Arc<Mutex<ProcessRegistry>>,
    }

    impl Fixture {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "darwin-art-native-scm-{}-{suffix}.sock",
                std::process::id()
            ));
            let listener = UnixListener::bind(&path).unwrap();
            let birth = ProcessIncarnation::read_live(std::process::id()).unwrap();
            let mut registry = ProcessRegistry::default();
            registry
                .acquire(std::process::id(), "org.example.native.scm", birth, false)
                .unwrap();
            Self {
                path,
                listener,
                service: Arc::new(ScmService::new().unwrap()),
                processes: Arc::new(Mutex::new(registry)),
            }
        }

        fn server(&self) -> JoinHandle<Result<(), ProfileError>> {
            let listener = self.listener.try_clone().unwrap();
            let service = self.service.clone();
            let processes = self.processes.clone();
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let request = protocol::read_message(&mut stream)?;
                service.serve(&processes, &mut stream, &request.payload)
            })
        }

        fn wait_transfer_idle(&self) {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if !self.service.owner.lock().unwrap().has_pending().unwrap() {
                    return;
                }
                assert!(Instant::now() < deadline, "guardian aliases did not retire");
                thread::sleep(Duration::from_millis(2));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[repr(C)]
    struct PairState {
        holder_a: u128,
        holder_b: u128,
    }

    unsafe extern "C" fn capture_pair(target: *mut c_void, offer: *const ScmPairOfferV1) -> i32 {
        if target.is_null() || offer.is_null() {
            return libc::EINVAL;
        }
        let offer = unsafe { &*offer };
        let state = unsafe { &mut *target.cast::<PairState>() };
        state.holder_a = u128::from_le_bytes(offer.holder_a);
        state.holder_b = u128::from_le_bytes(offer.holder_b);
        0
    }

    unsafe extern "C" fn clear_pair(_target: *mut c_void) {}

    fn register_pair(
        provider: &NativeScmEndpointProvider,
        fixture: &Fixture,
        state: &mut PairState,
    ) {
        let installer = ScmPairInstallerV1 {
            abi_version: SCM_ENDPOINT_ABI_VERSION,
            struct_size: size_of::<ScmPairInstallerV1>() as u32,
            target: state as *mut PairState as *mut c_void,
            install: Some(capture_pair),
            clear: Some(clear_pair),
        };
        let server = fixture.server();
        assert_eq!(
            unsafe {
                (provider.hooks().register_pair.unwrap())(provider.hooks().context, &installer)
            },
            0
        );
        server.join().unwrap().unwrap();
        assert_ne!(state.holder_a, 0);
        assert_ne!(state.holder_b, 0);
    }

    #[test]
    fn retained_context_outlives_local_provider_and_release_balances() {
        let owner =
            NativeScmEndpointProvider::new(PathBuf::from("/tmp/darwin-scm-unused.sock")).unwrap();
        let weak = Arc::downgrade(&owner.context);
        let hooks = owner.hooks();
        let pinned = unsafe { hooks.retain.unwrap()(hooks.context) };
        drop(owner);
        assert!(weak.upgrade().is_some());
        assert_eq!(
            unsafe { hooks.register_pair.unwrap()(pinned, std::ptr::null()) },
            libc::EINVAL
        );
        unsafe { hooks.release.unwrap()(pinned) };
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn native_prepare_accepts_zero_payload_with_null_pointer() {
        let fixture = Fixture::new();
        let provider = NativeScmEndpointProvider::new(fixture.path.clone()).unwrap();
        let mut pair = PairState {
            holder_a: 0,
            holder_b: 0,
        };
        register_pair(&provider, &fixture, &mut pair);
        let hooks = provider.hooks();
        let request = ScmPrepareRequestV2 {
            carrier_holder: pair.holder_a.to_le_bytes(),
            payload_fds: std::ptr::null(),
            payload_count: 0,
            managed_count: 0,
            managed: std::ptr::null(),
        };
        let mut prepared = ScmPreparedV2 {
            authority: [0; 16],
            ticket: 0,
            metadata_fd: -1,
            guardian_fd: -1,
            payload_count: 0,
            reserved: 0,
        };
        let server = fixture.server();
        assert_eq!(unsafe { (hooks.prepare.unwrap())(hooks.context, &request, &mut prepared) }, 0);
        server.join().unwrap().unwrap();
        assert_eq!(prepared.payload_count, 0);
        assert!(prepared.metadata_fd >= 0 && prepared.guardian_fd >= 0);
        drop(unsafe { OwnedFd::from_raw_fd(prepared.metadata_fd) });
        drop(unsafe { OwnedFd::from_raw_fd(prepared.guardian_fd) });
        fixture.wait_transfer_idle();
    }

    #[test]
    fn native_prepare_admit_settle_roundtrip_authenticates_and_retires() {
        let fixture = Fixture::new();
        let provider = NativeScmEndpointProvider::new(fixture.path.clone()).unwrap();
        let mut pair = PairState {
            holder_a: 0,
            holder_b: 0,
        };
        register_pair(&provider, &fixture, &mut pair);
        let hooks = provider.hooks();
        let payload = File::open("/dev/null").unwrap();
        let payload_two = File::open("/dev/null").unwrap();
        let payload_fds = [payload.as_raw_fd(), payload_two.as_raw_fd()];
        // Two guest ordinals may alias one exact source Description. The
        // daemon mints one fresh delegation per ordinal during prepare.
        let managed = [
            ScmManagedPayloadV2 {
                ordinal: 0,
                holder: pair.holder_a.to_le_bytes(),
            },
            ScmManagedPayloadV2 {
                ordinal: 1,
                holder: pair.holder_a.to_le_bytes(),
            },
        ];
        let prepare_request = ScmPrepareRequestV2 {
            carrier_holder: pair.holder_a.to_le_bytes(),
            payload_fds: payload_fds.as_ptr(),
            payload_count: 2,
            managed_count: 2,
            managed: managed.as_ptr(),
        };
        let duplicate_fds = [payload.as_raw_fd(), payload.as_raw_fd()];
        let duplicate_request = ScmPrepareRequestV2 {
            carrier_holder: pair.holder_a.to_le_bytes(),
            payload_fds: duplicate_fds.as_ptr(),
            payload_count: 2,
            managed_count: 0,
            managed: std::ptr::null(),
        };
        let mut failed_prepare = ScmPreparedV2 {
            authority: [0xff; 16],
            ticket: u64::MAX,
            metadata_fd: 44,
            guardian_fd: 45,
            payload_count: 99,
            reserved: 77,
        };
        assert_eq!(
            unsafe {
                (hooks.prepare.unwrap())(hooks.context, &duplicate_request, &mut failed_prepare)
            },
            libc::EPROTO
        );
        assert_eq!(failed_prepare.metadata_fd, -1);
        assert_eq!(failed_prepare.guardian_fd, -1);
        assert_eq!(failed_prepare.payload_count, 0);

        let mut prepared = ScmPreparedV2 {
            authority: [0; 16],
            ticket: 0,
            metadata_fd: -1,
            guardian_fd: -1,
            payload_count: 0,
            reserved: 0,
        };
        let server = fixture.server();
        assert_eq!(
            unsafe { (hooks.prepare.unwrap())(hooks.context, &prepare_request, &mut prepared) },
            0
        );
        server.join().unwrap().unwrap();
        assert_ne!(prepared.metadata_fd, -1);
        assert_ne!(prepared.guardian_fd, -1);
        assert_eq!(prepared.payload_count, 2);

        let metadata = unsafe { OwnedFd::from_raw_fd(prepared.metadata_fd) };
        let guardian = unsafe { OwnedFd::from_raw_fd(prepared.guardian_fd) };
        assert_eq!(
            unsafe { libc::lseek(metadata.as_raw_fd(), 3, libc::SEEK_SET) },
            3
        );
        let mut wrong_count = ScmAdmissionV2::default();
        let wrong_count_request = ScmAdmitRequestV2 {
            carrier_holder: pair.holder_b.to_le_bytes(),
            metadata_fd: metadata.as_raw_fd(),
            payload_count: 1,
            publish_ordinals: std::ptr::null(),
            publish_count: 0,
            reserved: 0,
        };
        assert_ne!(
            unsafe {
                (hooks.admit.unwrap())(hooks.context, &wrong_count_request, &mut wrong_count)
            },
            0
        );
        assert_eq!(wrong_count.claim_count, 0);
        assert_eq!(
            unsafe { libc::lseek(metadata.as_raw_fd(), 0, libc::SEEK_CUR) },
            3
        );

        let publish_ordinals = [0_u64, 1_u64];
        let admit_request = ScmAdmitRequestV2 {
            carrier_holder: pair.holder_b.to_le_bytes(),
            metadata_fd: metadata.as_raw_fd(),
            payload_count: 2,
            publish_ordinals: publish_ordinals.as_ptr(),
            publish_count: publish_ordinals.len() as u32,
            reserved: 0,
        };
        let mut admission = ScmAdmissionV2::default();
        let server = fixture.server();
        assert_eq!(
            unsafe { (hooks.admit.unwrap())(hooks.context, &admit_request, &mut admission) },
            0
        );
        server.join().unwrap().unwrap();
        assert_eq!(admission.claim_count, 2);
        assert_eq!(admission.claims[0].ordinal, 0);
        assert_eq!(admission.claims[1].ordinal, 1);
        assert_eq!(admission.claims[0].grant.carrier, admission.claims[1].grant.carrier);
        assert_eq!(admission.claims[0].grant.side, admission.claims[1].grant.side);
        assert_ne!(admission.claims[0].grant.holder, admission.claims[1].grant.holder);
        let server = fixture.server();
        assert_eq!(
            unsafe {
                (hooks.settle.unwrap())(
                    hooks.context,
                    admission.authority.as_ptr(),
                    admission.ticket,
                    1,
                )
            },
            0
        );
        server.join().unwrap().unwrap();
        drop(metadata);
        drop(guardian);
        fixture.wait_transfer_idle();

        for holder in [
            u128::from_le_bytes(admission.claims[0].grant.holder),
            u128::from_le_bytes(admission.claims[1].grant.holder),
            pair.holder_a,
            pair.holder_b,
        ] {
            let bytes = holder.to_le_bytes();
            let server = fixture.server();
            assert_eq!(
                unsafe { (hooks.release_holder.unwrap())(hooks.context, bytes.as_ptr()) },
                0
            );
            server.join().unwrap().unwrap();
        }
        assert!(!fixture.service.has_pending().unwrap());
    }
}
