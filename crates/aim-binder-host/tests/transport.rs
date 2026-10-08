//! The daemon transport end to end, in one host process: two binder files
//! opened through the client, a transaction carrying data, an fd and a
//! security context, readiness for epoll, the reply, and release on the
//! last close.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

mod support;

use aim_binder_driver::Errno;
use aim_binder_driver::uapi::*;
use aim_binder_host::client::{Client, UserMemory};
use aim_binder_host::server::Server;
use support::*;

fn readable(fd: i32) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: polling our own fd.
    unsafe { libc::poll(&mut p, 1, 0) == 1 }
}

fn wait_readable(fd: i32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !readable(fd) {
        assert!(Instant::now() < deadline, "binder fd never became readable");
        std::thread::sleep(Duration::from_millis(1));
    }
}

static NAME: AtomicU32 = AtomicU32::new(0);

#[test]
fn transaction_with_fd_readiness_reply_and_release() {
    let name = format!(
        "dev.aim.test.binder-host.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();

    // The context manager accepts fds and wants its callers' contexts.
    let (mgr_tid, caller_tid) = (100, 200);
    let mgr = open(&client, 1000, "u:r:servicemanager:s0");
    let mut fbo = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: FLAT_BINDER_FLAG_ACCEPTS_FDS | FLAT_BINDER_FLAG_TXN_SECURITY_CTX,
        binder: 0x1234,
        cookie: 0x5678,
    }
    .encode();
    mgr.ioctl(
        mgr_tid,
        BINDER_SET_CONTEXT_MGR_EXT,
        fbo.as_mut_ptr() as u64,
        &mut Own,
    )
    .unwrap();
    let mut version = [0u8; 4];
    mgr.ioctl(
        mgr_tid,
        BINDER_VERSION,
        version.as_mut_ptr() as u64,
        &mut Own,
    )
    .unwrap();
    assert_eq!(i32::from_le_bytes(version), CURRENT_PROTOCOL_VERSION);
    let mut enter = Vec::new();
    cmd(&mut enter, BC_ENTER_LOOPER, &[]);
    write_read(&mgr, mgr_tid, &enter, false).unwrap();
    mgr.poll(mgr_tid).unwrap();
    assert!(!readable(mgr.fd));

    // The caller sends a marker and the read end of a pipe.
    let caller = open(&client, 10_001, "u:r:shell:s0");
    let mut pipe = [0i32; 2];
    // SAFETY: a pipe with a message in it.
    unsafe {
        assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
        assert_eq!(libc::write(pipe[1], b"hello".as_ptr().cast(), 5), 5);
    }
    let call = std::thread::spawn(move || {
        let mut data = [0u8; 32];
        data[0..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        data[8..12].copy_from_slice(&BINDER_TYPE_FD.to_le_bytes());
        data[16..20].copy_from_slice(&(pipe[0] as u32).to_le_bytes());
        let offsets = 8u64.to_le_bytes();
        let tr = TransactionData {
            code: 7,
            data_size: data.len() as u64,
            offsets_size: 8,
            buffer: data.as_ptr() as u64,
            offsets: offsets.as_ptr() as u64,
            ..Default::default()
        };
        let mut write = Vec::new();
        cmd(&mut write, BC_TRANSACTION, &tr.encode());
        // As IPCThreadState::waitForResponse: read until the reply.
        let mut read = write_read(&caller, caller_tid, &write, true).unwrap();
        let reply = loop {
            if let Some((_, tr)) = returns(&read).into_iter().find(|(c, _)| *c == BR_REPLY) {
                break tr.unwrap();
            }
            read = write_read(&caller, caller_tid, &[], true).unwrap();
        };
        // SAFETY: the reply lies in our read-only receive buffer.
        let value = unsafe { (reply.buffer as *const u32).read() };
        (caller, value)
    });

    // The manager's fd becomes readable; its read delivers the call.
    wait_readable(mgr.fd);
    let read = write_read(&mgr, mgr_tid, &[], true).unwrap();
    assert!(
        !readable(mgr.fd),
        "readiness must clear once the work is read"
    );
    let (code, tr) = returns(&read)
        .into_iter()
        .find(|(c, _)| *c == BR_TRANSACTION_SEC_CTX)
        .expect("BR_TRANSACTION_SEC_CTX");
    let tr = tr.unwrap();
    assert_eq!(code, BR_TRANSACTION_SEC_CTX);
    assert_eq!((tr.target, tr.cookie, tr.code), (0x1234, 0x5678, 7));
    assert_eq!(tr.sender_euid, 10_001);
    assert_eq!(tr.sender_pid, std::process::id() as i32);
    // SAFETY: the transaction lies in the manager's receive buffer, and the
    // security context pointer follows the transaction data in the read.
    unsafe {
        assert_eq!((tr.buffer as *const u32).read(), 0x1122_3344);
        let fd = ((tr.buffer + 16) as *const u32).read() as i32;
        let mut got = [0u8; 5];
        assert_eq!(libc::read(fd, got.as_mut_ptr().cast(), 5), 5);
        assert_eq!(&got, b"hello");
        let at = read.len() - 8;
        let ctx = u64::from_le_bytes(read[at..].try_into().unwrap());
        let ctx = std::ffi::CStr::from_ptr(ctx as *const libc::c_char);
        assert_eq!(ctx.to_str().unwrap(), "u:r:shell:s0");
    }

    let reply_data = 0x42u32.to_le_bytes();
    let reply = TransactionData {
        data_size: 4,
        buffer: reply_data.as_ptr() as u64,
        ..Default::default()
    };
    let mut write = Vec::new();
    cmd(&mut write, BC_FREE_BUFFER, &tr.buffer.to_le_bytes());
    cmd(&mut write, BC_REPLY, &reply.encode());
    let read = write_read(&mgr, mgr_tid, &write, true).unwrap();
    assert!(
        returns(&read)
            .iter()
            .any(|(c, _)| *c == BR_TRANSACTION_COMPLETE)
    );
    let (caller, value) = call.join().unwrap();
    assert_eq!(value, 0x42);

    // The manager's last close releases it: the caller's death
    // notification for handle 0 fires.
    let mut write = Vec::new();
    cmd(&mut write, BC_INCREFS, &0u32.to_le_bytes());
    cmd(&mut write, BC_ACQUIRE, &0u32.to_le_bytes());
    let mut death = 0u32.to_le_bytes().to_vec();
    death.extend_from_slice(&0x99u64.to_le_bytes());
    cmd(&mut write, BC_REQUEST_DEATH_NOTIFICATION, &death);
    write_read(&caller, caller_tid, &write, false).unwrap();
    // SAFETY: closing the manager's binder fd.
    unsafe { libc::close(mgr.fd) };
    let read = write_read(&caller, caller_tid, &[], true).unwrap();
    let at = read
        .windows(4)
        .position(|w| w == BR_DEAD_BINDER.to_le_bytes())
        .expect("BR_DEAD_BINDER");
    assert_eq!(
        u64::from_le_bytes(read[at + 4..at + 12].try_into().unwrap()),
        0x99
    );
}

/// Guest memory where address 0 faults, as it does in a real process.
struct NullFaults;

impl UserMemory for NullFaults {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, Errno> {
        if address == 0 {
            return Err(14); // EFAULT
        }
        Own.read(address, len)
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), Errno> {
        if address == 0 {
            return Err(14);
        }
        Own.write(address, data)
    }
}

#[test]
fn thread_exit_ignores_its_argument() {
    let name = format!(
        "dev.aim.test.binder-host.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();
    let file = open(&client, 1000, "u:r:keystore:s0");
    write_read(&file, 300, &[], false).unwrap();
    // libbinder's IPCThreadState::threadDestructor.
    file.ioctl(300, BINDER_THREAD_EXIT, 0, &mut NullFaults)
        .unwrap();
}

/// A BINDER_WRITE_READ restarted after a signal carries its write buffer
/// fully consumed: copying the (empty) rest of it succeeds, as on Linux.
#[test]
fn a_restarted_write_read_with_its_writes_consumed() {
    let name = format!(
        "dev.aim.test.binder-host.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();
    let file = open(&client, 1000, "u:r:system_server:s0");
    let mut write = Vec::new();
    cmd(&mut write, BC_ENTER_LOOPER, &[]);
    let bwr = WriteRead {
        write_size: write.len() as u64,
        write_consumed: write.len() as u64,
        write_buffer: write.as_ptr() as u64,
        ..Default::default()
    };
    let mut arg = bwr.encode();
    file.ioctl(301, BINDER_WRITE_READ, arg.as_mut_ptr() as u64, &mut Own)
        .unwrap();
    assert_eq!(WriteRead::decode(&arg).write_consumed, write.len() as u64);
}

/// A one-way call carrying more fds than a thread keeps placeholders for
/// (Chrome's `IChildProcessService.setupConnection` passes its child a
/// dozen or more): the reader gets every file, none is dropped (#256).
#[test]
fn a_transaction_with_many_fds_reaches_the_reader() {
    const FDS: usize = 20;
    let name = format!(
        "dev.aim.test.binder-host.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();
    let (mgr_tid, caller_tid) = (400, 401);
    let mgr = open(&client, 1000, "u:r:servicemanager:s0");
    let mut fbo = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
        binder: 0x1234,
        cookie: 0x5678,
    }
    .encode();
    mgr.ioctl(
        mgr_tid,
        BINDER_SET_CONTEXT_MGR_EXT,
        fbo.as_mut_ptr() as u64,
        &mut Own,
    )
    .unwrap();
    let mut enter = Vec::new();
    cmd(&mut enter, BC_ENTER_LOOPER, &[]);
    write_read(&mgr, mgr_tid, &enter, false).unwrap();

    // Each fd is the read end of a pipe holding its own index.
    let caller = open(&client, 10_001, "u:r:untrusted_app:s0");
    let mut data = vec![0u8; FDS * 24];
    let mut offsets = Vec::new();
    for i in 0..FDS {
        let mut pipe = [0i32; 2];
        // SAFETY: a pipe with one byte in it.
        unsafe {
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
            assert_eq!(libc::write(pipe[1], [i as u8].as_ptr().cast(), 1), 1);
            libc::close(pipe[1]);
        }
        let at = i * 24;
        data[at..at + 4].copy_from_slice(&BINDER_TYPE_FD.to_le_bytes());
        data[at + 8..at + 12].copy_from_slice(&(pipe[0] as u32).to_le_bytes());
        offsets.extend_from_slice(&(at as u64).to_le_bytes());
    }
    let tr = TransactionData {
        code: 3,
        flags: TF_ONE_WAY | TF_ACCEPT_FDS,
        data_size: data.len() as u64,
        offsets_size: offsets.len() as u64,
        buffer: data.as_ptr() as u64,
        offsets: offsets.as_ptr() as u64,
        ..Default::default()
    };
    let mut write = Vec::new();
    cmd(&mut write, BC_TRANSACTION, &tr.encode());
    let read = write_read(&caller, caller_tid, &write, true).unwrap();
    assert!(
        returns(&read)
            .iter()
            .any(|(c, _)| *c == BR_TRANSACTION_COMPLETE)
    );

    let read = write_read(&mgr, mgr_tid, &[], true).unwrap();
    let tr = returns(&read)
        .into_iter()
        .find_map(|(c, tr)| (c == BR_TRANSACTION).then(|| tr.unwrap()))
        .expect("BR_TRANSACTION");
    assert_eq!((tr.code, tr.data_size), (3, (FDS * 24) as u64));
    for i in 0..FDS {
        // SAFETY: the transaction lies in the manager's receive buffer; its
        // fd objects now hold the manager's fds.
        unsafe {
            let fd = ((tr.buffer + (i * 24 + 8) as u64) as *const u32).read() as i32;
            let mut got = [0u8; 1];
            assert_eq!(libc::read(fd, got.as_mut_ptr().cast(), 1), 1, "fd {i}");
            assert_eq!(got[0], i as u8);
        }
    }
}

/// Files the daemon's process shares reach a client, paged past one
/// message's ports, as memory entries of the file's own pages.
#[test]
fn shared_files_reach_the_client_as_the_same_pages() {
    use aim_binder_host::mach;
    use aim_binder_host::wire::SharedFile;
    use std::os::unix::fs::MetadataExt;

    let name = format!(
        "dev.aim.test.binder-host.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let server = Server::start(&name).unwrap();
    let path = std::env::temp_dir().join(format!("aim-shared-file-{}", std::process::id()));
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    const LEN: usize = 16384;
    file.set_len(LEN as u64).unwrap();
    let meta = file.metadata().unwrap();
    // SAFETY: a shared mapping of our file, and a reservation for its view.
    let (rw, view) = unsafe {
        use std::os::fd::AsRawFd;
        let rw = libc::mmap(
            std::ptr::null_mut(),
            LEN,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        ) as *mut u8;
        let view = libc::mmap(
            std::ptr::null_mut(),
            LEN,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        ) as *const u8;
        (rw, view)
    };
    let entry = mach::share_read_only(rw as u64, LEN as u64).unwrap();
    for i in 0..300 {
        server.share_file(SharedFile {
            dev: meta.dev() as u32 as u64,
            ino: meta.ino() + i,
            size: LEN as u64,
            entry,
        });
    }
    let client = Client::connect(&name).unwrap();
    let shared = client.shared_files().unwrap();
    assert_eq!(shared.len(), 300);
    assert_eq!(shared[299].ino, meta.ino() + 299);
    assert_eq!(shared[0].size, LEN as u64);
    mach::map_read_only(shared[0].entry, view as u64, LEN as u64).unwrap();
    // SAFETY: both mappings are LEN bytes long.
    unsafe {
        rw.add(100).write(7);
        assert_eq!(view.add(100).read_volatile(), 7);
    }
    let _ = std::fs::remove_file(&path);
}

/// More guest threads wait in the driver at once than the daemon has
/// workers: a parked read holds no worker (#553). Every caller's call is
/// delivered before any looper replies.
#[test]
fn more_waiting_threads_than_workers() {
    let name = format!(
        "dev.aim.test.binder-host.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();
    let n = 4 * std::thread::available_parallelism().map_or(4, |n| n.get());
    let mgr = open(&client, 1000, "u:r:servicemanager:s0");
    let mut fbo = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: 0x1234,
        cookie: 0x5678,
    }
    .encode();
    mgr.ioctl(
        100,
        BINDER_SET_CONTEXT_MGR_EXT,
        fbo.as_mut_ptr() as u64,
        &mut Own,
    )
    .unwrap();
    let all_delivered = std::sync::Arc::new(std::sync::Barrier::new(n));
    for i in 0..n {
        let all_delivered = all_delivered.clone();
        std::thread::spawn(move || {
            let tid = 1000 + i as i32;
            let mut enter = Vec::new();
            cmd(&mut enter, BC_ENTER_LOOPER, &[]);
            let mut read = write_read(&mgr, tid, &enter, true).unwrap();
            let call = loop {
                if let Some((_, tr)) = returns(&read)
                    .into_iter()
                    .find(|(c, _)| *c == BR_TRANSACTION)
                {
                    break tr.unwrap();
                }
                read = write_read(&mgr, tid, &[], true).unwrap();
            };
            all_delivered.wait();
            let mut reply = Vec::new();
            cmd(&mut reply, BC_FREE_BUFFER, &call.buffer.to_le_bytes());
            cmd(&mut reply, BC_REPLY, &TransactionData::default().encode());
            write_read(&mgr, tid, &reply, false).unwrap();
        });
    }
    let caller = open(&client, 10_001, "u:r:shell:s0");
    let (done, finished) = std::sync::mpsc::channel();
    for i in 0..n {
        let done = done.clone();
        std::thread::spawn(move || {
            let tid = 5000 + i as i32;
            let tr = TransactionData {
                code: 1,
                ..Default::default()
            };
            let mut write = Vec::new();
            cmd(&mut write, BC_TRANSACTION, &tr.encode());
            let mut read = write_read(&caller, tid, &write, true).unwrap();
            while !returns(&read).iter().any(|(c, _)| *c == BR_REPLY) {
                read = write_read(&caller, tid, &[], true).unwrap();
            }
            done.send(()).unwrap();
        });
    }
    for _ in 0..n {
        finished
            .recv_timeout(Duration::from_secs(20))
            .expect("a call never completed");
    }
}

#[test]
fn typed_fd_import_failure_rolls_back_delivery_before_copyout_and_unblocks_sender() {
    struct Reject {
        installed: Vec<i32>,
        closed: Vec<i32>,
        writes: usize,
    }
    impl UserMemory for Reject {
        fn read(&mut self, a: u64, n: usize) -> Result<Vec<u8>, Errno> {
            Own.read(a, n)
        }
        fn write(&mut self, a: u64, b: &[u8]) -> Result<(), Errno> {
            self.writes += 1;
            Own.write(a, b)
        }
        fn install_typed(&mut self, fd: i32, _: u32) -> Result<(), Errno> {
            self.installed.push(fd);
            Err(22)
        }
        fn closed(&mut self, fd: i32) {
            self.closed.push(fd);
        }
    }
    let name = format!(
        "dev.aim.test.binder-reject.{}.{}",
        std::process::id(),
        NAME.fetch_add(1, Ordering::Relaxed)
    );
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();
    let mgr = open(&client, 1000, "u:r:servicemanager:s0");
    let caller = open(&client, 10001, "u:r:shell:s0");
    let mut node = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
        binder: 0x1234,
        cookie: 0x5678,
    }
    .encode();
    mgr.ioctl(
        100,
        BINDER_SET_CONTEXT_MGR_EXT,
        node.as_mut_ptr() as u64,
        &mut Own,
    )
    .unwrap();
    let mut enter = Vec::new();
    cmd(&mut enter, BC_ENTER_LOOPER, &[]);
    write_read(&mgr, 100, &enter, false).unwrap();
    let mut pipe = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let mut expected: libc::stat = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::fstat(pipe[0], &mut expected) }, 0);
    let calling = std::thread::spawn(move || {
        use std::os::fd::AsRawFd;
        use std::os::fd::FromRawFd;
        let file = unsafe { std::fs::File::from_raw_fd(pipe[0]) };
        let _writer = unsafe { std::fs::File::from_raw_fd(pipe[1]) };
        let object = FlatBinderObject {
            kind: BINDER_TYPE_FD,
            flags: 0,
            binder: file.as_raw_fd() as u64,
            cookie: 0,
        }
        .encode();
        let offsets = 0u64.to_le_bytes();
        let tr = TransactionData {
            code: 7,
            flags: TF_ACCEPT_FDS,
            data_size: object.len() as u64,
            offsets_size: 8,
            buffer: object.as_ptr() as u64,
            offsets: offsets.as_ptr() as u64,
            ..Default::default()
        };
        let mut write = Vec::new();
        cmd(&mut write, BC_TRANSACTION, &tr.encode());
        let mut read = write_read(&caller, 200, &write, true).unwrap();
        while !returns(&read)
            .iter()
            .any(|(code, _)| *code == BR_FAILED_REPLY)
        {
            read = write_read(&caller, 200, &[], true).unwrap();
        }
        let mut write = Vec::new();
        cmd(
            &mut write,
            BC_TRANSACTION,
            &TransactionData {
                code: 8,
                ..Default::default()
            }
            .encode(),
        );
        let mut read = write_read(&caller, 200, &write, true).unwrap();
        loop {
            if let Some((_, Some(reply))) = returns(&read).into_iter().find(|(c, _)| *c == BR_REPLY)
            {
                let mut free = Vec::new();
                cmd(&mut free, BC_FREE_BUFFER, &reply.buffer.to_le_bytes());
                write_read(&caller, 200, &free, false).unwrap();
                break;
            }
            read = write_read(&caller, 200, &[], true).unwrap();
        }
        unsafe { libc::close(caller.fd) };
    });
    let mut read = [0u8; 512];
    let mut arg = WriteRead {
        read_size: 512,
        read_buffer: read.as_mut_ptr() as u64,
        ..Default::default()
    }
    .encode();
    let mut reject = Reject {
        installed: Vec::new(),
        closed: Vec::new(),
        writes: 0,
    };
    assert_eq!(
        mgr.ioctl(100, BINDER_WRITE_READ, arg.as_mut_ptr() as u64, &mut reject),
        Err(22)
    );
    assert_eq!(reject.writes, 0);
    assert_eq!(reject.installed.len(), 1);
    assert_eq!(reject.closed, reject.installed);
    // Parallel tests may reuse a just-closed numeric FD. The received unique
    // pipe must be gone from this slot, even if another file has since taken it.
    let mut remaining: libc::stat = unsafe { std::mem::zeroed() };
    let exists = unsafe { libc::fstat(reject.installed[0], &mut remaining) } == 0;
    assert!(!exists || (remaining.st_dev, remaining.st_ino) != (expected.st_dev, expected.st_ino));
    let read = write_read(&mgr, 100, &[], true).unwrap();
    let (_, Some(next)) = returns(&read)
        .into_iter()
        .find(|(c, _)| *c == BR_TRANSACTION)
        .unwrap()
    else {
        panic!("transaction required")
    };
    assert_eq!(next.code, 8);
    let mut reply = Vec::new();
    cmd(&mut reply, BC_FREE_BUFFER, &next.buffer.to_le_bytes());
    cmd(&mut reply, BC_REPLY, &TransactionData::default().encode());
    write_read(&mgr, 100, &reply, false).unwrap();
    calling.join().unwrap();
    unsafe { libc::close(mgr.fd) };
}
