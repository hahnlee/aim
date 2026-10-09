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
//! Its nodes accept file descriptors, as libbinder's do: they are closed
//! after the call, which may take a file it keeps ([`LocalProcess::file`]),
//! and a call that carries some to a service that takes none fails as the
//! driver fails it for a node that does not accept them. Replies may carry
//! fds too (`TF_ACCEPT_FDS`, as libbinder asks), closed with the reply.
//! The files of a parcel this process sends get fds for the send, closed
//! once the driver has taken them, as a libbinder parcel owns the fds it
//! writes.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

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

/// Intrinsic libbinder transactions, dispatched before service AIDL bodies.
const INTERFACE_TRANSACTION: u32 = u32::from_be_bytes(*b"_NTF");
const PING_TRANSACTION: u32 = u32::from_be_bytes(*b"_PNG");
const DEBUG_PID_TRANSACTION: u32 = u32::from_be_bytes(*b"_PID");

/// BBinder::transact writes its owning process PID directly, without an
/// exception header, interface token or caller permission check.
fn debug_pid_reply(pid: i32) -> Parcel {
    let mut reply = Parcel::new();
    reply.write_i32(pid);
    reply
}

/// What a service answers a call with: the reply, or a status that the
/// caller receives instead of one (`TF_STATUS_CODE`).
pub type Reply = Result<Parcel, StatusCode>;

/// A binder node served on the host.
pub trait Service: Send + Sync {
    /// Its interface descriptor (`INTERFACE_TRANSACTION`).
    fn descriptor(&self) -> &str;
    /// A plain Binder has no attached interface descriptor.
    fn has_descriptor(&self) -> bool {
        true
    }
    /// Handles one call; a one-way call's reply is dropped.
    fn transact(&self, call: &mut Call<'_>) -> Reply;
    /// Whether its calls may carry file descriptors (closed after the
    /// call).
    fn accepts_fds(&self) -> bool {
        false
    }
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
        joined: RefCell::new(Vec::new()),
    };
}

/// A host thread's binder identity; on exit it leaves every process it
/// used (`BINDER_THREAD_EXIT`, as `IPCThreadState`'s destructor does).
struct ThreadState {
    tid: i32,
    joined: RefCell<Vec<Joined>>,
}

/// A process the thread used, with the returns it read from it but has
/// not handled yet (`IPCThreadState::mIn`): a wait ends at its result and
/// leaves what follows for the thread's next wait.
struct Joined {
    process: Weak<LocalProcess>,
    input: VecDeque<(u32, Vec<u8>)>,
}

impl Drop for ThreadState {
    fn drop(&mut self) {
        for process in self
            .joined
            .borrow()
            .iter()
            .filter_map(|j| j.process.upgrade())
        {
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

impl FileTable {
    fn install(&mut self, file: File) -> u32 {
        let fd = self.next;
        self.next += 1;
        self.files.insert(fd, file);
        fd
    }
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
        Ok(self.files.lock().unwrap().install(file))
    }

    fn close_fd(&mut self, fd: u32) {
        self.files.lock().unwrap().files.remove(&fd);
    }
}

pub struct LocalProcess {
    driver: Arc<Driver>,
    handle: ProcHandle,
    credentials: Credentials,
    services: Mutex<HashMap<u64, Arc<dyn Service>>>,
    deaths: Mutex<HashMap<u64, Box<dyn FnOnce() + Send>>>,
    next_cookie: AtomicU64,
    files: Mutex<FileTable>,
    this: Weak<LocalProcess>,
    _memory: Arc<HeapReceiveMemory>,
    stopping: AtomicBool,
    loopers: Mutex<Loopers>,
    stopped: Condvar,
}

#[derive(Default)]
struct Loopers {
    workers: Vec<std::thread::JoinHandle<()>>,
    ids: Vec<std::thread::ThreadId>,
    tids: Vec<i32>,
    owner: Option<std::thread::ThreadId>,
    finished: bool,
    failed: bool,
}

/// A received reply; its buffer goes back to the driver when dropped.
pub struct Received {
    process: Arc<LocalProcess>,
    buffer: u64,
    data: std::borrow::Cow<'static, [u8]>,
    objects: Vec<u64>,
    owned_fds: Vec<u32>,
    _keepers: crate::parcel::Keepers,
}

impl Received {
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.data, &self.objects)
    }

    /// Copy a received payload for another outgoing transaction while retaining
    /// its Binder objects, file descriptors and receive buffer until consumption.
    pub fn into_parcel(self) -> Parcel {
        let mut parcel = Parcel::new();
        parcel.write_raw(&self.data, &self.objects);
        parcel.keep_alive(Arc::new(self));
        parcel
    }

    /// Retain a remote node in the process that received this reply.
    pub fn retain_remote_binder(&self, binder: Binder) -> Result<Strong, StatusCode> {
        match binder {
            Binder::Handle(handle) => Ok(self.process.strong(handle)),
            _ => Err(crate::parcel::BAD_VALUE),
        }
    }
}

impl Drop for Received {
    fn drop(&mut self) {
        self.process.close(std::mem::take(&mut self.owned_fds));
        if self.buffer != 0 {
            self.process
                .close(self.process.received_fds(&self.data, &self.objects));
            let mut out = Commands::default();
            out.u32(BC_FREE_BUFFER).u64(self.buffer);
            self.process.write(&out.0);
        }
    }
}

thread_local! {
    static CALLER: std::cell::Cell<Option<(i32, u32)>> = const { std::cell::Cell::new(None) };
    static AUTHENTICATED_INBOUND: std::cell::Cell<Option<(ProcHandle, u64)>> = const { std::cell::Cell::new(None) };
}

