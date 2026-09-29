//! A binder process on the host (ADR 0013): native system services join
//! the guest's binder context as one more process of the driver, inside the
//! process that hosts the driver, so a call to them crosses no Mach hop
//! after the caller's own.
//!
//! [`LocalProcess`] plays libbinder's `ProcessState` and `IPCThreadState`
//! over [`Driver`] directly: host threads are its binder threads (each with
//! its own driver thread id), guest addresses are host addresses, and the
//! receive buffer is heap memory. It serves [`Service`]s as nodes, calls
//! other processes' nodes, holds references to them and learns of their
//! death, following the protocol as libbinder uses it.
//!
//! Its nodes do not accept file descriptors, and it asks for none in
//! replies: a transaction that carries one fails in the driver (#433).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_driver::uapi::*;
use aim_binder_driver::{
    Credentials, Device, Driver, Errno, File, GuestProcess, HeapReceiveMemory, ProcHandle, errno,
};

use crate::parcel::{
    Binder, DEAD_OBJECT, FAILED_TRANSACTION, OK, Parcel, Reader, StatusCode, UNKNOWN_TRANSACTION,
};

/// libbinder's `BINDER_VM_SIZE` with 16 KiB pages.
const VM_SIZE: usize = 1024 * 1024 - 2 * 16 * 1024;
/// libbinder's `DEFAULT_MAX_BINDER_THREADS`.
const MAX_THREADS: u32 = 15;
const READ_SIZE: usize = 4096;

/// `IBinder::INTERFACE_TRANSACTION` and `PING_TRANSACTION`.
const INTERFACE_TRANSACTION: u32 = u32::from_be_bytes(*b"_NTF");
const PING_TRANSACTION: u32 = u32::from_be_bytes(*b"_PNG");

/// What a service answers a call with: the reply, or a status that the
/// caller receives instead of one (`TF_STATUS_CODE`).
pub type Reply = Result<Parcel, StatusCode>;

/// A binder node served on the host.
pub trait Service: Send + Sync {
    /// Its interface descriptor (`INTERFACE_TRANSACTION`).
    fn descriptor(&self) -> &str;
    /// Handles one call; a one-way call's reply is dropped.
    fn transact(&self, call: &mut Call<'_>) -> Reply;
}

/// An incoming call, with the caller's identity from the driver.
pub struct Call<'a> {
    pub code: u32,
    pub flags: u32,
    pub sender_pid: i32,
    pub sender_euid: u32,
    pub data: Reader<'a>,
}

impl Call<'_> {
    pub fn is_oneway(&self) -> bool {
        self.flags & TF_ONE_WAY != 0
    }
}

/// Driver thread ids of this process's host threads.
static NEXT_TID: AtomicI32 = AtomicI32::new(1);

thread_local! {
    static THREAD: ThreadState = ThreadState {
        tid: NEXT_TID.fetch_add(1, Ordering::Relaxed),
        used: RefCell::new(Vec::new()),
        pending: RefCell::new(Pending::default()),
    };
}

/// A host thread's binder identity; on exit it leaves every process it
/// used (`BINDER_THREAD_EXIT`, as `IPCThreadState`'s destructor does).
struct ThreadState {
    tid: i32,
    used: RefCell<Vec<Weak<LocalProcess>>>,
    /// Returns read but not yet taken by the wait they belong to: a
    /// nested call's reply can be read while an outer call waits.
    pending: RefCell<Pending>,
}

#[derive(Default)]
struct Pending {
    complete: usize,
    reply: Option<TransactionData>,
    error: Option<StatusCode>,
}

impl Drop for ThreadState {
    fn drop(&mut self) {
        for process in self.used.borrow().iter().filter_map(Weak::upgrade) {
            let mut arg = [0u8; 4];
            let _ = process.ioctl(self.tid, BINDER_THREAD_EXIT, &mut arg);
        }
    }
}

#[derive(Default)]
struct FileTable {
    files: HashMap<u32, File>,
    next: u32,
}

/// The process as the driver sees it in one ioctl: its memory is this
/// address space.
struct Local<'a> {
    files: &'a Mutex<FileTable>,
}

