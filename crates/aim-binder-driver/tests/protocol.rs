//! Byte-level protocol tests: references, one-way ordering, fds,
//! scatter-gather, nested calls, deaths, failures and the ioctl surface.

mod support;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use aim_binder_driver::uapi::*;
use aim_binder_driver::{Device, Driver, HeapReceiveMemory, errno};
use support::*;

const P_PID: i32 = 10;
const Q_PID: i32 = 20;
const NODE_PTR: u64 = 0x7000;
const NODE_COOKIE: u64 = 0x7100;

/// A context manager `Q` whose thread `Q_PID` has entered the looper.
fn context_manager(driver: &Arc<Driver>, flags: u32) -> Arc<Process> {
    let q = Process::open(driver, Device::Binder, Q_PID, 1000, 0);
    q.become_context_manager(Q_PID, flags).unwrap();
    q.flush(Q_PID, Commands::new().enter_looper());
    q
}

fn with_binder(ptr: u64, cookie: u64) -> Parcel {
    let mut p = Parcel::new();
    p.write_i32(42).write_local_binder(ptr, cookie);
    p
}

/// P sends its node to Q in a call; Q takes a strong and weak proxy and
/// replies. Returns Q's handle for the node.
fn publish(p: &Arc<Process>, q: &Arc<Process>) -> u32 {
    let caller = {
        let p = p.clone();
        thread::spawn(move || {
            p.transact(
                P_PID,
                Commands::new().transaction(
                    0,
                    1,
                    TF_ACCEPT_FDS,
                    &with_binder(NODE_PTR, NODE_COOKIE),
                ),
            )
        })
    };
    let request = q.transact(Q_PID, &Commands::new());
    let (tr, _) = find_transaction(&request);
    let handle = object_of(&tr, 0).handle();
    q.flush(Q_PID, Commands::new().increfs(handle).acquire(handle));
    let replied = q.transact(Q_PID, Commands::new().reply(0, &Parcel::new()));
    assert_eq!(names(&replied), ["BR_NOOP", "BR_TRANSACTION_COMPLETE"]);
    q.flush(Q_PID, Commands::new().free_buffer(tr.buffer));
    let returns = caller.join().unwrap();
    assert_eq!(
        names(&returns),
        [
            "BR_NOOP",
            "BR_INCREFS",
            "BR_ACQUIRE",
            "BR_TRANSACTION_COMPLETE",
            "BR_REPLY"
        ]
    );
    let reply = find_reply(&returns);
    p.flush(
        P_PID,
        Commands::new()
            .increfs_done(NODE_PTR, NODE_COOKIE)
            .acquire_done(NODE_PTR, NODE_COOKIE)
            .free_buffer(reply.buffer),
    );
    handle
}

fn node_debug_info(p: &Process, above: u64) -> (u64, u64, u32, u32) {
    let mut info = [0u8; NODE_DEBUG_INFO_SIZE];
    info[..8].copy_from_slice(&above.to_le_bytes());
    p.ioctl(P_PID, BINDER_GET_NODE_DEBUG_INFO, &mut info)
        .unwrap();
    let u64_at = |o: usize| u64::from_le_bytes(info[o..o + 8].try_into().unwrap());
    let u32_at = |o: usize| u32::from_le_bytes(info[o..o + 4].try_into().unwrap());
    (u64_at(0), u64_at(8), u32_at(16), u32_at(20))
}

