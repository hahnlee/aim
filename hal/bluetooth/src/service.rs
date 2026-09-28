//! `IBluetoothHci` over the host controller, with the semantics of AOSP's
//! default implementation (hardware/interfaces/bluetooth/aidl/default):
//! `initialize` opens the controller and answers through
//! `initializationComplete`; a second `initialize` gets
//! `ALREADY_INITIALIZED`; `close`, or the death of the stack's callback
//! object, closes it.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use aim_hostcall::bluetooth::{MAX_PACKET, kind};
use aim_hostcall::guest::{self, Errno};
use android_hardware_bluetooth::aidl::android::hardware::bluetooth::{
    IBluetoothHci::IBluetoothHci, IBluetoothHciCallbacks::IBluetoothHciCallbacks, Status::Status,
};
use binder::{DeathRecipient, IBinder, Interface, Strong};

struct Session {
    reader: Option<JoinHandle<()>>,
    wake: i32,
    _death: DeathRecipient,
}

#[derive(Clone, Default)]
pub struct Hci {
    session: Arc<Mutex<Option<Session>>>,
}

impl Interface for Hci {}

fn close(session: &Mutex<Option<Session>>) {
    let Some(mut s) = session.lock().unwrap().take() else {
        return;
    };
    // The host drops its end of the wake pipe: the reader sees the hang-up.
    let _ = guest::bluetooth_close();
    if let Some(r) = s.reader.take() {
        let _ = r.join();
    }
    // SAFETY: our fd, closed once.
    unsafe { libc::close(s.wake) };
}

/// Deliver the host's packets to the stack until the controller closes.
fn reader(wake: i32, callbacks: Strong<dyn IBluetoothHciCallbacks>) {
    let mut buf = vec![0u8; MAX_PACKET];
    loop {
        let mut p = libc::pollfd {
            fd: wake,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd on our stack.
        let n = unsafe { libc::poll(&mut p, 1, -1) };
        if n < 0 {
            continue;
        }
        loop {
            match guest::bluetooth_recv(&mut buf) {
                Ok((k, len)) => {
                    let packet = &buf[..len];
                    let r = match k {
                        kind::EVENT => callbacks.hciEventReceived(packet),
                        kind::ACL => callbacks.aclDataReceived(packet),
                        _ => Ok(()),
                    };
                    if let Err(e) = r {
                        eprintln!("bluetooth: callback failed: {e:?}");
                    }
                }
                Err(Errno(libc::EAGAIN)) => break,
                Err(_) => return,
            }
        }
        if p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return;
        }
    }
}

impl IBluetoothHci for Hci {
    fn initialize(&self, callbacks: &Strong<dyn IBluetoothHciCallbacks>) -> binder::Result<()> {
        let mut slot = self.session.lock().unwrap();
        if slot.is_some() {
            drop(slot);
            return callbacks.initializationComplete(Status::ALREADY_INITIALIZED);
        }
        let wake = match guest::bluetooth_open() {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("bluetooth: cannot open the host controller: {e:?}");
                drop(slot);
                return callbacks.initializationComplete(Status::UNABLE_TO_OPEN_INTERFACE);
            }
        };
        let weak = Arc::downgrade(&self.session);
        let mut death = DeathRecipient::new(move || {
            if let Some(s) = weak.upgrade() {
                eprintln!("bluetooth: the stack died; closing");
                close(&s);
            }
        });
        callbacks.as_binder().link_to_death(&mut death)?;
        let cb = callbacks.clone();
        *slot = Some(Session {
            reader: Some(std::thread::spawn(move || reader(wake, cb))),
            wake,
            _death: death,
        });
        drop(slot);
        callbacks.initializationComplete(Status::SUCCESS)
    }

    fn close(&self) -> binder::Result<()> {
        close(&self.session);
        Ok(())
    }

    fn sendHciCommand(&self, command: &[u8]) -> binder::Result<()> {
        send(kind::COMMAND, command);
        Ok(())
    }

    fn sendAclData(&self, data: &[u8]) -> binder::Result<()> {
        send(kind::ACL, data);
        Ok(())
    }

    fn sendScoData(&self, data: &[u8]) -> binder::Result<()> {
        send(kind::SCO, data);
        Ok(())
    }

    fn sendIsoData(&self, data: &[u8]) -> binder::Result<()> {
        send(kind::ISO, data);
        Ok(())
    }
}

fn send(k: u32, packet: &[u8]) {
    if let Err(e) = guest::bluetooth_send(k, packet) {
        eprintln!("bluetooth: packet of kind {k} dropped: {e:?}");
    }
}