impl GuestProcess for Local<'_> {
    fn copy_from_user(&mut self, address: u64, out: &mut [u8]) -> Result<(), Errno> {
        if out.is_empty() {
            return Ok(());
        }
        if address == 0 {
            return Err(errno::EFAULT);
        }
        // SAFETY: the driver reads only what this process handed it: its
        // command buffer and the parcels it sends, alive for the ioctl.
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
        // SAFETY: as above: the read buffer and argument of this ioctl.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
        Ok(())
    }

    fn get_file(&mut self, fd: u32) -> Result<File, Errno> {
        let table = self.files.lock().unwrap();
        table.files.get(&fd).cloned().ok_or(errno::EBADF)
    }

    fn install_file(&mut self, file: File) -> Result<u32, Errno> {
        let mut table = self.files.lock().unwrap();
        let fd = table.next;
        table.next += 1;
        table.files.insert(fd, file);
        Ok(fd)
    }

    fn close_fd(&mut self, fd: u32) {
        self.files.lock().unwrap().files.remove(&fd);
    }
}

pub struct LocalProcess {
    driver: Arc<Driver>,
    handle: ProcHandle,
    services: Mutex<HashMap<u64, Arc<dyn Service>>>,
    deaths: Mutex<HashMap<u64, Box<dyn FnOnce() + Send>>>,
    next_cookie: AtomicU64,
    files: Mutex<FileTable>,
    this: Weak<LocalProcess>,
}

/// A received reply; its buffer goes back to the driver when dropped.
pub struct Received {
    process: Arc<LocalProcess>,
    buffer: u64,
    data: &'static [u8],
    objects: Vec<u64>,
}

impl Received {
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(self.data, &self.objects)
    }
}

impl Drop for Received {
    fn drop(&mut self) {
        if self.buffer != 0 {
            let mut out = Commands::default();
            out.u32(BC_FREE_BUFFER).u64(self.buffer);
            self.process.write(&out.0);
        }
    }
}

/// A strong reference to another process's node, released when dropped.
pub struct Strong {
    process: Arc<LocalProcess>,
    pub handle: u32,
}

impl Strong {
    pub fn binder(&self) -> Binder {
        Binder::Handle(self.handle)
    }

    pub fn transact(&self, code: u32, data: &Parcel, oneway: bool) -> Result<Received, StatusCode> {
        self.process.transact(self.handle, code, data, oneway)
    }
}

impl Drop for Strong {
    fn drop(&mut self) {
        let mut out = Commands::default();
        out.u32(BC_RELEASE).u32(self.handle);
        out.u32(BC_DECREFS).u32(self.handle);
        self.process.write(&out.0);
    }
}

#[derive(Default)]
struct Commands(Vec<u8>);

impl Commands {
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
}

/// What a thread waits for in [`LocalProcess::wait`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Until {
    /// A looper: never returns.
    Forever,
    /// `BR_TRANSACTION_COMPLETE` (a one-way call or a reply went out).
    Complete,
    /// `BR_REPLY`.
    Reply,
}

impl LocalProcess {
    /// Opens `device` as a process with `credentials` (`ProcessState`).
    pub fn open(driver: &Arc<Driver>, device: Device, credentials: Credentials) -> Arc<Self> {
        let handle = driver.open(device, credentials);
        let memory = HeapReceiveMemory::new(VM_SIZE);
        let process = Arc::new_cyclic(|this| Self {
            driver: driver.clone(),
            handle,
            services: Mutex::new(HashMap::new()),
            deaths: Mutex::new(HashMap::new()),
            next_cookie: AtomicU64::new(1),
            files: Mutex::new(FileTable {
                files: HashMap::new(),
                next: 3,
            }),
            this: this.clone(),
        });
        let tid = process.tid();
        process
            .ioctl(tid, BINDER_SET_MAX_THREADS, &mut MAX_THREADS.to_le_bytes())
            .expect("BINDER_SET_MAX_THREADS");
        driver
            .mmap(handle, memory.address(), VM_SIZE, false, memory)
            .expect("binder mmap");
        process
    }