#[test]
fn strong_and_weak_references_reach_the_owner() {
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 4);
    let handle = publish(&p, &q);
    assert_eq!(handle, 1);
    assert_eq!(node_debug_info(&p, 0), (NODE_PTR, NODE_COOKIE, 1, 1));

    // Only the context manager may ask for a ref's counts.
    let mut info = [0u8; NODE_INFO_FOR_REF_SIZE];
    info[..4].copy_from_slice(&handle.to_le_bytes());
    q.ioctl(Q_PID, BINDER_GET_NODE_INFO_FOR_REF, &mut info)
        .unwrap();
    assert_eq!(
        u32::from_le_bytes(info[4..8].try_into().unwrap()),
        1,
        "strong"
    );
    assert_eq!(
        p.ioctl(
            P_PID,
            BINDER_GET_NODE_INFO_FOR_REF,
            &mut [0u8; NODE_INFO_FOR_REF_SIZE]
        ),
        Err(errno::EPERM)
    );

    // Dropping the last strong ref is reported as BR_RELEASE ...
    q.flush(Q_PID, Commands::new().release(handle));
    assert_eq!(
        names(&p.transact(P_PID, &Commands::new())),
        ["BR_NOOP", "BR_RELEASE"]
    );
    assert_eq!(node_debug_info(&p, 0), (NODE_PTR, NODE_COOKIE, 0, 1));
    // ... and the last weak ref as BR_DECREFS, after which the node is gone.
    q.flush(Q_PID, Commands::new().decrefs(handle));
    assert_eq!(
        p.transact(P_PID, &Commands::new()),
        [
            Return::Noop,
            Return::Decrefs {
                ptr: NODE_PTR,
                cookie: NODE_COOKIE
            }
        ]
    );
    assert_eq!(node_debug_info(&p, 0), (0, 0, 0, 0));
    // The handle no longer exists: a strong increment is ignored.
    q.flush(Q_PID, Commands::new().acquire(handle));
    let mut info = [0u8; NODE_INFO_FOR_REF_SIZE];
    info[..4].copy_from_slice(&handle.to_le_bytes());
    assert_eq!(
        q.ioctl(Q_PID, BINDER_GET_NODE_INFO_FOR_REF, &mut info),
        Err(errno::EINVAL)
    );
}

#[test]
fn handle_zero_references_and_context_manager_rules() {
    let driver = Driver::new();
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    // No context manager yet: handle 0 is dead.
    let returns = p.transact(P_PID, Commands::new().transaction(0, 1, 0, &Parcel::new()));
    assert_eq!(names(&returns), ["BR_NOOP", "BR_DEAD_REPLY"]);

    let q = context_manager(&driver, 0);
    // The context manager cannot take a reference to itself ...
    let err = q.talk(Q_PID, Commands::new().acquire(0), 0).unwrap_err();
    assert_eq!(err.0, errno::EINVAL);
    assert_eq!(err.1.write_consumed, 0);
    // ... nor call itself.
    let returns = q.transact(Q_PID, Commands::new().transaction(0, 1, 0, &Parcel::new()));
    assert_eq!(names(&returns), ["BR_NOOP", "BR_FAILED_REPLY"]);
    let mut ee = [0u8; EXTENDED_ERROR_SIZE];
    q.ioctl(Q_PID, BINDER_GET_EXTENDED_ERROR, &mut ee).unwrap();
    assert_eq!(
        u32::from_le_bytes(ee[4..8].try_into().unwrap()),
        BR_FAILED_REPLY
    );
    assert_eq!(
        i32::from_le_bytes(ee[8..12].try_into().unwrap()),
        -errno::EINVAL
    );
    // Reading clears it.
    q.ioctl(Q_PID, BINDER_GET_EXTENDED_ERROR, &mut ee).unwrap();
    assert_eq!(u32::from_le_bytes(ee[4..8].try_into().unwrap()), BR_OK);

    // BC_INCREFS 0 creates the handle-0 reference implicitly.
    p.flush(P_PID, Commands::new().increfs(0).acquire(0));
    p.flush(P_PID, Commands::new().release(0).decrefs(0));

    // Another uid may not become context manager of this context, even after
    // the first one is gone; another device is its own context.
    q.release();
    let other = Process::open(&driver, Device::Binder, 30, 2000, 0);
    assert_eq!(other.become_context_manager(30, 0), Err(errno::EPERM));
    let hw = Process::open(&driver, Device::HwBinder, 31, 2000, 0);
    assert_eq!(hw.become_context_manager(31, 0), Ok(()));
}

#[test]
fn invalid_handles_and_errors_stop_the_write_stream() {
    let driver = Driver::new();
    let _q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let mut commands = Commands::new();
    commands.transaction(7, 1, 0, &Parcel::new()).increfs(0);
    let first_len = 4 + TRANSACTION_DATA_SIZE;
    // The failed transaction is consumed; the next command waits until the
    // error has been read.
    let talk = p.talk(P_PID, &commands, READ_CAPACITY).unwrap();
    assert_eq!(talk.write_consumed, first_len);
    assert_eq!(names(&talk.returns), ["BR_NOOP", "BR_FAILED_REPLY"]);

    // Unknown commands fail the ioctl without consuming them.
    let err = p
        .talk(
            P_PID,
            Commands::new().raw(&(0x6300u32 | 99).to_le_bytes()),
            0,
        )
        .unwrap_err();
    assert_eq!((err.0, err.1.write_consumed), (errno::EINVAL, 0));
    // A truncated command is a fault.
    let err = p
        .talk(P_PID, Commands::new().raw(&BC_INCREFS.to_le_bytes()), 0)
        .unwrap_err();
    assert_eq!(err.0, errno::EFAULT);
    // Unsupported and freezer commands.
    let mut freeze = BC_REQUEST_FREEZE_NOTIFICATION.to_le_bytes().to_vec();
    freeze.extend_from_slice(&[0; HANDLE_COOKIE_SIZE]);
    let err = p.talk(P_PID, Commands::new().raw(&freeze), 0).unwrap_err();
    assert_eq!(err.0, errno::EINVAL);
}

