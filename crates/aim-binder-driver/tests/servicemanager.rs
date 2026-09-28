//! Golden replay of the command sequences libbinder issues for
//! servicemanager's start, `defaultServiceManager()`, `addService`,
//! `getService` and a call between two clients, read from the Android 16
//! sources:
//!
//! - servicemanager `main.cpp`: `becomeContextManager()` issues
//!   `BINDER_SET_CONTEXT_MGR_EXT` with `FLAT_BINDER_FLAG_TXN_SECURITY_CTX`;
//!   `BinderCallback::setupTo` calls `IPCThreadState::setupPolling`
//!   (`BC_ENTER_LOOPER`, flushed without a read) and then runs
//!   `handlePolledCommands()` whenever the fd is readable.
//! - `ProcessState::getStrongProxyForHandle(0)` pings the context manager
//!   (`PING_TRANSACTION`, `TF_ACCEPT_FDS`) before it creates the BpBinder,
//!   whose constructor issues `BC_INCREFS 0` and whose first strong ref
//!   issues `BC_ACQUIRE 0`; on a non-looper thread `flushIfNeeded()` writes
//!   each at once.
//! - `IPCThreadState::executeCommand`: BR_INCREFS/BR_ACQUIRE queue
//!   BC_INCREFS_DONE/BC_ACQUIRE_DONE; a received Parcel's destructor queues
//!   BC_FREE_BUFFER; servicemanager's `addService` takes a strong proxy
//!   (BC_INCREFS/BC_ACQUIRE) and links to death (flushed).
//!
//! AIDL transaction codes are opaque to the driver; the values below only
//! need to be consistent within the test.

mod support;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use aim_binder_driver::uapi::*;
use aim_binder_driver::{Device, Driver};
use support::*;

const PING_TRANSACTION: u32 = u32::from_be_bytes(*b"_PNG");
const GET_SERVICE: u32 = 1;
const ADD_SERVICE: u32 = 3;
const SERVICE_CALL: u32 = 1;

const SM_PID: i32 = 100;
const A_PID: i32 = 200;
const B_PID: i32 = 300;
const SYSTEM_UID: u32 = 1000;
const APP_UID: u32 = 10_050;

const SERVICE_WEAKREFS: u64 = 0xa000;
const SERVICE_BBINDER: u64 = 0xa100;
const SM_DEATH_COOKIE: u64 = 0x5eed;

struct Transcript(Mutex<Vec<String>>);

impl Transcript {
    fn log(&self, who: &str, returns: &[Return]) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{who}: {}", names(returns).join(" ")));
    }
}

/// servicemanager's side: wait until the binder fd is readable, then one
/// `handlePolledCommands()` read.
struct ServiceManager {
    process: Arc<Process>,
    readable: mpsc::Receiver<()>,
}

impl ServiceManager {
    fn start(driver: &Arc<Driver>) -> Self {
        let process = Process::open_with(
            driver,
            Device::Binder,
            SM_PID,
            SYSTEM_UID,
            Some("u:r:servicemanager:s0"),
            0,
        );
        process
            .become_context_manager(SM_PID, FLAT_BINDER_FLAG_TXN_SECURITY_CTX)
            .unwrap();
        // A second context manager is refused.
        assert_eq!(
            process.become_context_manager(SM_PID, 0),
            Err(aim_binder_driver::errno::EBUSY)
        );
        process.flush(SM_PID, Commands::new().enter_looper());
        let (tx, readable) = mpsc::channel();
        let tx = Mutex::new(tx);
        driver.set_notifier(
            process.handle,
            Arc::new(move |_| {
                let _ = tx.lock().unwrap().send(());
            }),
        );
        assert!(!driver.poll(process.handle, SM_PID).unwrap());
        Self { process, readable }
    }

    fn next(&self) -> Vec<Return> {
        while !self
            .process
            .driver
            .poll(self.process.handle, SM_PID)
            .unwrap()
        {
            self.readable.recv().unwrap();
        }
        self.process.transact(SM_PID, &Commands::new())
    }
}

/// `defaultServiceManager()` on a fresh client thread.
fn default_service_manager(
    client: &Arc<Process>,
    tid: i32,
    sm: &ServiceManager,
    log: &Transcript,
    who: &str,
) {
    let pinger = {
        let client = client.clone();
        thread::spawn(move || {
            client.transact(
                tid,
                Commands::new().transaction(0, PING_TRANSACTION, TF_ACCEPT_FDS, &Parcel::new()),
            )
        })
    };
    let request = sm.next();
    log.log("sm", &request);
    let (tr, secctx) = find_transaction(&request);
    assert_eq!((tr.target, tr.cookie, tr.code), (0, 0, PING_TRANSACTION));
    assert_eq!(tr.sender_pid, client.pid);
    let expected_context = if who == "a" {
        "u:r:system_server:s0"
    } else {
        "u:r:untrusted_app:s0:c50,c256,c512,c768"
    };
    assert_eq!(read_c_string(secctx.unwrap()), expected_context);
    let replied = sm
        .process
        .transact(SM_PID, Commands::new().reply(0, &Parcel::new()));
    log.log("sm", &replied);
    sm.process
        .flush(SM_PID, Commands::new().free_buffer(tr.buffer));

    let returns = pinger.join().unwrap();
    log.log(who, &returns);
    let reply = find_reply(&returns);
    assert_eq!(reply.data_size, 0);
    client.flush(tid, Commands::new().free_buffer(reply.buffer));
    client.flush(tid, Commands::new().increfs(0));
    client.flush(tid, Commands::new().acquire(0));
}

