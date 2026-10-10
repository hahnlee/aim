//! In-process multi-"process" harness: every guest process is a driver
//! process with its own receive mapping and fd table, all in this address
//! space, so guest pointers are host pointers exactly as they are for the
//! in-process syscall layer. Threads of a guest process are OS threads with
//! distinct tids.
//!
//! Commands are built and returns parsed at the byte level, the way
//! libbinder's IPCThreadState writes `mOut` and reads `mIn`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_driver::uapi::*;
use aim_binder_driver::{
    Credentials, Device, Driver, Errno, File, GuestProcess, HeapReceiveMemory, ProcHandle, Resume,
    errno,
};

/// libbinder's `BINDER_VM_SIZE` with 16 KiB pages: 1 MiB minus two pages.
pub const BINDER_VM_SIZE: usize = 1024 * 1024 - 2 * 16 * 1024;
/// `IPCThreadState::mIn` capacity used by `talkWithDriver`.
pub const READ_CAPACITY: usize = 256;

/// A file passed through binder in tests.
#[derive(Debug)]
pub struct TestFile(pub String);

#[derive(Default)]
struct FdTable {
    files: HashMap<u32, File>,
    next: u32,
}

pub struct Process {
    pub driver: Arc<Driver>,
    pub handle: ProcHandle,
    pub memory: Arc<HeapReceiveMemory>,
    pub pid: i32,
    files: Mutex<FdTable>,
}

/// The per-ioctl view of a harness process.
struct Guest<'a> {
    files: &'a Mutex<FdTable>,
}

impl GuestProcess for Guest<'_> {
    fn copy_from_user(&mut self, address: u64, out: &mut [u8]) -> Result<(), Errno> {
        if out.is_empty() {
            return Ok(());
        }
        if address == 0 {
            return Err(errno::EFAULT);
        }
        // SAFETY: harness guest addresses point at live test allocations.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, out.as_mut_ptr(), out.len()) };
        Ok(())
    }

    fn copy_to_user(&mut self, address: u64, data: &[u8]) -> Result<(), Errno> {
        if data.is_empty() {
            return Ok(());
        }
        if address == 0 {
            return Err(errno::EFAULT);
        }
        // SAFETY: as above.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
        Ok(())
    }

    fn get_file(&mut self, fd: u32) -> Result<File, Errno> {
        self.files
            .lock()
            .unwrap()
            .files
            .get(&fd)
            .cloned()
            .ok_or(errno::EBADF)
    }

    fn install_file(&mut self, file: File) -> Result<u32, Errno> {
        let mut table = self.files.lock().unwrap();
        let fd = table.next.max(3);
        table.next = fd + 1;
        table.files.insert(fd, file);
        Ok(fd)
    }

    fn close_fd(&mut self, fd: u32) {
        self.files.lock().unwrap().files.remove(&fd);
    }
}

impl Process {
    /// `ProcessState::open_driver` + `mmap`: open, check the version, set
    /// the thread pool size, enable one-way spam detection, map.
    pub fn open(
        driver: &Arc<Driver>,
        device: Device,
        pid: i32,
        uid: u32,
        max_threads: u32,
    ) -> Arc<Self> {
        Self::open_with(driver, device, pid, uid, None, max_threads)
    }

    pub fn open_with(
        driver: &Arc<Driver>,
        device: Device,
        pid: i32,
        uid: u32,
        security_context: Option<&str>,
        max_threads: u32,
    ) -> Arc<Self> {
        let handle = driver.open(
            device,
            Credentials {
                pid,
                euid: uid,
                security_context: security_context.map(str::to_owned),
            },
        );
        let memory = HeapReceiveMemory::new(BINDER_VM_SIZE);
        let process = Arc::new(Self {
            driver: driver.clone(),
            handle,
            memory: memory.clone(),
            pid,
            files: Mutex::new(FdTable::default()),
        });
        let mut version = [0u8; 4];
        process.ioctl(pid, BINDER_VERSION, &mut version).unwrap();
        assert_eq!(i32::from_le_bytes(version), CURRENT_PROTOCOL_VERSION);
        process
            .ioctl(pid, BINDER_SET_MAX_THREADS, &mut max_threads.to_le_bytes())
            .unwrap();
        process
            .ioctl(
                pid,
                BINDER_ENABLE_ONEWAY_SPAM_DETECTION,
                &mut 1u32.to_le_bytes(),
            )
            .unwrap();
        driver
            .mmap(handle, memory.address(), BINDER_VM_SIZE, false, memory)
            .unwrap();
        process
    }