#[test]
fn ioctl_surface() {
    let driver = Driver::new();
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let mut freeze = [0u8; FREEZE_INFO_SIZE];
    freeze[..4].copy_from_slice(&(P_PID as u32).to_le_bytes());
    freeze[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        p.ioctl(P_PID, BINDER_FREEZE, &mut freeze),
        Err(errno::EINVAL)
    );
    assert_eq!(
        p.ioctl(
            P_PID,
            BINDER_GET_FROZEN_INFO,
            &mut [0u8; FROZEN_STATUS_INFO_SIZE]
        ),
        Err(errno::EINVAL)
    );
    assert_eq!(
        p.ioctl(P_PID, BINDER_SET_IDLE_TIMEOUT, &mut [0u8; 8]),
        Err(errno::EINVAL)
    );
    // Wrong argument size for the command.
    assert_eq!(
        p.ioctl(P_PID, BINDER_VERSION, &mut [0u8; 8]),
        Err(errno::EINVAL)
    );
    // The mapping is read-only and installed once.
    let memory = HeapReceiveMemory::new(4096);
    assert_eq!(
        driver.mmap(p.handle, memory.address(), 4096, false, memory.clone()),
        Err(errno::EBUSY)
    );
    let fresh = driver.open(
        Device::Binder,
        aim_binder_driver::Credentials {
            pid: 1,
            euid: 0,
            security_context: None,
        },
    );
    assert_eq!(
        driver.mmap(fresh, memory.address(), 4096, true, memory),
        Err(errno::EPERM)
    );
    // BINDER_THREAD_EXIT forgets the thread; it is recreated on next use.
    p.ioctl(P_PID + 1, BINDER_THREAD_EXIT, &mut [0u8; 4])
        .unwrap();
    // A new thread's first read returns without work.
    assert_eq!(names(&p.transact(P_PID + 2, &Commands::new())), ["BR_NOOP"]);
}

#[test]
fn nonblocking_reads_and_interrupts() {
    let driver = Driver::new();
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    driver.set_nonblocking(p.handle, true);
    let err = p.talk(P_PID, &Commands::new(), READ_CAPACITY).unwrap_err();
    assert_eq!(err.0, errno::EAGAIN);
    driver.set_nonblocking(p.handle, false);

    let reader = {
        let p = p.clone();
        thread::spawn(move || {
            p.talk(P_PID, &Commands::new(), READ_CAPACITY)
                .map(|_| ())
                .map_err(|e| e.0)
        })
    };
    thread::sleep(Duration::from_millis(20));
    driver.interrupt(p.handle, P_PID);
    assert_eq!(reader.join().unwrap(), Err(errno::EINTR));
}