    fn arc(&self) -> Arc<Self> {
        self.this.upgrade().expect("a live process")
    }

    /// This thread's driver thread id, registering it for its exit.
    fn tid(&self) -> i32 {
        THREAD.with(|t| {
            let mut used = t.used.borrow_mut();
            if !used.iter().any(|p| p.ptr_eq(&self.this)) {
                used.push(self.this.clone());
            }
            t.tid
        })
    }

    fn ioctl(&self, tid: i32, cmd: u32, arg: &mut [u8]) -> Result<(), Errno> {
        let mut local = Local { files: &self.files };
        self.driver.ioctl(self.handle, tid, cmd, arg, &mut local)
    }

    /// One `BINDER_WRITE_READ`: returns what was consumed and read.
    fn talk(&self, write: &[u8], read: &mut [u8]) -> Result<(usize, usize), Errno> {
        let bwr = WriteRead {
            write_size: write.len() as u64,
            write_consumed: 0,
            write_buffer: write.as_ptr() as u64,
            read_size: read.len() as u64,
            read_consumed: 0,
            read_buffer: read.as_mut_ptr() as u64,
        };
        let mut arg = bwr.encode();
        let tid = self.tid();
        loop {
            match self.ioctl(tid, BINDER_WRITE_READ, &mut arg) {
                Err(errno::EINTR) => continue,
                Err(e) => return Err(e),
                Ok(()) => {
                    let bwr = WriteRead::decode(&arg);
                    return Ok((bwr.write_consumed as usize, bwr.read_consumed as usize));
                }
            }
        }
    }

    /// Commands that need no answer (reference counts, freed buffers).
    fn write(&self, commands: &[u8]) {
        let _ = self.talk(commands, &mut []);
    }

    /// Serves `service` as a node; the binder to hand out for it.
    pub fn add_service(&self, service: Arc<dyn Service>) -> Binder {
        let ptr = self.next_cookie.fetch_add(1, Ordering::Relaxed) << 4;
        self.services.lock().unwrap().insert(ptr, service);
        Binder::Local(ptr)
    }

    /// Starts the thread pool: one looper now, more as the driver asks.
    pub fn start(&self) {
        self.spawn_looper(false);
    }

    fn spawn_looper(&self, registered: bool) {
        let process = self.arc();
        let _ = std::thread::Builder::new()
            .name("binder-local".into())
            .spawn(move || {
                let mut out = Commands::default();
                out.u32(if registered {
                    BC_REGISTER_LOOPER
                } else {
                    BC_ENTER_LOOPER
                });
                let _ = process.wait(&mut out.0, Until::Forever);
            });
    }

    /// Takes a strong reference to a received handle, so that it outlives
    /// the buffer it came in (`BpBinder`).
    pub fn strong(&self, handle: u32) -> Strong {
        let mut out = Commands::default();
        out.u32(BC_INCREFS).u32(handle);
        out.u32(BC_ACQUIRE).u32(handle);
        self.write(&out.0);
        Strong {
            process: self.arc(),
            handle,
        }
    }

    /// Calls `on_death` once when `strong`'s process dies; the returned
    /// cookie clears it ([`LocalProcess::clear_death`]).
    pub fn link_to_death(&self, strong: &Strong, on_death: Box<dyn FnOnce() + Send>) -> u64 {
        let cookie = self.next_cookie.fetch_add(1, Ordering::Relaxed);
        self.deaths.lock().unwrap().insert(cookie, on_death);
        let mut out = Commands::default();
        out.u32(BC_REQUEST_DEATH_NOTIFICATION)
            .u32(strong.handle)
            .u64(cookie);
        self.write(&out.0);
        cookie
    }

    pub fn clear_death(&self, strong: &Strong, cookie: u64) {
        if self.deaths.lock().unwrap().remove(&cookie).is_some() {
            let mut out = Commands::default();
            out.u32(BC_CLEAR_DEATH_NOTIFICATION)
                .u32(strong.handle)
                .u64(cookie);
            self.write(&out.0);
        }
    }