    pub fn ioctl(&self, tid: i32, cmd: u32, arg: &mut [u8]) -> Result<(), Errno> {
        let mut guest = Guest { files: &self.files };
        self.driver.ioctl(self.handle, tid, cmd, arg, &mut guest)
    }

    /// [`Process::ioctl`] through `Driver::ioctl_or_park`.
    pub fn ioctl_or_park(
        &self,
        tid: i32,
        cmd: u32,
        arg: &mut [u8],
        resume: Resume,
    ) -> Option<Result<(), Errno>> {
        let mut guest = Guest { files: &self.files };
        self.driver
            .ioctl_or_park(self.handle, tid, cmd, arg, &mut guest, resume)
    }

    /// `BINDER_SET_CONTEXT_MGR_EXT` as servicemanager issues it.
    pub fn become_context_manager(&self, tid: i32, flags: u32) -> Result<(), Errno> {
        let mut obj = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags,
            binder: 0,
            cookie: 0,
        }
        .encode();
        self.ioctl(tid, BINDER_SET_CONTEXT_MGR_EXT, &mut obj)
    }

    /// One `BINDER_WRITE_READ`: returns the read bytes and write_consumed.
    pub fn talk(
        &self,
        tid: i32,
        commands: &Commands,
        read_size: usize,
    ) -> Result<Talk, (Errno, Talk)> {
        let mut read = vec![0u8; read_size];
        let bwr = WriteRead {
            write_size: commands.bytes.len() as u64,
            write_consumed: 0,
            write_buffer: commands.bytes.as_ptr() as u64,
            read_size: read_size as u64,
            read_consumed: 0,
            read_buffer: read.as_mut_ptr() as u64,
        };
        let mut arg = bwr.encode();
        let result = self.ioctl(tid, BINDER_WRITE_READ, &mut arg);
        let bwr = WriteRead::decode(&arg);
        read.truncate(bwr.read_consumed as usize);
        let talk = Talk {
            write_consumed: bwr.write_consumed as usize,
            returns: parse_returns(&read),
            raw: read,
        };
        match result {
            Ok(()) => Ok(talk),
            Err(e) => Err((e, talk)),
        }
    }

    /// `talkWithDriver(true)` with `mIn` empty.
    pub fn transact(&self, tid: i32, commands: &Commands) -> Vec<Return> {
        let talk = self
            .talk(tid, commands, READ_CAPACITY)
            .expect("BINDER_WRITE_READ");
        assert_eq!(
            talk.write_consumed,
            commands.bytes.len(),
            "all commands consumed"
        );
        talk.returns
    }

    /// `talkWithDriver(false)`: write only.
    pub fn flush(&self, tid: i32, commands: &Commands) {
        let talk = self.talk(tid, commands, 0).expect("BINDER_WRITE_READ");
        assert_eq!(talk.write_consumed, commands.bytes.len());
    }

    /// Read until a return matching `done` arrives, collecting everything.
    pub fn read_until(&self, tid: i32, done: impl Fn(&Return) -> bool) -> Vec<Return> {
        let mut all = Vec::new();
        loop {
            let returns = self.transact(tid, &Commands::new());
            let finished = returns.iter().any(&done);
            all.extend(returns);
            if finished {
                return all;
            }
        }
    }

    pub fn add_file(&self, file: File) -> u32 {
        let mut guest = Guest { files: &self.files };
        guest.install_file(file).unwrap()
    }

    pub fn close_file(&self,fd:u32){self.files.lock().unwrap().files.remove(&fd);}

    pub fn file(&self, fd: u32) -> Option<File> {
        self.files.lock().unwrap().files.get(&fd).cloned()
    }

    pub fn open_fds(&self) -> usize {
        self.files.lock().unwrap().files.len()
    }

    pub fn release(&self) {
        self.driver.release(self.handle);
    }
}

#[derive(Debug)]
pub struct Talk {
    pub write_consumed: usize,
    pub returns: Vec<Return>,
    pub raw: Vec<u8>,
}

/// A parcel as libbinder's `Parcel` lays it out: data plus object offsets.
#[derive(Clone, Default)]
pub struct Parcel {
    pub data: Vec<u8>,
    pub offsets: Vec<u64>,
}