/// A read that would wait parks instead: its resume runs when work
/// arrives (on the thread that queued it), on an interrupt and on release,
/// and the ioctl issued again with its argument as the park left it reads
/// on without redoing the writes.
#[test]
fn parked_reads_resume_on_work_interrupts_and_release() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let resumed = Arc::new(AtomicUsize::new(0));
    let resume = || -> aim_binder_driver::Resume {
        let resumed = resumed.clone();
        Box::new(move || {
            resumed.fetch_add(1, Ordering::SeqCst);
        })
    };
    let mut read = vec![0u8; READ_CAPACITY];
    let mut arg = WriteRead {
        read_size: read.len() as u64,
        read_buffer: read.as_mut_ptr() as u64,
        ..Default::default()
    }
    .encode();
    assert_eq!(
        q.ioctl_or_park(Q_PID, BINDER_WRITE_READ, &mut arg, resume()),
        None
    );
    assert_eq!(resumed.load(Ordering::SeqCst), 0);

    // P's one-way call resumes it before P's ioctl returns.
    p.transact(
        P_PID,
        Commands::new().transaction(0, 5, TF_ONE_WAY, &Parcel::new()),
    );
    assert_eq!(resumed.load(Ordering::SeqCst), 1);
    q.ioctl(Q_PID, BINDER_WRITE_READ, &mut arg).unwrap();
    let bwr = WriteRead::decode(&arg);
    let returns = parse_returns(&read[..bwr.read_consumed as usize]);
    assert_eq!(names(&returns), ["BR_NOOP", "BR_TRANSACTION"]);
    let (tr, _) = find_transaction(&returns);
    assert_eq!(tr.code, 5);

    // An interrupt resumes it, and the retry fails with EINTR.
    let free = Commands::new().free_buffer(tr.buffer).bytes.clone();
    let mut arg = WriteRead {
        write_size: free.len() as u64,
        write_buffer: free.as_ptr() as u64,
        read_size: read.len() as u64,
        read_buffer: read.as_mut_ptr() as u64,
        ..Default::default()
    }
    .encode();
    assert_eq!(
        q.ioctl_or_park(Q_PID, BINDER_WRITE_READ, &mut arg, resume()),
        None
    );
    assert_eq!(WriteRead::decode(&arg).write_consumed, free.len() as u64);
    driver.interrupt(q.handle, Q_PID);
    assert_eq!(resumed.load(Ordering::SeqCst), 2);
    assert_eq!(
        q.ioctl_or_park(Q_PID, BINDER_WRITE_READ, &mut arg, resume()),
        Some(Err(errno::EINTR))
    );

    // Release resumes it, and the retry finds the file gone.
    assert_eq!(
        q.ioctl_or_park(Q_PID, BINDER_WRITE_READ, &mut arg, resume()),
        None
    );
    q.release();
    assert_eq!(resumed.load(Ordering::SeqCst), 3);
    assert_eq!(
        q.ioctl(Q_PID, BINDER_WRITE_READ, &mut arg),
        Err(errno::EBADF)
    );
}

#[test]
fn oneway_calls_are_serialized_per_node() {
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    for code in [1, 2] {
        let returns = p.transact(
            P_PID,
            Commands::new().transaction(0, code, TF_ONE_WAY, &Parcel::new()),
        );
        // One-way completion is immediate.
        assert_eq!(names(&returns), ["BR_NOOP", "BR_TRANSACTION_COMPLETE"]);
    }
    let first = q.transact(Q_PID, &Commands::new());
    let (tr, _) = find_transaction(&first);
    assert_eq!(
        (tr.code, tr.flags & TF_ONE_WAY, tr.sender_pid),
        (1, TF_ONE_WAY, 0)
    );
    assert_eq!(tr.sender_euid, 10_001);
    // The second waits for the first buffer to be freed.
    driver.set_nonblocking(q.handle, true);
    assert_eq!(
        q.talk(Q_PID, &Commands::new(), READ_CAPACITY)
            .unwrap_err()
            .0,
        errno::EAGAIN
    );
    driver.set_nonblocking(q.handle, false);
    let second = q.transact(Q_PID, Commands::new().free_buffer(tr.buffer));
    assert_eq!(find_transaction(&second).0.code, 2);
}

#[test]
fn oneway_flood_is_reported_as_spam() {
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let mut big = Parcel::new();
    big.data = vec![0x5a; 45 * 1024];
    let mut seen = Vec::new();
    for _ in 0..12 {
        let returns = p.transact(P_PID, Commands::new().transaction(0, 1, TF_ONE_WAY, &big));
        seen.push(returns[1].name());
    }
    assert!(seen.contains(&"BR_ONEWAY_SPAM_SUSPECT"), "{seen:?}");
    assert_eq!(
        seen.iter()
            .filter(|n| **n == "BR_ONEWAY_SPAM_SUSPECT")
            .count(),
        1
    );
    // The async half of the mapping is exhausted.
    assert_eq!(seen.last(), Some(&"BR_FAILED_REPLY"));
    drop(q);
}