    /// Calls `code` on `handle` (0 is the context manager) and waits for
    /// the reply, serving calls routed to this thread meanwhile.
    pub fn transact(
        &self,
        handle: u32,
        code: u32,
        data: &Parcel,
        oneway: bool,
    ) -> Result<Received, StatusCode> {
        let tr = TransactionData {
            target: u64::from(handle),
            cookie: 0,
            code,
            flags: if oneway { TF_ONE_WAY } else { 0 },
            sender_pid: 0,
            sender_euid: 0,
            data_size: data.data().len() as u64,
            offsets_size: 8 * data.objects().len() as u64,
            buffer: data.data().as_ptr() as u64,
            offsets: data.objects().as_ptr() as u64,
        };
        let mut out = Commands::default();
        out.u32(BC_TRANSACTION).bytes(&tr.encode());
        let reply = self.wait(
            &mut out.0,
            if oneway {
                Until::Complete
            } else {
                Until::Reply
            },
        )?;
        let Some(tr) = reply else {
            return Ok(Received {
                process: self.arc(),
                buffer: 0,
                data: &[],
                objects: Vec::new(),
            });
        };
        // SAFETY: the driver wrote the reply into the receive buffer at
        // `tr.buffer`, which stays allocated until BC_FREE_BUFFER.
        let data: &'static [u8] =
            unsafe { std::slice::from_raw_parts(tr.buffer as *const u8, tr.data_size as usize) };
        let objects = self.offsets(&tr);
        let received = Received {
            process: self.arc(),
            buffer: tr.buffer,
            data,
            objects,
        };
        if tr.flags & TF_STATUS_CODE != 0 {
            let status = data.get(..4).map_or(FAILED_TRANSACTION, |s| {
                i32::from_le_bytes(s.try_into().unwrap())
            });
            if status != OK {
                return Err(status);
            }
        }
        Ok(received)
    }

    fn offsets(&self, tr: &TransactionData) -> Vec<u64> {
        (0..tr.offsets_size as usize / 8)
            .map(|i| {
                // SAFETY: the offsets array of a received buffer.
                unsafe { (tr.offsets as *const u64).add(i).read_unaligned() }
            })
            .collect()
    }

    /// `waitForResponse` and `executeCommand`: talks to the driver and
    /// handles its returns until `until`.
    fn wait(&self, out: &mut Vec<u8>, until: Until) -> Result<Option<TransactionData>, StatusCode> {
        let mut read = vec![0u8; READ_SIZE];
        loop {
            let taken = THREAD.with(|t| {
                let mut p = t.pending.borrow_mut();
                if let Some(error) = p.error.take() {
                    return Some(Err(error));
                }
                match until {
                    Until::Complete if p.complete > 0 => {
                        p.complete -= 1;
                        Some(Ok(None))
                    }
                    Until::Reply if p.reply.is_some() => {
                        // A synchronous call's own completion comes with
                        // its reply.
                        p.complete = 0;
                        Some(Ok(p.reply.take()))
                    }
                    _ => None,
                }
            });
            if let Some(result) = taken {
                return result;
            }
            let (consumed, n) = self.talk(out, &mut read).map_err(|e| -e)?;
            out.drain(..consumed);
            let mut at = 0;
            while at + 4 <= n {
                let cmd = u32::from_le_bytes(read[at..at + 4].try_into().unwrap());
                let payload = read[at + 4..at + 4 + ioc_size(cmd)].to_vec();
                at += 4 + ioc_size(cmd);
                self.execute_command(cmd, &payload, out);
            }
        }
    }