impl Parcel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_i32(&mut self, value: i32) -> &mut Self {
        self.data.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// `Parcel::writeString16`: length, UTF-16 code units, NUL, 4-byte pad.
    pub fn write_string16(&mut self, s: &str) -> &mut Self {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.write_i32(units.len() as i32);
        for unit in units.iter().chain(std::iter::once(&0)) {
            self.data.extend_from_slice(&unit.to_le_bytes());
        }
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }
        self
    }

    /// `Parcel::writeInterfaceToken` (kernel binder, system header).
    pub fn write_interface_token(&mut self, descriptor: &str) -> &mut Self {
        const STRICT_MODE_PENALTY_GATHER: i32 = 1 << 31;
        const UNSET_WORK_SOURCE: i32 = -1;
        const SYSTEM_HEADER: i32 = i32::from_be_bytes(*b"SYST");
        self.write_i32(STRICT_MODE_PENALTY_GATHER);
        self.write_i32(UNSET_WORK_SOURCE);
        self.write_i32(SYSTEM_HEADER);
        self.write_string16(descriptor)
    }

    fn write_object(&mut self, bytes: &[u8]) -> &mut Self {
        self.offsets.push(self.data.len() as u64);
        self.data.extend_from_slice(bytes);
        self
    }

    /// `flattenBinder` of a local BBinder: the object, then the stability.
    pub fn write_local_binder(&mut self, weakrefs: u64, bbinder: u64) -> &mut Self {
        let obj = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
            binder: weakrefs,
            cookie: bbinder,
        };
        self.write_object(&obj.encode());
        self.write_i32(0b000011) // Stability::SYSTEM
    }

    /// `flattenBinder` of a BpBinder: the handle, then the stability.
    pub fn write_handle(&mut self, handle: u32) -> &mut Self {
        let obj = FlatBinderObject {
            kind: BINDER_TYPE_HANDLE,
            flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
            binder: handle as u64,
            cookie: 0,
        };
        self.write_object(&obj.encode());
        self.write_i32(0b000011)
    }

    /// `Parcel::writeFileDescriptor`.
    pub fn write_fd(&mut self, fd: u32) -> &mut Self {
        let mut obj = [0u8; FD_OBJECT_SIZE];
        obj[..4].copy_from_slice(&BINDER_TYPE_FD.to_le_bytes());
        obj[8..12].copy_from_slice(&fd.to_le_bytes());
        obj[16..24].copy_from_slice(&1u64.to_le_bytes()); // cookie: takeOwnership
        self.write_object(&obj)
    }

    pub fn write_raw_object(&mut self, bytes: &[u8]) -> &mut Self {
        self.write_object(bytes)
    }
}

/// An `mOut` buffer. Payloads referenced by transactions are kept alive
/// with it, as the Parcel they come from is in libbinder.
#[derive(Default)]
pub struct Commands {
    pub bytes: Vec<u8>,
    keep: Vec<Box<[u8]>>,
}

impl Commands {
    pub fn new() -> Self {
        Self::default()
    }

    fn word(&mut self, cmd: u32) -> &mut Self {
        self.bytes.extend_from_slice(&cmd.to_le_bytes());
        self
    }

    fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    fn keep(&mut self, bytes: &[u8]) -> u64 {
        if bytes.is_empty() {
            return 0;
        }
        let boxed: Box<[u8]> = bytes.into();
        let address = boxed.as_ptr() as u64;
        self.keep.push(boxed);
        address
    }

    /// `IPCThreadState::writeTransactionData`.
    pub fn transaction_data(
        &mut self,
        cmd: u32,
        handle: u32,
        code: u32,
        flags: u32,
        parcel: &Parcel,
    ) -> &mut Self {
        let offsets: Vec<u8> = parcel
            .offsets
            .iter()
            .flat_map(|o| o.to_le_bytes())
            .collect();
        let tr = TransactionData {
            target: handle as u64,
            cookie: 0,
            code,
            flags,
            sender_pid: 0,
            sender_euid: 0,
            data_size: parcel.data.len() as u64,
            offsets_size: offsets.len() as u64,
            buffer: self.keep(&parcel.data),
            offsets: self.keep(&offsets),
        };
        self.word(cmd);
        self.bytes.extend_from_slice(&tr.encode());
        self
    }

    pub fn transaction(
        &mut self,
        handle: u32,
        code: u32,
        flags: u32,
        parcel: &Parcel,
    ) -> &mut Self {
        self.transaction_data(BC_TRANSACTION, handle, code, flags, parcel)
    }

    /// `sendReply`: handle -1, code 0.
    pub fn reply(&mut self, flags: u32, parcel: &Parcel) -> &mut Self {
        self.transaction_data(BC_REPLY, u32::MAX, 0, flags, parcel)
    }

    /// BC_TRANSACTION_SG with explicit scatter-gather payload size.
    pub fn transaction_sg(
        &mut self,
        handle: u32,
        code: u32,
        flags: u32,
        parcel: &Parcel,
        buffers_size: u64,
    ) -> &mut Self {
        self.transaction_data(BC_TRANSACTION_SG, handle, code, flags, parcel);
        self.u64(buffers_size)
    }