#[test]
fn file_descriptors_move_to_the_receiver() {
    let driver = Driver::new();
    let q = context_manager(&driver, FLAT_BINDER_FLAG_ACCEPTS_FDS);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let file: aim_binder_driver::File = Arc::new(TestFile("ashmem".into()));
    let fd = p.add_file(file.clone());
    let mut parcel = Parcel::new();
    parcel.write_i32(1).write_fd(fd);
    let caller = {
        let p = p.clone();
        thread::spawn(move || {
            p.transact(
                P_PID,
                Commands::new().transaction(0, 1, TF_ACCEPT_FDS, &parcel),
            )
        })
    };
    let request = q.transact(Q_PID, &Commands::new());
    let (tr, _) = find_transaction(&request);
    let offset = offsets_of(&tr)[0] as usize;
    let data = data_of(&tr);
    assert_eq!(
        u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()),
        BINDER_TYPE_FD
    );
    let received = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
    assert!(Arc::ptr_eq(&q.file(received).unwrap(), &file));
    // A reply may carry fds only if the caller accepts them.
    let mut with_fd = Parcel::new();
    with_fd.write_fd(received);
    q.transact(Q_PID, Commands::new().reply(0, &with_fd));
    let returns = caller.join().unwrap();
    let reply = find_reply(&returns);
    let offset = offsets_of(&reply)[0] as usize;
    let back = u32::from_le_bytes(data_of(&reply)[offset + 8..offset + 12].try_into().unwrap());
    assert!(Arc::ptr_eq(&p.file(back).unwrap(), &file));

    // Without TF_ACCEPT_FDS on the call, a reply with an fd fails: the
    // replier sees completion, the caller BR_FAILED_REPLY.
    let caller = {
        let p = p.clone();
        thread::spawn(move || {
            p.transact(P_PID, Commands::new().transaction(0, 1, 0, &Parcel::new()))
        })
    };
    let (tr2, _) = find_transaction(&q.transact(Q_PID, Commands::new().free_buffer(tr.buffer)));
    let replied = q.transact(Q_PID, Commands::new().reply(0, &with_fd));
    assert_eq!(names(&replied), ["BR_NOOP", "BR_TRANSACTION_COMPLETE"]);
    let returns = caller.join().unwrap();
    assert!(
        names(&returns).contains(&"BR_FAILED_REPLY"),
        "{:?}",
        names(&returns)
    );
    q.flush(Q_PID, Commands::new().free_buffer(tr2.buffer));

    // A node that does not accept fds refuses them.
    let driver = Driver::new();
    let _q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let fd = p.add_file(file);
    let mut parcel = Parcel::new();
    parcel.write_fd(fd);
    let returns = p.transact(
        P_PID,
        Commands::new().transaction(0, 1, TF_ACCEPT_FDS, &parcel),
    );
    assert_eq!(names(&returns), ["BR_NOOP", "BR_FAILED_REPLY"]);
    let mut ee = [0u8; EXTENDED_ERROR_SIZE];
    p.ioctl(P_PID, BINDER_GET_EXTENDED_ERROR, &mut ee).unwrap();
    assert_eq!(
        i32::from_le_bytes(ee[8..12].try_into().unwrap()),
        -errno::EPERM
    );
}