    fn execute_command(&self, cmd: u32, payload: &[u8], out: &mut Vec<u8>) {
        let pending = |f: &dyn Fn(&mut Pending)| THREAD.with(|t| f(&mut t.pending.borrow_mut()));
        match cmd {
            BR_TRANSACTION_COMPLETE => pending(&|p| p.complete += 1),
            BR_REPLY => {
                let tr = TransactionData::decode(payload);
                pending(&|p| p.reply = Some(tr));
            }
            BR_DEAD_REPLY => pending(&|p| p.error = Some(DEAD_OBJECT)),
            BR_FAILED_REPLY | BR_FROZEN_REPLY => pending(&|p| p.error = Some(FAILED_TRANSACTION)),
            BR_TRANSACTION | BR_TRANSACTION_SEC_CTX => {
                self.execute(TransactionData::decode(payload), out);
            }
            BR_INCREFS | BR_ACQUIRE => {
                // Nodes live as long as the process: acknowledge.
                let done = if cmd == BR_INCREFS {
                    BC_INCREFS_DONE
                } else {
                    BC_ACQUIRE_DONE
                };
                out.extend_from_slice(&done.to_le_bytes());
                out.extend_from_slice(payload);
            }
            BR_DEAD_BINDER => {
                let cookie = u64::from_le_bytes(payload.try_into().unwrap());
                if let Some(on_death) = self.deaths.lock().unwrap().remove(&cookie) {
                    on_death();
                }
                out.extend_from_slice(&BC_DEAD_BINDER_DONE.to_le_bytes());
                out.extend_from_slice(&cookie.to_le_bytes());
            }
            BR_SPAWN_LOOPER => self.spawn_looper(true),
            _ => {} // NOOP, OK, RELEASE, DECREFS, death cleared, spam
        }
    }