    /// Keep a scatter-gather payload alive and return its address.
    pub fn keep_payload(&mut self, bytes: &[u8]) -> u64 {
        self.keep(bytes)
    }

    pub fn increfs(&mut self, handle: u32) -> &mut Self {
        self.word(BC_INCREFS).u32(handle)
    }
    pub fn acquire(&mut self, handle: u32) -> &mut Self {
        self.word(BC_ACQUIRE).u32(handle)
    }
    pub fn release(&mut self, handle: u32) -> &mut Self {
        self.word(BC_RELEASE).u32(handle)
    }
    pub fn decrefs(&mut self, handle: u32) -> &mut Self {
        self.word(BC_DECREFS).u32(handle)
    }
    pub fn increfs_done(&mut self, ptr: u64, cookie: u64) -> &mut Self {
        self.word(BC_INCREFS_DONE).u64(ptr).u64(cookie)
    }
    pub fn acquire_done(&mut self, ptr: u64, cookie: u64) -> &mut Self {
        self.word(BC_ACQUIRE_DONE).u64(ptr).u64(cookie)
    }
    pub fn free_buffer(&mut self, buffer: u64) -> &mut Self {
        self.word(BC_FREE_BUFFER).u64(buffer)
    }
    pub fn enter_looper(&mut self) -> &mut Self {
        self.word(BC_ENTER_LOOPER)
    }
    pub fn register_looper(&mut self) -> &mut Self {
        self.word(BC_REGISTER_LOOPER)
    }
    pub fn exit_looper(&mut self) -> &mut Self {
        self.word(BC_EXIT_LOOPER)
    }
    pub fn request_death(&mut self, handle: u32, cookie: u64) -> &mut Self {
        self.word(BC_REQUEST_DEATH_NOTIFICATION)
            .u32(handle)
            .u64(cookie)
    }
    pub fn clear_death(&mut self, handle: u32, cookie: u64) -> &mut Self {
        self.word(BC_CLEAR_DEATH_NOTIFICATION)
            .u32(handle)
            .u64(cookie)
    }
    pub fn dead_binder_done(&mut self, cookie: u64) -> &mut Self {
        self.word(BC_DEAD_BINDER_DONE).u64(cookie)
    }
    /// Arbitrary bytes (for malformed-stream tests).
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(bytes);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Return {
    Noop,
    Ok,
    Error(i32),
    SpawnLooper,
    TransactionComplete,
    DeadReply,
    FailedReply,
    FrozenReply,
    OnewaySpamSuspect,
    Transaction {
        tr: TransactionData,
        secctx: Option<u64>,
    },
    Reply(TransactionData),
    Increfs {
        ptr: u64,
        cookie: u64,
    },
    Acquire {
        ptr: u64,
        cookie: u64,
    },
    Release {
        ptr: u64,
        cookie: u64,
    },
    Decrefs {
        ptr: u64,
        cookie: u64,
    },
    DeadBinder(u64),
    ClearDeathDone(u64),
    Unknown(u32),
}

impl Return {
    /// The short name used in golden transcripts.
    pub fn name(&self) -> &'static str {
        match self {
            Return::Noop => "BR_NOOP",
            Return::Ok => "BR_OK",
            Return::Error(_) => "BR_ERROR",
            Return::SpawnLooper => "BR_SPAWN_LOOPER",
            Return::TransactionComplete => "BR_TRANSACTION_COMPLETE",
            Return::DeadReply => "BR_DEAD_REPLY",
            Return::FailedReply => "BR_FAILED_REPLY",
            Return::FrozenReply => "BR_FROZEN_REPLY",
            Return::OnewaySpamSuspect => "BR_ONEWAY_SPAM_SUSPECT",
            Return::Transaction {
                secctx: Some(_), ..
            } => "BR_TRANSACTION_SEC_CTX",
            Return::Transaction { .. } => "BR_TRANSACTION",
            Return::Reply(_) => "BR_REPLY",
            Return::Increfs { .. } => "BR_INCREFS",
            Return::Acquire { .. } => "BR_ACQUIRE",
            Return::Release { .. } => "BR_RELEASE",
            Return::Decrefs { .. } => "BR_DECREFS",
            Return::DeadBinder(_) => "BR_DEAD_BINDER",
            Return::ClearDeathDone(_) => "BR_CLEAR_DEATH_NOTIFICATION_DONE",
            Return::Unknown(_) => "BR_<unknown>",
        }
    }