#[test]
fn scatter_gather_buffers_and_fd_arrays() {
    let driver = Driver::new();
    let q = context_manager(&driver, FLAT_BINDER_FLAG_ACCEPTS_FDS);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let files: Vec<aim_binder_driver::File> = (0..2)
        .map(|i| Arc::new(TestFile(format!("f{i}"))) as _)
        .collect();
    let fds: Vec<u32> = files.iter().map(|f| p.add_file(f.clone())).collect();

    let mut commands = Commands::new();
    // Parent: 8-byte pointer slot (fixed up to the child) + two fds.
    let mut parent_bytes = vec![0u8; 16];
    parent_bytes[8..12].copy_from_slice(&fds[0].to_le_bytes());
    parent_bytes[12..16].copy_from_slice(&fds[1].to_le_bytes());
    let child_bytes = b"hidl-string!".to_vec();
    let parent_addr = commands.keep_payload(&parent_bytes);
    let child_addr = commands.keep_payload(&child_bytes);
    // Point the sender's parent at the sender's child, as HIDL does.
    // SAFETY: parent_addr is a live 16-byte allocation owned by `commands`.
    unsafe { (parent_addr as *mut u64).write_unaligned(child_addr) };

    let mut parcel = Parcel::new();
    parcel.write_raw_object(
        &BufferObject {
            flags: 0,
            buffer: parent_addr,
            length: 16,
            parent: 0,
            parent_offset: 0,
        }
        .encode(),
    );
    parcel.write_raw_object(
        &BufferObject {
            flags: BINDER_BUFFER_FLAG_HAS_PARENT,
            buffer: child_addr,
            length: child_bytes.len() as u64,
            parent: 0,
            parent_offset: 0,
        }
        .encode(),
    );
    parcel.write_raw_object(
        &FdArrayObject {
            num_fds: 2,
            parent: 0,
            parent_offset: 8,
        }
        .encode(),
    );
    commands.transaction_sg(0, 1, TF_ACCEPT_FDS, &parcel, 16 + 16);

    let caller = {
        let p = p.clone();
        thread::spawn(move || p.transact(P_PID, &commands))
    };
    let request = q.transact(Q_PID, &Commands::new());
    let (tr, _) = find_transaction(&request);
    let data = data_of(&tr);
    let parent = BufferObject::decode(&data[0..BUFFER_OBJECT_SIZE]);
    let child = BufferObject::decode(&data[BUFFER_OBJECT_SIZE..2 * BUFFER_OBJECT_SIZE]);
    // Both payloads were copied into Q's buffer, after the offsets.
    let base = tr.buffer;
    let limit = base + BINDER_VM_SIZE as u64;
    assert!(parent.buffer > tr.offsets && parent.buffer < limit);
    assert_eq!(read_guest(child.buffer, child_bytes.len()), child_bytes);
    // The parent's pointer now points at the child's copy.
    let copied_parent = read_guest(parent.buffer, 16);
    assert_eq!(
        u64::from_le_bytes(copied_parent[..8].try_into().unwrap()),
        child.buffer
    );
    // The fd array holds Q's new fds.
    let received: Vec<u32> = copied_parent[8..16]
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    for (fd, file) in received.iter().zip(&files) {
        assert!(Arc::ptr_eq(&q.file(*fd).unwrap(), file));
    }
    let before = q.open_fds();
    q.transact(Q_PID, Commands::new().reply(0, &Parcel::new()));
    // Freeing the buffer closes the fds received through the array.
    q.flush(Q_PID, Commands::new().free_buffer(tr.buffer));
    assert_eq!(q.open_fds(), before - 2);
    caller.join().unwrap();
}

#[test]
fn nested_calls_return_to_the_waiting_thread() {
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 4);
    // P has an idle looper thread too; the nested call must not go there.
    let spare = P_PID + 1;
    assert_eq!(
        p.transact(spare, Commands::new().enter_looper()),
        [Return::SpawnLooper]
    );
    let spare_reader = {
        let p = p.clone();
        thread::spawn(move || p.transact(spare, &Commands::new()))
    };
    thread::sleep(Duration::from_millis(10));

    let caller = {
        let p = p.clone();
        thread::spawn(move || {
            // Outer call carrying P's node.
            let first = p.transact(
                P_PID,
                Commands::new().transaction(
                    0,
                    1,
                    TF_ACCEPT_FDS,
                    &with_binder(NODE_PTR, NODE_COOKIE),
                ),
            );
            // P_PID receives the nested call while it waits for its reply.
            let (nested, _) = find_transaction(&first);
            assert_eq!(nested.code, 99);
            assert_eq!((nested.target, nested.cookie), (NODE_PTR, NODE_COOKIE));
            let mut answer = Parcel::new();
            answer.write_i32(7);
            let mut reply = Commands::new();
            reply
                .increfs_done(NODE_PTR, NODE_COOKIE)
                .acquire_done(NODE_PTR, NODE_COOKIE)
                .free_buffer(nested.buffer)
                .reply(0, &answer);
            let second = p.transact(P_PID, &reply);
            let mut all = first;
            all.extend(second);
            // The outer reply may need one more read.
            if !all.iter().any(|r| matches!(r, Return::Reply(_))) {
                all.extend(p.read_until(P_PID, |r| matches!(r, Return::Reply(_))));
            }
            all
        })
    };
    let request = q.transact(Q_PID, &Commands::new());
    let (outer, _) = find_transaction(&request);
    let handle = object_of(&outer, 0).handle();
    // Q calls back into P from the thread serving P's call.
    let mut nested = Commands::new();
    nested.transaction(handle, 99, TF_ACCEPT_FDS, &Parcel::new());
    let returns = q.transact(Q_PID, &nested);
    let returns = if returns.iter().any(|r| matches!(r, Return::Reply(_))) {
        returns
    } else {
        q.read_until(Q_PID, |r| matches!(r, Return::Reply(_)))
    };
    let answer = find_reply(&returns);
    assert_eq!(data_of(&answer), 7i32.to_le_bytes());
    q.flush(Q_PID, Commands::new().free_buffer(answer.buffer));
    q.transact(Q_PID, Commands::new().reply(0, &Parcel::new()));
    let all = caller.join().unwrap();
    assert!(names(&all).contains(&"BR_REPLY"));
    // The spare looper saw nothing but a later release of the node.
    q.flush(Q_PID, Commands::new().free_buffer(outer.buffer));
    let spare_saw = spare_reader.join().unwrap();
    assert!(
        !names(&spare_saw).contains(&"BR_TRANSACTION"),
        "{:?}",
        names(&spare_saw)
    );
}

