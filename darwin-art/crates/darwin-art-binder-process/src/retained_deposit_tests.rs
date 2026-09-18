//! Tests the actual outbound Binder caller, not daemon ownership acceptance.
use super::*;
use crate::{
    DescriptorApi, DescriptorRetainedApi, DescriptorTransferBinding, RetainedExportedDescriptor,
};
use darwin_art_binder_device::authority_protocol::{ConnectionToken, LocalNodeToken, NodeToken};
use std::{ffi::c_void, os::fd::IntoRawFd, sync::atomic::AtomicUsize};

static SERIAL: Mutex<()> = Mutex::new(());
static RELEASES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn export(
    _: i32,
    _: *const DescriptorTransferBinding,
    output: *mut RetainedExportedDescriptor,
) -> i32 {
    let fd = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
    let lease = Box::into_raw(Box::new(0_u8)).cast();
    unsafe {
        (*output).host_fd = fd;
        (*output).lease = lease;
    }
    0
}
unsafe extern "C" fn release(lease: *mut c_void) {
    unsafe {
        drop(Box::from_raw(lease.cast::<u8>()));
    }
    RELEASES.fetch_add(1, Ordering::SeqCst);
}
unsafe extern "C" fn bare(_: i32) -> i32 {
    panic!("bare provider called")
}

struct PausedDeposit {
    entered: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<bool>>,
}
impl AuthorityTransport for PausedDeposit {
    fn connection(&self) -> ConnectionToken {
        ConnectionToken::from_nonzero(4).unwrap()
    }
    fn android_uid(&self) -> u32 {
        1000
    }
    fn send(&self, _: Message) -> Result<(), crate::TransportError> {
        // Source retention must have ended at deposit ACK, before routing.
        assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
        Err(crate::TransportError("controlled route failure".into()))
    }
    fn receive(&self) -> Result<Message, crate::TransportError> {
        unreachable!()
    }
    fn deposit_transfer(
        &self,
        _: TransferToken,
        image: &TransferImage,
    ) -> Result<(), crate::TransportError> {
        assert_eq!(image.files().len(), 1);
        assert_eq!(RELEASES.load(Ordering::SeqCst), 0);
        self.entered.send(()).unwrap();
        let success = self
            .resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(RELEASES.load(Ordering::SeqCst), 0);
        if success {
            Ok(())
        } else {
            Err(crate::TransportError("controlled deposit failure".into()))
        }
    }
    fn take_transfer(
        &self,
        _: ConnectionToken,
        _: TransferToken,
    ) -> Result<TransferImage, crate::TransportError> {
        unreachable!()
    }
}

fn paused_deposit(success: bool) {
    let _serial = SERIAL.lock().unwrap();
    RELEASES.store(0, Ordering::SeqCst);
    let (entered, observer) = mpsc::sync_channel(1);
    let (resume, resumed) = mpsc::sync_channel(1);
    let device = Device::default()
        .open(ProcessIdentity::new(33, 1000).unwrap())
        .unwrap();
    let (client, _) = Client::new(
        PausedDeposit {
            entered,
            resume: Mutex::new(resumed),
        },
        device,
    );
    client
        .install_descriptor_api(DescriptorApi {
            export: bare,
            import: bare,
            close: bare,
            bundle: None,
            retained: Some(DescriptorRetainedApi { export, release }),
        })
        .unwrap();
    let worker = std::thread::spawn(move || {
        let mut bytes = [0_u8; 24];
        bytes[..4].copy_from_slice(
            &darwin_art_binder_device::objects::Kind::Fd
                .tag()
                .to_le_bytes(),
        );
        bytes[8..12].copy_from_slice(&17_u32.to_le_bytes());
        let snapshot = TransactionSnapshot::capture(&bytes, &0_u64.to_le_bytes(), 0).unwrap();
        let target = NodeToken::new(
            ConnectionToken::from_nonzero(7).unwrap(),
            LocalNodeToken::from_nonzero(3).unwrap(),
        );
        client.submit_transaction(12, target, &snapshot, &[], 9, 0)
    });
    let observed = observer.recv_timeout(Duration::from_secs(2));
    let retained = RELEASES.load(Ordering::SeqCst);
    // Always wake/join the worker before assertions, including a failed wait.
    let _ = resume.send(success);
    let result = worker.join();
    observed.unwrap();
    assert_eq!(retained, 0);
    assert!(matches!(result.unwrap(), Err(Error::Transport(_))));
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn actual_transaction_keeps_lease_through_paused_deposit_then_releases_before_route() {
    paused_deposit(true);
}
#[test]
fn actual_transaction_deposit_failure_releases_lease() {
    paused_deposit(false);
}