#[test]
fn servicemanager_handshake_add_get_and_call_golden() {
    let driver = Driver::new();
    let log = Transcript(Mutex::new(Vec::new()));
    let sm = ServiceManager::start(&driver);

    // --- Client A (system_server-like): defaultServiceManager()->addService.
    let a = Process::open_with(
        &driver,
        Device::Binder,
        A_PID,
        SYSTEM_UID,
        Some("u:r:system_server:s0"),
        15,
    );
    default_service_manager(&a, A_PID, &sm, &log, "a");

    let mut add = Parcel::new();
    add.write_interface_token("android.os.IServiceManager")
        .write_string16("demo.service")
        .write_local_binder(SERVICE_WEAKREFS, SERVICE_BBINDER)
        .write_i32(0) // allowIsolated
        .write_i32(8); // DUMP_FLAG_PRIORITY_DEFAULT
    let adder = {
        let a = a.clone();
        let add = add.clone();
        thread::spawn(move || {
            a.transact(
                A_PID,
                Commands::new().transaction(0, ADD_SERVICE, TF_ACCEPT_FDS, &add),
            )
        })
    };
    let request = sm.next();
    log.log("sm", &request);
    let (tr, _) = find_transaction(&request);
    assert_eq!(tr.code, ADD_SERVICE);
    assert_eq!(tr.sender_euid, SYSTEM_UID);
    let service = object_of(&tr, 0);
    assert_eq!(service.kind, BINDER_TYPE_HANDLE);
    assert_eq!(service.handle(), 1, "first non-context-manager handle");
    assert_eq!(service.cookie, 0);
    // Everything but the object is delivered as sent.
    let received = data_of(&tr);
    let object_at = add.offsets[0] as usize;
    assert_eq!(received[..object_at], add.data[..object_at]);
    assert_eq!(received[object_at + 24..], add.data[object_at + 24..]);
    // readStrongBinder -> BpBinder(1); linkToDeath flushes.
    sm.process.flush(
        SM_PID,
        Commands::new()
            .increfs(1)
            .acquire(1)
            .request_death(1, SM_DEATH_COOKIE),
    );
    let mut status = Parcel::new();
    status.write_i32(0);
    let replied = sm
        .process
        .transact(SM_PID, Commands::new().reply(0, &status));
    log.log("sm", &replied);
    sm.process
        .flush(SM_PID, Commands::new().free_buffer(tr.buffer));

    let returns = adder.join().unwrap();
    log.log("a", &returns);
    assert!(returns.contains(&Return::Increfs {
        ptr: SERVICE_WEAKREFS,
        cookie: SERVICE_BBINDER
    }));
    assert!(returns.contains(&Return::Acquire {
        ptr: SERVICE_WEAKREFS,
        cookie: SERVICE_BBINDER
    }));
    let reply = find_reply(&returns);
    assert_eq!(data_of(&reply), 0i32.to_le_bytes());
    a.flush(
        A_PID,
        Commands::new()
            .increfs_done(SERVICE_WEAKREFS, SERVICE_BBINDER)
            .acquire_done(SERVICE_WEAKREFS, SERVICE_BBINDER)
            .free_buffer(reply.buffer),
    );

    // A joins its thread pool on a second thread (the main looper). The
    // new thread's first read returns at once, and the driver asks for one
    // more looper since none is waiting.
    let spawn = a.transact(A_PID + 1, Commands::new().enter_looper());
    log.log("a", &spawn);
    assert_eq!(spawn, [Return::SpawnLooper]);
    let a_looper = {
        let a = a.clone();
        thread::spawn(move || a.transact(A_PID + 1, &Commands::new()))
    };

    // --- Client B (an app): defaultServiceManager()->getService.
    let b = Process::open_with(
        &driver,
        Device::Binder,
        B_PID,
        APP_UID,
        Some("u:r:untrusted_app:s0:c50,c256,c512,c768"),
        15,
    );
    default_service_manager(&b, B_PID, &sm, &log, "b");
    let mut get = Parcel::new();
    get.write_interface_token("android.os.IServiceManager")
        .write_string16("demo.service");
    let getter = {
        let b = b.clone();
        thread::spawn(move || {
            b.transact(
                B_PID,
                Commands::new().transaction(0, GET_SERVICE, TF_ACCEPT_FDS, &get),
            )
        })
    };
    let request = sm.next();
    log.log("sm", &request);
    let (tr, _) = find_transaction(&request);
    assert_eq!(
        (tr.code, tr.sender_pid, tr.sender_euid),
        (GET_SERVICE, B_PID, APP_UID)
    );
    let mut found = Parcel::new();
    found.write_i32(0).write_handle(1);
    let replied = sm
        .process
        .transact(SM_PID, Commands::new().reply(0, &found));
    log.log("sm", &replied);
    sm.process
        .flush(SM_PID, Commands::new().free_buffer(tr.buffer));
    let returns = getter.join().unwrap();
    log.log("b", &returns);
    let reply = find_reply(&returns);
    let service = object_of(&reply, 0);
    assert_eq!((service.kind, service.handle()), (BINDER_TYPE_HANDLE, 1));
    b.flush(B_PID, Commands::new().increfs(1));
    b.flush(B_PID, Commands::new().acquire(1));
    b.flush(B_PID, Commands::new().free_buffer(reply.buffer));

    // --- B calls A's service directly.
    let mut call = Parcel::new();
    call.write_interface_token("demo.IDemo")
        .write_string16("ping");
    let caller = {
        let b = b.clone();
        thread::spawn(move || {
            b.transact(
                B_PID,
                Commands::new().transaction(1, SERVICE_CALL, TF_ACCEPT_FDS, &call),
            )
        })
    };
    let request = a_looper.join().unwrap();
    log.log("a", &request);
    let (tr, secctx) = find_transaction(&request);
    assert_eq!((tr.target, tr.cookie), (SERVICE_WEAKREFS, SERVICE_BBINDER));
    assert_eq!(
        (tr.sender_pid, tr.sender_euid, secctx),
        (B_PID, APP_UID, None)
    );
    let mut pong = Parcel::new();
    pong.write_i32(0).write_string16("pong");
    let replied = a.transact(A_PID + 1, Commands::new().reply(0, &pong));
    log.log("a", &replied);
    a.flush(A_PID + 1, Commands::new().free_buffer(tr.buffer));
    let returns = caller.join().unwrap();
    log.log("b", &returns);
    let reply = find_reply(&returns);
    assert_eq!(data_of(&reply), pong.data);
    b.flush(B_PID, Commands::new().free_buffer(reply.buffer));

    // --- A dies: servicemanager's obituary, B's call fails.
    a.release();
    let obituary = sm.next();
    log.log("sm", &obituary);
    assert!(obituary.contains(&Return::DeadBinder(SM_DEATH_COOKIE)));
    // BpBinder::sendObituary clears and flushes; then BC_DEAD_BINDER_DONE.
    sm.process
        .flush(SM_PID, Commands::new().clear_death(1, SM_DEATH_COOKIE));
    let done = sm
        .process
        .transact(SM_PID, Commands::new().dead_binder_done(SM_DEATH_COOKIE));
    log.log("sm", &done);
    assert!(done.contains(&Return::ClearDeathDone(SM_DEATH_COOKIE)));
    sm.process
        .flush(SM_PID, Commands::new().release(1).decrefs(1));

    let dead = b.transact(
        B_PID,
        Commands::new().transaction(1, SERVICE_CALL, TF_ACCEPT_FDS, &Parcel::new()),
    );
    log.log("b", &dead);

    let transcript = log.0.into_inner().unwrap();
    let expected = [
        "sm: BR_NOOP BR_TRANSACTION_SEC_CTX",
        "sm: BR_NOOP BR_TRANSACTION_COMPLETE",
        "a: BR_NOOP BR_TRANSACTION_COMPLETE BR_REPLY",
        "sm: BR_NOOP BR_TRANSACTION_SEC_CTX",
        "sm: BR_NOOP BR_TRANSACTION_COMPLETE",
        "a: BR_NOOP BR_INCREFS BR_ACQUIRE BR_TRANSACTION_COMPLETE BR_REPLY",
        "a: BR_SPAWN_LOOPER",
        "sm: BR_NOOP BR_TRANSACTION_SEC_CTX",
        "sm: BR_NOOP BR_TRANSACTION_COMPLETE",
        "b: BR_NOOP BR_TRANSACTION_COMPLETE BR_REPLY",
        "sm: BR_NOOP BR_TRANSACTION_SEC_CTX",
        "sm: BR_NOOP BR_TRANSACTION_COMPLETE",
        "b: BR_NOOP BR_TRANSACTION_COMPLETE BR_REPLY",
        "a: BR_NOOP BR_TRANSACTION",
        "a: BR_NOOP BR_TRANSACTION_COMPLETE",
        "b: BR_NOOP BR_TRANSACTION_COMPLETE BR_REPLY",
        "sm: BR_NOOP BR_DEAD_BINDER",
        "sm: BR_NOOP BR_CLEAR_DEATH_NOTIFICATION_DONE",
        "b: BR_NOOP BR_DEAD_REPLY",
    ];
    assert_eq!(transcript, expected);
}