#[test]
fn death_notifications_and_their_clearing() {
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let handle = publish(&p, &q);

    // Clearing a live registration completes at once.
    q.flush(Q_PID, Commands::new().request_death(handle, 0xc1));
    let returns = q.transact(Q_PID, Commands::new().clear_death(handle, 0xc1));
    assert_eq!(returns, [Return::Noop, Return::ClearDeathDone(0xc1)]);

    // A registration fires when the owner dies.
    q.flush(Q_PID, Commands::new().request_death(handle, 0xc2));
    p.release();
    assert_eq!(
        q.transact(Q_PID, &Commands::new()),
        [Return::Noop, Return::DeadBinder(0xc2)]
    );
    q.flush(Q_PID, Commands::new().dead_binder_done(0xc2));
    // Calls to the dead node fail.
    let returns = q.transact(
        Q_PID,
        Commands::new().transaction(handle, 1, 0, &Parcel::new()),
    );
    assert_eq!(names(&returns), ["BR_NOOP", "BR_DEAD_REPLY"]);
    // Registering on an already dead node fires immediately.
    q.flush(Q_PID, Commands::new().clear_death(handle, 0xc2));
    let returns = q.transact(Q_PID, Commands::new().request_death(handle, 0xc3));
    assert_eq!(
        names(&returns),
        ["BR_NOOP", "BR_CLEAR_DEATH_NOTIFICATION_DONE"]
    );
    let returns = q.transact(Q_PID, &Commands::new());
    assert_eq!(returns, [Return::Noop, Return::DeadBinder(0xc3)]);
}

#[test]
fn peer_death_during_a_call() {
    // The server dies while the caller waits: BR_DEAD_REPLY.
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    let caller = {
        let p = p.clone();
        thread::spawn(move || {
            p.transact(P_PID, Commands::new().transaction(0, 1, 0, &Parcel::new()))
        })
    };
    let request = q.transact(Q_PID, &Commands::new());
    find_transaction(&request);
    q.release();
    assert_eq!(
        names(&caller.join().unwrap()),
        ["BR_NOOP", "BR_TRANSACTION_COMPLETE", "BR_DEAD_REPLY"]
    );

    // The caller dies while the server works: the reply just completes.
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    p.flush(P_PID, Commands::new().transaction(0, 1, 0, &Parcel::new()));
    let request = q.transact(Q_PID, &Commands::new());
    let (tr, _) = find_transaction(&request);
    p.release();
    let replied = q.transact(Q_PID, Commands::new().reply(0, &Parcel::new()));
    assert_eq!(names(&replied), ["BR_NOOP", "BR_TRANSACTION_COMPLETE"]);
    q.flush(Q_PID, Commands::new().free_buffer(tr.buffer));
    // A reply with nothing to reply to is a protocol error.
    let returns = q.transact(Q_PID, Commands::new().reply(0, &Parcel::new()));
    assert_eq!(names(&returns), ["BR_NOOP", "BR_FAILED_REPLY"]);
}