struct AuthenticatedInbound(Option<(ProcHandle, u64)>);
impl AuthenticatedInbound {
    fn enter(process: ProcHandle, buffer: u64) -> Self {
        Self(AUTHENTICATED_INBOUND.replace(Some((process, buffer))))
    }
}
impl Drop for AuthenticatedInbound {
    fn drop(&mut self) {
        AUTHENTICATED_INBOUND.set(self.0);
    }
}

struct CallingIdentity(Option<(i32, u32)>);
impl CallingIdentity {
    fn enter(pid: i32, uid: u32) -> Self {
        Self(CALLER.replace(Some((pid, uid))))
    }
}
impl Drop for CallingIdentity {
    fn drop(&mut self) {
        CALLER.set(self.0);
    }
}

/// A retained node of this process, dispatched like libbinder's BBinder.
pub struct LocalService {
    process: Arc<LocalProcess>,
    service: Arc<dyn Service>,
}
impl LocalService {
    pub fn transact(&self, code: u32, data: &Parcel, oneway: bool) -> Result<Received, StatusCode> {
        if self.process.stopping.load(Ordering::Acquire) {
            return Err(DEAD_OBJECT);
        }
        let request = self.process.owned_reply(data);
        if !request.owned_fds.is_empty() && !self.service.accepts_fds() {
            return Err(FAILED_TRANSACTION);
        }
        let (pid, uid) = CALLER
            .get()
            .unwrap_or((self.process.credentials.pid, self.process.credentials.euid));
        let _identity = CallingIdentity::enter(pid, uid);
        let mut call = Call {
            code,
            flags: TF_ACCEPT_FDS | if oneway { TF_ONE_WAY } else { 0 },
            sender_pid: pid,
            sender_euid: uid,
            data: request.reader(),
        };
        let reply = match code {
            INTERFACE_TRANSACTION => {
                let mut reply = Parcel::new();
                reply.write_string16(
                    self.service
                        .has_descriptor()
                        .then(|| self.service.descriptor()),
                );
                Ok(reply)
            }
            PING_TRANSACTION => Ok(Parcel::new()),
            DEBUG_PID_TRANSACTION => Ok(debug_pid_reply(self.process.credentials.pid)),
            _ => self.service.transact(&mut call),
        }?;
        if oneway {
            Ok(self.process.owned_reply(&Parcel::new()))
        } else {
            Ok(self.process.owned_reply(&reply))
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
        let handle = driver.open(device, credentials.clone());
        let memory = HeapReceiveMemory::new(VM_SIZE);
        let process = Arc::new_cyclic(|this| Self {
            driver: driver.clone(),
            handle,
            credentials,
            services: Mutex::new(HashMap::new()),
            deaths: Mutex::new(HashMap::new()),
            next_cookie: AtomicU64::new(1),
            files: Mutex::new(FileTable {
                files: HashMap::new(),
                next: 3,
            }),
            this: this.clone(),
            _memory: memory.clone(),
            stopping: AtomicBool::new(false),
            loopers: Mutex::new(Loopers::default()),
            stopped: Condvar::new(),
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
            let mut joined = t.joined.borrow_mut();
            if !joined.iter().any(|j| j.process.ptr_eq(&self.this)) {
                joined.push(Joined {
                    process: self.this.clone(),
                    input: VecDeque::new(),
                });
            }
            t.tid
        })
    }

    /// This thread's queue of unhandled returns from this process.
    fn input<R>(&self, f: impl FnOnce(&mut VecDeque<(u32, Vec<u8>)>) -> R) -> R {
        self.tid();
        THREAD.with(|t| {
            let mut joined = t.joined.borrow_mut();
            let joined = joined.iter_mut().find(|j| j.process.ptr_eq(&self.this));
            f(&mut joined.unwrap().input)
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
                Err(errno::EINTR) if !self.stopping.load(Ordering::Acquire) => continue,
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

    /// Its open binder file description, for the driver's own calls
    /// ([`Driver::shadow`]).
    pub fn proc_handle(&self) -> ProcHandle {
        self.handle
    }

    /// Serves `service` as a node; the binder to hand out for it.
    pub fn add_service(&self, service: Arc<dyn Service>) -> Binder {
        let ptr = self.next_cookie.fetch_add(1, Ordering::Relaxed) << 4;
        self.services.lock().unwrap().insert(ptr, service);
        Binder::Local(ptr)
    }

    /// Retain a local object returned by the driver to its owning process.
    pub fn local_service(&self, ptr: u64) -> Option<LocalService> {
        let service = self.services.lock().unwrap().get(&ptr).cloned()?;
        Some(LocalService {
            process: self.arc(),
            service,
        })
    }

    fn owned_reply(&self, parcel: &Parcel) -> Received {
        let (bytes, owned_fds) = self.install_files(parcel);
        Received {
            process: self.arc(),
            buffer: 0,
            data: bytes.into_owned().into(),
            objects: parcel.objects().to_vec(),
            owned_fds,
            _keepers: parcel.keepers(),
        }
    }

    /// Starts the thread pool: one looper now, more as the driver asks.
    pub fn start(&self) {
        self.spawn_looper(false);
    }

    fn spawn_looper(&self, registered: bool) {
        let mut loopers = self.loopers.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        let process = self.arc();
        if let Ok(worker) = std::thread::Builder::new()
            .name("binder-local".into())
            .spawn(move || {
                let mut out = Commands::default();
                out.u32(if registered {
                    BC_REGISTER_LOOPER
                } else {
                    BC_ENTER_LOOPER
                });
                // Register before publishing the tid: interrupt must find a
                // driver thread even if shutdown precedes its first read.
                process.write(&out.0);
                out.0.clear();
                {
                    let mut loopers = process.loopers.lock().unwrap();
                    if process.stopping.load(Ordering::Acquire) {
                        return;
                    }
                    loopers.tids.push(process.tid());
                }
                let _ = process.wait(&mut out.0, Until::Forever);
            })
        {
            loopers.ids.push(worker.thread().id());
            loopers.workers.push(worker);
        }
    }

    /// Stop this native Binder process and release its owned keepers (#1266).
    /// Borrowed transaction buffers remain mapped until their receivers drop.
    pub fn shutdown(&self) -> std::io::Result<()> {
        let current = std::thread::current().id();
        let mut loopers = self.loopers.lock().unwrap();
        if loopers.ids.contains(&current) || loopers.owner == Some(current) {
            return Err(std::io::Error::from_raw_os_error(libc::EDEADLK));
        }
        while self.stopping.load(Ordering::Acquire) && !loopers.finished {
            loopers = self.stopped.wait(loopers).unwrap();
        }
        if loopers.finished {
            return if loopers.failed {
                Err(std::io::Error::other("native Binder looper panicked"))
            } else {
                Ok(())
            };
        }
        self.stopping.store(true, Ordering::Release);
        loopers.owner = Some(current);
        let workers = std::mem::take(&mut loopers.workers);
        let tids = std::mem::take(&mut loopers.tids);
        drop(loopers);
        for tid in tids {
            self.driver.interrupt(self.handle, tid);
        }
        let mut failed = false;
        for worker in workers {
            failed |= worker.join().is_err();
        }
        // Release only after callbacks finish; release invalidates the
        // driver's receive allocator while callbacks may still read it.
        self.driver.release(self.handle);
        let services = std::mem::take(&mut *self.services.lock().unwrap());
        let files = std::mem::take(&mut self.files.lock().unwrap().files);
        let deaths = std::mem::take(&mut *self.deaths.lock().unwrap());
        drop((services, files, deaths));
        let mut loopers = self.loopers.lock().unwrap();
        loopers.failed = failed;
        loopers.finished = true;
        loopers.owner = None;
        loopers.ids.clear();
        self.stopped.notify_all();
        if failed {
            Err(std::io::Error::other("native Binder looper panicked"))
        } else {
            Ok(())
        }
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
        let removed = { self.deaths.lock().unwrap().remove(&cookie) };
        if removed.is_some() {
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
        self.transact_inner(handle, code, data, oneway, false)
    }

    /// Calls an original owner as the authenticated inbound Binder caller.
    /// The driver verifies the live incoming transaction; no UID/PID is accepted.
    /// Identity of this process's live driver-dispatched inbound call. Local
    /// synthetic calls and another process's dispatch do not establish provenance.
    pub fn authenticated_inbound_identity(&self) -> Option<(i32, u32)> {
        AUTHENTICATED_INBOUND
            .get()
            .filter(|(process, _)| *process == self.handle)
            .and_then(|_| CALLER.get())
    }

    pub fn transact_preserving_inbound(
        &self,
        handle: u32,
        code: u32,
        data: &Parcel,
    ) -> Result<Received, StatusCode> {
        self.transact_inner(handle, code, data, false, true)
    }

    fn transact_inner(
        &self,
        handle: u32,
        code: u32,
        data: &Parcel,
        oneway: bool,
        forward: bool,
    ) -> Result<Received, StatusCode> {
        let (bytes, fds) = self.install_files(data);
        let tr = TransactionData {
            target: u64::from(handle),
            cookie: 0,
            code,
            flags: TF_ACCEPT_FDS | if oneway { TF_ONE_WAY } else { 0 },
            sender_pid: 0,
            sender_euid: 0,
            data_size: bytes.len() as u64,
            offsets_size: 8 * data.objects().len() as u64,
            buffer: bytes.as_ptr() as u64,
            offsets: data.objects().as_ptr() as u64,
        };
        let mut out = Commands::default();
        if forward {
            let inbound = AUTHENTICATED_INBOUND
                .get()
                .filter(|(process, _)| *process == self.handle)
                .ok_or(FAILED_TRANSACTION);
            let result = inbound.and_then(|(_, buffer)| {
                let mut local = Local { files: &self.files };
                self.driver
                    .forward_inbound_transaction(self.handle, self.tid(), buffer, &tr, &mut local)
                    .map_err(|_| FAILED_TRANSACTION)
            });
            if let Err(error) = result {
                self.close(fds);
                return Err(error);
            }
        } else {
            out.u32(BC_TRANSACTION).bytes(&tr.encode());
        }
        let reply = self.wait(
            &mut out.0,
            if oneway {
                Until::Complete
            } else {
                Until::Reply
            },
        );
        self.close(fds);
        let reply = reply?;
        let Some(tr) = reply else {
            return Ok(Received {
                process: self.arc(),
                buffer: 0,
                data: (&[][..]).into(),
                objects: Vec::new(),
                owned_fds: Vec::new(),
                _keepers: Default::default(),
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
            data: data.into(),
            objects,
            owned_fds: Vec::new(),
            _keepers: Default::default(),
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

    /// `waitForResponse` and `executeCommand`: handles the thread's
    /// returns one at a time, talking to the driver (and sending `out`)
    /// when it has none, until `until`. A failed call ends any wait but a
    /// looper's.
    fn wait(&self, out: &mut Vec<u8>, until: Until) -> Result<Option<TransactionData>, StatusCode> {
        loop {
            if self.stopping.load(Ordering::Acquire) {
                return Err(DEAD_OBJECT);
            }
            let Some((cmd, payload)) = self.input(|input| input.pop_front()) else {
                self.read(out).map_err(|e| -e)?;
                continue;
            };
            match (cmd, until) {
                (_, Until::Forever) => self.execute_command(cmd, &payload, out),
                (BR_TRANSACTION_COMPLETE, Until::Complete) => return Ok(None),
                // A synchronous call's own completion.
                (BR_TRANSACTION_COMPLETE, Until::Reply) => {}
                (BR_REPLY, Until::Reply) => return Ok(Some(TransactionData::decode(&payload))),
                (BR_DEAD_REPLY, _) => return Err(DEAD_OBJECT),
                (BR_FAILED_REPLY | BR_FROZEN_REPLY, _) => return Err(FAILED_TRANSACTION),
                _ => self.execute_command(cmd, &payload, out),
            }
        }
    }

    /// Writes `out` and queues what the driver returns.
    fn read(&self, out: &mut Vec<u8>) -> Result<(), Errno> {
        let mut read = [0u8; READ_SIZE];
        let (consumed, n) = self.talk(out, &mut read)?;
        out.drain(..consumed);
        self.input(|input| {
            let mut at = 0;
            while at + 4 <= n {
                let cmd = u32::from_le_bytes(read[at..at + 4].try_into().unwrap());
                input.push_back((cmd, read[at + 4..at + 4 + ioc_size(cmd)].to_vec()));
                at += 4 + ioc_size(cmd);
            }
        });
        Ok(())
    }

    fn execute_command(&self, cmd: u32, payload: &[u8], out: &mut Vec<u8>) {
        match cmd {
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
                let on_death = { self.deaths.lock().unwrap().remove(&cookie) };
                if let Some(on_death) = on_death { on_death(); }
                out.extend_from_slice(&BC_DEAD_BINDER_DONE.to_le_bytes());
                out.extend_from_slice(&cookie.to_le_bytes());
            }
            BR_SPAWN_LOOPER => self.spawn_looper(true),
            _ => {} // NOOP, OK, RELEASE, DECREFS, death cleared, a looper's results
        }
    }

    /// The file behind `fd` of a call or reply being read, to keep past
    /// its close.
    pub fn file(&self, fd: u32) -> Option<File> {
        self.files.lock().unwrap().files.get(&fd).cloned()
    }

    /// `parcel`'s bytes with an fd of this process in each fd object, and
    /// those fds, to close once the driver has taken the files.
    fn install_files<'p>(&self, parcel: &'p Parcel) -> (std::borrow::Cow<'p, [u8]>, Vec<u32>) {
        if parcel.files().is_empty() {
            return (parcel.data().into(), Vec::new());
        }
        let mut bytes = parcel.data().to_vec();
        let mut fds = Vec::new();
        let mut table = self.files.lock().unwrap();
        for (at, file) in parcel.files() {
            let fd = table.install(file.clone());
            // flat_binder_object.handle, which holds the fd.
            let at = *at as usize + 8;
            bytes[at..at + 4].copy_from_slice(&fd.to_le_bytes());
            fds.push(fd);
        }
        (bytes.into(), fds)
    }

    fn close(&self, fds: Vec<u32>) {
        let mut files = self.files.lock().unwrap();
        for fd in fds {
            files.files.remove(&fd);
        }
    }

    /// The file descriptors the driver installed for a received call:
    /// its fd objects and the fds of its fd arrays.
    fn received_fds(&self, data: &[u8], objects: &[u64]) -> Vec<u32> {
        let u32_at = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let mut fds = Vec::new();
        for &at in objects {
            let at = at as usize;
            match u32_at(at) {
                BINDER_TYPE_FD => fds.push(u32_at(at + 8)),
                BINDER_TYPE_FDA => {
                    let array = FdArrayObject::decode(&data[at..at + FD_ARRAY_OBJECT_SIZE]);
                    let parent = objects[array.parent as usize] as usize;
                    let parent = BufferObject::decode(&data[parent..parent + BUFFER_OBJECT_SIZE]);
                    let base = (parent.buffer + array.parent_offset) as *const u32;
                    for i in 0..array.num_fds as usize {
                        // SAFETY: the driver placed the parent buffer, with
                        // the array's fds, in this call's receive buffer.
                        fds.push(unsafe { base.add(i).read_unaligned() });
                    }
                }
                _ => {}
            }
        }
        fds
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
        let fds = self.received_fds(data, &objects);
        let _identity = CallingIdentity::enter(tr.sender_pid, tr.sender_euid);
        let _inbound = AuthenticatedInbound::enter(self.handle, tr.buffer);
        let reply = match (&service, tr.code) {
            (None, _) => Err(UNKNOWN_TRANSACTION),
            (Some(s), _) if !fds.is_empty() && !s.accepts_fds() => Err(FAILED_TRANSACTION),
            (Some(s), INTERFACE_TRANSACTION) => {
                let mut p = Parcel::new();
                p.write_string16(s.has_descriptor().then(|| s.descriptor()));
                Ok(p)
            }
            (Some(_), PING_TRANSACTION) => Ok(Parcel::new()),
            (Some(_), DEBUG_PID_TRANSACTION) => Ok(debug_pid_reply(self.credentials.pid)),
            // Ops noted for a caller that asks for them go back with the
            // reply (`Binder.execTransactInternal`).
            (Some(s), _) => {
                let collect = tr.flags & crate::appops::FLAG_COLLECT_NOTED_APP_OPS != 0;
                let uid = collect.then_some(tr.sender_euid);
                crate::appops::collecting(uid, || s.transact(&mut call))
            }
        };
        self.close(fds);
        out.extend_from_slice(&BC_FREE_BUFFER.to_le_bytes());
        out.extend_from_slice(&tr.buffer.to_le_bytes());
        if call.is_oneway() {
            return;
        }
        let ((bytes, fds), offsets, flags) = match &reply {
            Ok(p) => (self.install_files(p), p.objects(), 0),
            Err(s) => (
                (s.to_le_bytes().to_vec().into(), Vec::new()),
                &[][..],
                TF_STATUS_CODE,
            ),
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
        self.close(fds);
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

    /// Counts its calls; takes file descriptors.
    struct Sink(AtomicI32);

    impl Service for Sink {
        fn descriptor(&self) -> &str {
            "test.ISink"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Parcel::new())
        }
        fn accepts_fds(&self) -> bool {
            true
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

    struct FileKeeper(Arc<std::fs::File>);

    impl Service for FileKeeper {
        fn descriptor(&self) -> &str { "test.IFileKeeper" }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            let _ = self.0.metadata().unwrap();
            Ok(Parcel::new())
        }
    }

    #[test]
    fn shutdown_releases_service_files_and_does_not_close_reused_fd() {
        // Isolate the numeric FD reuse from other tests opening files.
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "local::tests::shutdown_file_fixture", "--ignored", "--nocapture"])
            .output().unwrap();
        assert!(result.status.success(), "{}{}", String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr));
        assert!(String::from_utf8_lossy(&result.stdout)
            .contains("local::tests::shutdown_file_fixture ... ok"));
    }

    #[test]
    #[ignore = "run by the isolated FD ownership test"]
    fn shutdown_file_fixture() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let path = std::env::temp_dir().join(format!("aim-native-binder-file-{}", std::process::id()));
        std::fs::write(&path, b"native service data").unwrap();
        let file = Arc::new(std::fs::File::open(&path).unwrap());
        let fd = file.as_raw_fd();
        let weak_file = Arc::downgrade(&file);
        let driver = Driver::new();
        let process = open(&driver, 410, 1000);
        process.add_service(Arc::new(FileKeeper(file.clone())));
        process.files.lock().unwrap().install(file);
        process.start();
        let weak_process = Arc::downgrade(&process);
        drop(process);
        assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
        let process = weak_process.upgrade().expect("the looper retains its process");
        process.shutdown().unwrap();
        assert!(weak_file.upgrade().is_none());
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
        let replacement = std::fs::File::open(&path).unwrap();
        let reused = if replacement.as_raw_fd() == fd { replacement } else {
            let reused = unsafe { libc::fcntl(replacement.as_raw_fd(), libc::F_DUPFD_CLOEXEC, fd) };
            assert_eq!(reused, fd);
            std::fs::File::from(unsafe { OwnedFd::from_raw_fd(reused) })
        };
        process.shutdown().unwrap();
        drop(process);
        assert!(unsafe { libc::fcntl(reused.as_raw_fd(), libc::F_GETFD) } >= 0);
        drop(reused);
        std::fs::remove_file(path).unwrap();
    }

    struct BlockingFileCall {
        file: Arc<std::fs::File>,
        entered: mpsc::Sender<()>,
        resume: Mutex<mpsc::Receiver<()>>,
    }
    impl Service for BlockingFileCall {
        fn descriptor(&self) -> &str { "test.IBlockingFileCall" }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            self.entered.send(()).unwrap();
            self.resume.lock().unwrap().recv().unwrap();
            assert_eq!(call.data.read_i32(), Ok(1266));
            assert!(self.file.metadata().is_ok());
            Ok(Parcel::new())
        }
    }

    #[test]
    fn shutdown_waits_for_inflight_calls_before_releasing_buffers_and_files() {
        let path = std::env::temp_dir().join(format!("aim-native-binder-call-{}", std::process::id()));
        std::fs::write(&path, b"inflight data").unwrap();
        let file = Arc::new(std::fs::File::open(&path).unwrap());
        let weak_file = Arc::downgrade(&file);
        let driver = Driver::new();
        let manager = start_registry(&driver);
        let server = open(&driver, 411, 1000);
        let client = open(&driver, 412, 2000);
        let (entered, waiting) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let service = server.add_service(Arc::new(BlockingFileCall {
            file, entered, resume: Mutex::new(resumed),
        }));
        server.start();
        let handle = publish(&server, &client, "inflight", service);
        let caller = std::thread::spawn(move || {
            let mut request = Parcel::new(); request.write_i32(1266);
            let _ = handle.transact(1, &request, false);
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let owner = server.clone();
        let shutdown = std::thread::spawn(move || owner.shutdown());
        while !server.stopping.load(Ordering::Acquire) { std::thread::yield_now(); }
        let owner = server.clone();
        let concurrent = std::thread::spawn(move || owner.shutdown());
        assert!(!shutdown.is_finished());
        assert!(!concurrent.is_finished());
        assert!(weak_file.upgrade().is_some());
        resume.send(()).unwrap();
        shutdown.join().unwrap().unwrap();
        concurrent.join().unwrap().unwrap();
        caller.join().unwrap();
        assert!(weak_file.upgrade().is_none());
        client.shutdown().unwrap(); manager.shutdown().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    struct SelfShutdown(Weak<LocalProcess>);
    impl Service for SelfShutdown {
        fn descriptor(&self) -> &str { "test.ISelfShutdown" }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            let error = self.0.upgrade().unwrap().shutdown().unwrap_err();
            let mut reply = Parcel::new(); reply.write_i32(error.raw_os_error().unwrap());
            Ok(reply)
        }
    }

    #[test]
    fn shutdown_from_own_worker_returns_deadlock_error() {
        let driver = Driver::new();
        let manager = start_registry(&driver);
        let server = open(&driver, 413, 1000);
        let client = open(&driver, 414, 2000);
        let service = server.add_service(Arc::new(SelfShutdown(Arc::downgrade(&server))));
        let local = server.local_service(match service { Binder::Local(ptr) => ptr, _ => unreachable!() }).unwrap();
        server.start();
        let handle = publish(&server, &client, "self-stop", service);
        let reply = handle.transact(1, &Parcel::new(), false).unwrap();
        assert_eq!(reply.reader().read_i32(), Ok(libc::EDEADLK));
        server.shutdown().unwrap(); client.shutdown().unwrap(); manager.shutdown().unwrap();
        assert_eq!(local.transact(PING_TRANSACTION, &Parcel::new(), false).err(), Some(DEAD_OBJECT));
        // A received reply still owns its mapped bytes after process retirement.
        assert_eq!(reply.reader().read_i32(), Ok(libc::EDEADLK));
    }

    fn name(n: &str) -> Parcel {
        let mut p = Parcel::new();
        p.write_string16(Some(n));
        p
    }

    /// Starts a context manager serving a [`Registry`].
    fn start_registry(driver: &Arc<Driver>) -> Arc<LocalProcess> {
        let manager = open(driver, 100, 1000);
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
        manager
    }

    /// Registers `binder` of `server` as `n` and looks it up in `client`.
    fn publish(server: &LocalProcess, client: &LocalProcess, n: &str, binder: Binder) -> Strong {
        let mut add = name(n);
        add.write_binder(Some(binder));
        server.transact(0, ADD, &add, false).unwrap();
        let reply = client.transact(0, GET, &name(n), false).unwrap();
        let Some(Binder::Handle(h)) = reply.reader().read_binder().unwrap() else {
            panic!("no {n} binder");
        };
        client.strong(h)
    }

    #[test]
    fn native_namespace_init_identity_is_reported_on_real_driver_calls() {
        struct CallerIdentity;
        impl Service for CallerIdentity {
            fn descriptor(&self)->&str{"test.INamespaceCallerIdentity"}
            fn transact(&self,call:&mut Call<'_>)->Reply{
                let mut reply=Parcel::new();reply.write_i32(call.sender_pid);reply.write_u32(call.sender_euid);Ok(reply)
            }
        }
        let driver=Driver::new();let _manager=start_registry(&driver);
        let actual_host=unsafe{libc::getpid()};assert_ne!(actual_host,1);
        let init=open(&driver,1,1000);init.start();
        let guest=open(&driver,97001,10123);guest.start();
        let guest_node=guest.add_service(Arc::new(CallerIdentity));
        let target=publish(&guest,&init,"namespace-init-call-target",guest_node);
        let reply=target.transact(1,&Parcel::new(),false).unwrap();
        let mut reader=reply.reader();assert_eq!(reader.read_i32(),Ok(1));assert_eq!(reader.read_u32(),Ok(1000));assert_eq!(reader.remaining(),0);
        let init_node=init.add_service(Arc::new(CallerIdentity));
        let remote=publish(&init,&guest,"namespace-init-debug-owner",init_node);
        let reply=remote.transact(DEBUG_PID_TRANSACTION,&Parcel::new(),false).unwrap();
        assert_eq!(reply.reader().read_i32(),Ok(1));
        assert_eq!(unsafe{libc::getpid()},actual_host,"guest credential must not alter actual host process identity");
        let port=crate::mach::new_port(true).unwrap();
        let receiver=std::thread::spawn(move||{
            let mut buffer=crate::mach::Buffer::default();let request=crate::mach::receive(&mut buffer,port).unwrap();
            assert_eq!(request.pid,actual_host,"Mach audit identity must remain the actual host process");
            assert_eq!(request.data,1i32.to_le_bytes());
            crate::mach::reply(&mut buffer,request.reply,&crate::mach::Msg{id:78,ports:Vec::new(),data:request.pid.to_le_bytes().to_vec()});
            crate::mach::drop_own_send(port);crate::mach::destroy_receive(port);
        });
        let reply_port=crate::mach::new_port(false).unwrap();let mut buffer=crate::mach::Buffer::default();
        let reply=crate::mach::call(&mut buffer,port,reply_port,&crate::mach::Msg{id:77,ports:Vec::new(),data:1i32.to_le_bytes().to_vec()}).unwrap();
        assert_eq!(reply.data,actual_host.to_le_bytes());crate::mach::destroy_receive(reply_port);receiver.join().unwrap();
    }

    #[test]
    fn debug_pid_intrinsic_returns_actual_node_owner_on_local_and_remote_calls() {
        let driver = Driver::new();
        let _manager = start_registry(&driver);
        let owner = open(&driver, 231, 1000);
        let sink = Arc::new(Sink(AtomicI32::new(0)));
        let binder = owner.add_service(sink.clone());
        let Binder::Local(ptr) = binder else { unreachable!() };
        let local = owner.local_service(ptr).unwrap();
        {
            let _caller = CallingIdentity::enter(982, 10123);
            let response = local.transact(DEBUG_PID_TRANSACTION, &Parcel::new(), false).unwrap();
            let mut reader = response.reader();
            assert_eq!(reader.read_i32(), Ok(231));
            assert_eq!(reader.remaining(), 0);
        }
        owner.start();
        let caller = open(&driver, 982, 10123);
        let remote = publish(&owner, &caller, "debug-pid-owner", binder);
        let response = remote.transact(DEBUG_PID_TRANSACTION, &Parcel::new(), false).unwrap();
        let mut reader = response.reader();
        assert_eq!(reader.read_i32(), Ok(231));
        assert_eq!(reader.remaining(), 0);
        assert_eq!(sink.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn local_objects_preserve_identity_and_file_lifetimes() {
        struct Echo(Weak<LocalProcess>);
        impl Service for Echo {
            fn descriptor(&self) -> &str {
                "local.echo"
            }
            fn accepts_fds(&self) -> bool {
                true
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                call.data.enforce_interface(self.descriptor())?;
                let mut reply = Parcel::new();
                reply.write_i32(call.sender_pid);
                reply.write_i32(call.sender_euid as i32);
                if call.code == 1 {
                    let fd = call.data.read_fd()?;
                    reply.write_file(self.0.upgrade().unwrap().file(fd).unwrap());
                    reply.write_i32(fd as i32);
                } else if call.code != 0 {
                    return Err(UNKNOWN_TRANSACTION);
                }
                Ok(reply)
            }
        }
        let driver = Driver::new();
        let process = open(&driver, 991, 1000);
        let Binder::Local(ptr) = process.add_service(Arc::new(Echo(Arc::downgrade(&process))))
        else {
            unreachable!()
        };
        let service = process.local_service(ptr).unwrap();
        assert!(process.local_service(ptr + 1).is_none());
        let mut request = Parcel::new();
        request.write_interface_token("local.echo");
        let reply = service.transact(0, &request, false).unwrap();
        let mut reader = reply.reader();
        assert_eq!(reader.read_i32().unwrap(), 991);
        assert_eq!(reader.read_i32().unwrap(), 1000);
        {
            let _identity = CallingIdentity::enter(992, 19001);
            let reply = service.transact(0, &request, false).unwrap();
            let mut reader = reply.reader();
            assert_eq!(reader.read_i32().unwrap(), 992);
            assert_eq!(reader.read_i32().unwrap(), 19001);
        }
        assert!(CALLER.get().is_none());
        assert_eq!(
            service.transact(999, &request, false).err(),
            Some(UNKNOWN_TRANSACTION)
        );
        let wrong = Parcel::new();
        assert!(service.transact(0, &wrong, false).is_err());
        assert_eq!(
            service
                .transact(0, &request, true)
                .unwrap()
                .reader()
                .remaining(),
            0
        );
        let file: File = Arc::new(String::from("retained file"));
        request.write_file(file.clone());
        let reply = service.transact(1, &request, false).unwrap();
        let mut reader = reply.reader();
        reader.read_i32().unwrap();
        reader.read_i32().unwrap();
        let reply_fd = reader.read_fd().unwrap();
        let call_fd = reader.read_i32().unwrap() as u32;
        assert!(process.file(call_fd).is_none());
        assert!(Arc::ptr_eq(&process.file(reply_fd).unwrap(), &file));
        let retained = process.file(reply_fd).unwrap();
        drop(reply);
        assert!(process.file(reply_fd).is_none());
        assert!(Arc::ptr_eq(&retained, &file));
    }

    /// Hands out a node of its own (as a `ParceledListSlice` hands out its
    /// retriever) and echoes on it.
    struct Lister {
        process: Mutex<Weak<LocalProcess>>,
        echo: Arc<Echo>,
    }

    impl Service for Lister {
        fn descriptor(&self) -> &str {
            "test.ILister"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            let process = self.process.lock().unwrap().upgrade().unwrap();
            let mut reply = Parcel::new();
            reply.write_i32(7);
            reply.write_binder(Some(process.add_service(self.echo.clone())));
            Ok(reply)
        }
    }

    #[test]
    fn shadow_copies_calls_replies_and_followed_nodes() {
        use aim_binder_driver::{ShadowKind, ShadowReply, ShadowSink};
        let driver = Driver::new();
        let _manager = start_registry(&driver);
        let server = open(&driver, 200, 1000);
        let lister = server.add_service(Arc::new(Lister {
            process: Mutex::new(Arc::downgrade(&server)),
            echo: Arc::new(Echo {
                process: Mutex::new(Arc::downgrade(&server)),
                died: Mutex::new(mpsc::channel().0),
                watched: Mutex::new(Vec::new()),
            }),
        }));
        server.start();
        let client = open(&driver, 300, 10123);
        let lister = publish(&server, &client, "lister", lister);

        // Whoever holds a reference can have the node shadowed.
        let watcher = open(&driver, 400, 1000);
        let found = watcher.transact(0, GET, &name("lister"), false).unwrap();
        let Ok(Some(Binder::Handle(watched))) = found.reader().read_binder() else {
            panic!("no lister")
        };
        let watched = watcher.strong(watched);
        drop(found);
        let (copies, received) = mpsc::sync_channel(4);
        let sink = ShadowSink {
            copies,
            identify: |_| None,
            dropped: Arc::default(),
        };
        assert!(
            driver
                .shadow(watcher.proc_handle(), 999, sink.clone())
                .is_err()
        );
        driver
            .shadow(watcher.proc_handle(), watched.handle, sink)
            .unwrap();

        let mut arg = Parcel::new();
        arg.write_i32(5);
        let reply = lister.transact(1, &arg, false).unwrap();
        let mut r = reply.reader();
        assert_eq!(r.read_i32(), Ok(7));
        let Ok(Some(Binder::Handle(h))) = r.read_binder() else {
            panic!("no binder")
        };
        let copy = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (copy.from_pid, copy.from_euid, copy.to_pid),
            (300, 10123, 200)
        );
        assert_eq!((copy.code, copy.follows), (1, None));
        assert_eq!(copy.data.data, arg.data());
        let ShadowReply::Reply { parcel, .. } = &copy.reply else {
            panic!("no reply copy: {:?}", copy.reply)
        };
        let [object] = &parcel.objects[..] else {
            panic!("{:?}", parcel.objects)
        };
        assert_eq!(object.offset, 4);
        assert!(matches!(
            object.kind,
            ShadowKind::Binder { owner_pid: 200, .. }
        ));

        // The node the reply handed out is followed.
        let echo = client.strong(h);
        drop(reply);
        let mut arg = Parcel::new();
        arg.write_i32(9);
        echo.transact(ECHO, &arg, false).unwrap();
        let fetch = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((fetch.code, fetch.follows), (ECHO, Some(copy.seq)));
        assert!(matches!(fetch.reply, ShadowReply::Reply { .. }));
    }

    #[test]
    fn serves_calls_references_and_deaths() {
        let driver = Driver::new();
        let _manager = start_registry(&driver);

        let server = open(&driver, 200, 1000);
        let (died, deaths) = mpsc::channel();
        let echo = server.add_service(Arc::new(Echo {
            process: Mutex::new(Arc::downgrade(&server)),
            died: Mutex::new(died),
            watched: Mutex::new(Vec::new()),
        }));
        server.start();
        let client = open(&driver, 300, 10123);
        let echo = publish(&server, &client, "echo", echo);
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

        // A call with a file descriptor: refused by a service that takes
        // none, closed after the call by one that does.
        let sink = Arc::new(Sink(AtomicI32::new(0)));
        let sink_binder = server.add_service(sink.clone());
        let sink_handle = publish(&server, &client, "sink", sink_binder);
        let file: File = Arc::new(());
        let fd = Local {
            files: &client.files,
        }
        .install_file(file)
        .unwrap();
        let mut object = [0u8; FD_OBJECT_SIZE];
        object[..4].copy_from_slice(&BINDER_TYPE_FD.to_le_bytes());
        object[8..12].copy_from_slice(&fd.to_le_bytes());
        let mut with_fd = Parcel::new();
        with_fd.write_raw(&object, &[0]);
        assert_eq!(
            echo.transact(ECHO, &with_fd, false).err(),
            Some(FAILED_TRANSACTION)
        );
        sink_handle.transact(1, &with_fd, false).unwrap();
        assert_eq!(sink.0.load(Ordering::Relaxed), 1);
        assert!(server.files.lock().unwrap().files.is_empty());
        // A parcel's own file gets an fd for the send only.
        client.close(vec![fd]);
        let mut with_file = Parcel::new();
        with_file.write_file(Arc::new(()));
        sink_handle.transact(1, &with_file, false).unwrap();
        assert_eq!(sink.0.load(Ordering::Relaxed), 2);
        assert!(client.files.lock().unwrap().files.is_empty());

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

    /// Releases its own process while it serves a call, as a process that
    /// dies mid-call.
    struct Dies {
        driver: Arc<Driver>,
        process: Mutex<Weak<LocalProcess>>,
    }

    impl Service for Dies {
        fn descriptor(&self) -> &str {
            "test.IDies"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            let process = self.process.lock().unwrap().upgrade().unwrap();
            self.driver.release(process.handle);
            Ok(Parcel::new())
        }
    }

    /// A call whose target dies after its completion fails with
    /// DEAD_OBJECT, and the thread's next one-way call still goes out
    /// (`waitForResponse` drops a synchronous call's own completion).
    #[test]
    fn a_failed_call_leaves_the_next_oneway_call_intact() {
        let driver = Driver::new();
        let _manager = start_registry(&driver);
        let client = open(&driver, 300, 10123);

        let doomed = open(&driver, 200, 1000);
        let dies = doomed.add_service(Arc::new(Dies {
            driver: driver.clone(),
            process: Mutex::new(Arc::downgrade(&doomed)),
        }));
        doomed.start();
        let dies = publish(&doomed, &client, "dies", dies);

        let server = open(&driver, 400, 1000);
        let sink = Arc::new(Sink(AtomicI32::new(0)));
        let sink_binder = server.add_service(sink.clone());
        server.start();
        let sink_handle = publish(&server, &client, "sink", sink_binder);

        assert_eq!(
            dies.transact(1, &Parcel::new(), false).err(),
            Some(DEAD_OBJECT)
        );
        sink_handle.transact(1, &Parcel::new(), true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while sink.0.load(Ordering::Relaxed) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the one-way call was lost"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Notes op 1 for its caller when it collects for it.
    struct Noting;

    impl Service for Noting {
        fn descriptor(&self) -> &str {
            "test.INoting"
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            if crate::appops::collecting_uid() == Some(call.sender_euid) {
                crate::appops::collect_sync(1, None);
            }
            let mut reply = Parcel::new();
            reply.write_no_exception();
            Ok(reply)
        }
    }

    #[test]
    fn noted_ops_go_back_to_a_caller_that_collects() {
        let driver = Driver::new();
        let server = open(&driver, 100, 1000);
        let Binder::Local(ptr) = server.add_service(Arc::new(Noting)) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        server
            .ioctl(server.tid(), BINDER_SET_CONTEXT_MGR_EXT, &mut object)
            .unwrap();
        server.start();
        let client = open(&driver, 300, 10123);
        // As BinderProxy.transact sends a call when listening for its ops.
        let call = |flags: u32| {
            let tr = TransactionData {
                target: 0,
                cookie: 0,
                code: 1,
                flags,
                sender_pid: 0,
                sender_euid: 0,
                data_size: 0,
                offsets_size: 0,
                buffer: 0,
                offsets: 0,
            };
            let mut out = Commands::default();
            out.u32(BC_TRANSACTION).bytes(&tr.encode());
            let tr = client.wait(&mut out.0, Until::Reply).unwrap().unwrap();
            let received = Received {
                process: client.arc(),
                buffer: tr.buffer,
                // SAFETY: the reply, in the receive buffer until dropped.
                data: unsafe {
                    std::slice::from_raw_parts(tr.buffer as *const u8, tr.data_size as usize).into()
                },
                objects: Vec::new(),
                owned_fds: Vec::new(),
                _keepers: Default::default(),
            };
            received.data.to_vec()
        };
        let collected = call(TF_ACCEPT_FDS | crate::appops::FLAG_COLLECT_NOTED_APP_OPS);
        let mut r = Reader::new(&collected, &[]);
        assert_eq!(r.read_i32(), Ok(-127));
        assert_eq!(r.read_i32(), Ok(36));
        assert_eq!(r.read_i32(), Ok(1));
        assert_eq!(r.read_string16(), Ok(None));
        assert_eq!(r.read_i64(), Ok(1 << 1));
        let mut r = Reader::new(&collected, &[]);
        assert_eq!(r.read_exception(), Ok(Ok(())));
        assert_eq!(call(TF_ACCEPT_FDS), 0i32.to_le_bytes());
    }
}