    /// Serves one incoming call and sends its reply.
    fn execute(&self, tr: TransactionData, out: &mut Vec<u8>) {
        // SAFETY: the call's data in the receive buffer, alive until the
        // BC_FREE_BUFFER queued below.
        let data =
            unsafe { std::slice::from_raw_parts(tr.buffer as *const u8, tr.data_size as usize) };
        let objects = self.offsets(&tr);
        let service = self.services.lock().unwrap().get(&tr.cookie).cloned();
        let mut call = Call {
            code: tr.code,
            flags: tr.flags,
            sender_pid: tr.sender_pid,
            sender_euid: tr.sender_euid,
            data: Reader::new(data, &objects),
        };
        let reply = match (&service, tr.code) {
            (None, _) => Err(UNKNOWN_TRANSACTION),
            (Some(s), INTERFACE_TRANSACTION) => {
                let mut p = Parcel::new();
                p.write_string16(Some(s.descriptor()));
                Ok(p)
            }
            (Some(_), PING_TRANSACTION) => Ok(Parcel::new()),
            (Some(s), _) => s.transact(&mut call),
        };
        out.extend_from_slice(&BC_FREE_BUFFER.to_le_bytes());
        out.extend_from_slice(&tr.buffer.to_le_bytes());
        if call.is_oneway() {
            return;
        }
        let status;
        let (bytes, offsets, flags): (&[u8], &[u64], u32) = match &reply {
            Ok(p) => (p.data(), p.objects(), 0),
            Err(s) => {
                status = s.to_le_bytes();
                (&status, &[], TF_STATUS_CODE)
            }
        };
        let tr = TransactionData {
            target: 0,
            cookie: 0,
            code: 0,
            flags,
            sender_pid: 0,
            sender_euid: 0,
            data_size: bytes.len() as u64,
            offsets_size: 8 * offsets.len() as u64,
            buffer: bytes.as_ptr() as u64,
            offsets: offsets.as_ptr() as u64,
        };
        out.extend_from_slice(&BC_REPLY.to_le_bytes());
        out.extend_from_slice(&tr.encode());
        // The reply's bytes must live until the driver copied them.
        let _ = self.wait(out, Until::Complete);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const ADD: u32 = 1;
    const GET: u32 = 2;
    const ECHO: u32 = 3;
    const WATCH: u32 = 4;

    /// A context manager with `add(name, binder)` and `get(name)`.
    struct Registry {
        process: Mutex<Weak<LocalProcess>>,
        names: Mutex<HashMap<String, Strong>>,
    }

    impl Service for Registry {
        fn descriptor(&self) -> &str {
            "test.IRegistry"
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let process = self.process.lock().unwrap().upgrade().unwrap();
            let name = call.data.read_string16()?.unwrap();
            let mut reply = Parcel::new();
            match call.code {
                ADD => {
                    let Some(Binder::Handle(h)) = call.data.read_binder()? else {
                        return Err(crate::parcel::BAD_VALUE);
                    };
                    self.names.lock().unwrap().insert(name, process.strong(h));
                }
                GET => {
                    reply.write_binder(self.names.lock().unwrap().get(&name).map(|s| s.binder()))
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            Ok(reply)
        }
    }

    /// Echoes its argument with the caller's identity, and reports the
    /// death of a binder it is given.
    struct Echo {
        process: Mutex<Weak<LocalProcess>>,
        died: Mutex<mpsc::Sender<()>>,
        watched: Mutex<Vec<Strong>>,
    }

    impl Service for Echo {
        fn descriptor(&self) -> &str {
            "test.IEcho"
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let mut reply = Parcel::new();
            match call.code {
                ECHO => {
                    reply.write_i32(call.data.read_i32()?);
                    reply.write_i32(call.sender_pid);
                    reply.write_u32(call.sender_euid);
                }
                WATCH => {
                    let process = self.process.lock().unwrap().upgrade().unwrap();
                    let Some(Binder::Handle(h)) = call.data.read_binder()? else {
                        return Err(crate::parcel::BAD_VALUE);
                    };
                    let strong = process.strong(h);
                    let died = self.died.lock().unwrap().clone();
                    process.link_to_death(&strong, Box::new(move || died.send(()).unwrap()));
                    self.watched.lock().unwrap().push(strong);
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            Ok(reply)
        }
    }

    fn open(driver: &Arc<Driver>, pid: i32, uid: u32) -> Arc<LocalProcess> {
        LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid,
                euid: uid,
                security_context: None,
            },
        )
    }

    fn name(n: &str) -> Parcel {
        let mut p = Parcel::new();
        p.write_string16(Some(n));
        p
    }

    #[test]
    fn serves_calls_references_and_deaths() {
        let driver = Driver::new();
        let manager = open(&driver, 100, 1000);
        let registry = Arc::new(Registry {
            process: Mutex::new(Arc::downgrade(&manager)),
            names: Mutex::new(HashMap::new()),
        });
        let Binder::Local(ptr) = manager.add_service(registry) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        manager
            .ioctl(manager.tid(), BINDER_SET_CONTEXT_MGR_EXT, &mut object)
            .unwrap();
        manager.start();

        let server = open(&driver, 200, 1000);
        let (died, deaths) = mpsc::channel();
        let echo = server.add_service(Arc::new(Echo {
            process: Mutex::new(Arc::downgrade(&server)),
            died: Mutex::new(died),
            watched: Mutex::new(Vec::new()),
        }));
        server.start();
        let mut add = name("echo");
        add.write_binder(Some(echo));
        server.transact(0, ADD, &add, false).unwrap();

        let client = open(&driver, 300, 10123);
        let reply = client.transact(0, GET, &name("echo"), false).unwrap();
        let Some(Binder::Handle(h)) = reply.reader().read_binder().unwrap() else {
            panic!("no echo binder");
        };
        let echo = client.strong(h);
        drop(reply);
        let mut arg = Parcel::new();
        arg.write_i32(42);
        let reply = echo.transact(ECHO, &arg, false).unwrap();
        let mut r = reply.reader();
        assert_eq!(
            (r.read_i32(), r.read_i32(), r.read_u32()),
            (Ok(42), Ok(300), Ok(10123))
        );
        drop(reply);
        assert_eq!(
            client.transact(0, 99, &name("x"), false).err(),
            Some(UNKNOWN_TRANSACTION)
        );

        // A node of the client, watched by the server, dies with it.
        let listener = client.add_service(Arc::new(Registry {
            process: Mutex::new(Weak::new()),
            names: Mutex::new(HashMap::new()),
        }));
        let mut watch = Parcel::new();
        watch.write_binder(Some(listener));
        echo.transact(WATCH, &watch, false).unwrap();
        driver.release(client.handle);
        deaths.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