#[test]
fn security_context_only_for_nodes_that_ask() {
    let driver = Driver::new();
    let q = Process::open(&driver, Device::Binder, Q_PID, 1000, 0);
    q.become_context_manager(Q_PID, FLAT_BINDER_FLAG_TXN_SECURITY_CTX)
        .unwrap();
    q.flush(Q_PID, Commands::new().enter_looper());
    let p = Process::open_with(
        &driver,
        Device::Binder,
        P_PID,
        10_001,
        Some("u:r:untrusted_app:s0"),
        0,
    );
    let mut parcel = Parcel::new();
    parcel.data = vec![1, 2, 3];
    p.flush(
        P_PID,
        Commands::new().transaction(0, 1, TF_ONE_WAY, &parcel),
    );
    let (tr, secctx) = find_transaction(&q.transact(Q_PID, &Commands::new()));
    assert_eq!(data_of(&tr), [1, 2, 3]);
    assert_eq!(read_c_string(secctx.unwrap()), "u:r:untrusted_app:s0");
    // The context sits inside the transaction's own buffer.
    assert!(secctx.unwrap() > tr.buffer && secctx.unwrap() < tr.buffer + BINDER_VM_SIZE as u64);
}

#[test]
fn poll_and_process_work_notifications() {
    let driver = Driver::new();
    let q = context_manager(&driver, 0);
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    driver.set_notifier(
        q.handle,
        Arc::new(move |_| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
    );
    assert!(!driver.poll(q.handle, Q_PID).unwrap());
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 0);
    p.transact(
        P_PID,
        Commands::new().transaction(0, 1, TF_ONE_WAY, &Parcel::new()),
    );
    assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    assert!(driver.poll(q.handle, Q_PID).unwrap());
}

#[test]
fn pending_trace_nested_graph_is_atomic_and_non_destructive() {
    let driver = Driver::new();
    assert!(driver.pending_trace().records.is_empty());
    driver.start_trace();
    let q = context_manager(&driver, 0);
    let p = Process::open(&driver, Device::Binder, P_PID, 10_001, 4);
    p.flush(P_PID, Commands::new().transaction(0, 1, TF_ACCEPT_FDS, &with_binder(NODE_PTR, NODE_COOKIE)));
    let outer_request = q.transact(Q_PID, &Commands::new());
    let (outer, _) = find_transaction(&outer_request);
    let outer_id = driver.pending_trace().records[0].id;
    q.flush(Q_PID, Commands::new().transaction(object_of(&outer, 0).handle(), 99, TF_ACCEPT_FDS, &Parcel::new()));
    let nested_request = p.transact(P_PID, &Commands::new());
    let (nested, _) = find_transaction(&nested_request);
    let first = driver.pending_trace();
    assert_eq!(first.records.len(), 2);
    let inner = first.records.iter().find(|record| record.record.code == 99).unwrap();
    assert!(!inner.returning);
    assert_eq!(inner.record.from_parent, Some(outer_id));
    assert_eq!(inner.record.to_parent, Some(outer_id));
    assert_eq!((inner.record.from_pid, inner.record.to_pid), (Q_PID, P_PID));
    assert_eq!((inner.record.from_tid, inner.record.to_tid), (Q_PID, P_PID));
    assert_eq!((inner.from_stack, inner.to_stack), (Some(inner.id), Some(inner.id)));
    assert!(inner.record.delivered.is_some());
    let inner_id = inner.id;
    let repeated = driver.pending_trace();
    assert_eq!(repeated.records.len(), 2);
    assert!(repeated.records.iter().find(|r| r.id == inner_id).unwrap().age >= inner.age);
    assert!(driver.take_trace().is_empty());
    p.flush(P_PID, Commands::new().reply(0, &Parcel::new()));
    let replying = driver.pending_trace();
    let inner = replying.records.iter().find(|r| r.record.transaction_id == inner_id).unwrap();
    assert!(inner.returning);
    assert!(inner.record.latency.is_some());
    assert_eq!(inner.record.to_parent, Some(outer_id));
    let nested_reply = q.transact(Q_PID, &Commands::new());
    assert!(names(&nested_reply).contains(&"BR_REPLY"));
    q.flush(Q_PID, Commands::new().reply(0, &Parcel::new()));
    let outer_reply = p.transact(P_PID, &Commands::new());
    assert!(names(&outer_reply).contains(&"BR_REPLY"));
    assert!(driver.pending_trace().records.is_empty());
    let done = driver.take_trace();
    assert_eq!(done.len(), 2);
    assert!(done.iter().all(|r| r.returned.is_some()));
    assert!(driver.take_trace().is_empty());
    p.flush(P_PID, Commands::new().free_buffer(nested.buffer).free_buffer(find_reply(&outer_reply).buffer));
    q.flush(Q_PID, Commands::new().free_buffer(outer.buffer).free_buffer(find_reply(&nested_reply).buffer));
}