    pub fn transaction(&self) -> Option<&TransactionData> {
        match self {
            Return::Transaction { tr, .. } | Return::Reply(tr) => Some(tr),
            _ => None,
        }
    }
}

pub fn names(returns: &[Return]) -> Vec<&'static str> {
    returns.iter().map(Return::name).collect()
}

/// Parse an `mIn` buffer the way `IPCThreadState` consumes it.
pub fn parse_returns(bytes: &[u8]) -> Vec<Return> {
    let mut out = Vec::new();
    let mut at = 0;
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    while at + 4 <= bytes.len() {
        let cmd = u32_at(at);
        at += 4;
        let item = match cmd {
            BR_NOOP => Return::Noop,
            BR_OK => Return::Ok,
            BR_ERROR => {
                at += 4;
                Return::Error(u32_at(at - 4) as i32)
            }
            BR_SPAWN_LOOPER => Return::SpawnLooper,
            BR_TRANSACTION_COMPLETE => Return::TransactionComplete,
            BR_DEAD_REPLY => Return::DeadReply,
            BR_FAILED_REPLY => Return::FailedReply,
            BR_FROZEN_REPLY => Return::FrozenReply,
            BR_ONEWAY_SPAM_SUSPECT => Return::OnewaySpamSuspect,
            BR_TRANSACTION | BR_REPLY | BR_TRANSACTION_SEC_CTX => {
                let tr = TransactionData::decode(&bytes[at..at + TRANSACTION_DATA_SIZE]);
                at += TRANSACTION_DATA_SIZE;
                match cmd {
                    BR_REPLY => Return::Reply(tr),
                    BR_TRANSACTION => Return::Transaction { tr, secctx: None },
                    _ => {
                        at += 8;
                        Return::Transaction {
                            tr,
                            secctx: Some(u64_at(at - 8)),
                        }
                    }
                }
            }
            BR_INCREFS | BR_ACQUIRE | BR_RELEASE | BR_DECREFS => {
                let (ptr, cookie) = (u64_at(at), u64_at(at + 8));
                at += 16;
                match cmd {
                    BR_INCREFS => Return::Increfs { ptr, cookie },
                    BR_ACQUIRE => Return::Acquire { ptr, cookie },
                    BR_RELEASE => Return::Release { ptr, cookie },
                    _ => Return::Decrefs { ptr, cookie },
                }
            }
            BR_DEAD_BINDER => {
                at += 8;
                Return::DeadBinder(u64_at(at - 8))
            }
            BR_CLEAR_DEATH_NOTIFICATION_DONE => {
                at += 8;
                Return::ClearDeathDone(u64_at(at - 8))
            }
            other => {
                out.push(Return::Unknown(other));
                break;
            }
        };
        out.push(item);
    }
    out
}

/// Read guest memory (a received buffer) in this address space.
pub fn read_guest(address: u64, len: usize) -> Vec<u8> {
    if len == 0 {
        return Vec::new();
    }
    // SAFETY: the address comes from a BR_TRANSACTION/BR_REPLY in a live
    // harness mapping.
    unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
}

pub fn data_of(tr: &TransactionData) -> Vec<u8> {
    read_guest(tr.buffer, tr.data_size as usize)
}

pub fn offsets_of(tr: &TransactionData) -> Vec<u64> {
    read_guest(tr.offsets, tr.offsets_size as usize)
        .chunks(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// The first flat object in a received transaction.
pub fn object_of(tr: &TransactionData, index: usize) -> FlatBinderObject {
    let offset = offsets_of(tr)[index] as usize;
    FlatBinderObject::decode(&data_of(tr)[offset..offset + FLAT_BINDER_OBJECT_SIZE])
}

pub fn find_transaction(returns: &[Return]) -> (TransactionData, Option<u64>) {
    returns
        .iter()
        .find_map(|r| match r {
            Return::Transaction { tr, secctx } => Some((*tr, *secctx)),
            _ => None,
        })
        .expect("a BR_TRANSACTION")
}

pub fn find_reply(returns: &[Return]) -> TransactionData {
    returns
        .iter()
        .find_map(|r| match r {
            Return::Reply(tr) => Some(*tr),
            _ => None,
        })
        .expect("a BR_REPLY")
}

/// Read a NUL-terminated string at a guest address.
pub fn read_c_string(address: u64) -> String {
    let mut bytes = Vec::new();
    let mut at = address;
    loop {
        let b = read_guest(at, 1)[0];
        if b == 0 {
            break;
        }
        bytes.push(b);
        at += 1;
    }
    String::from_utf8(bytes).unwrap()
}
