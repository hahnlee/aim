#![forbid(unsafe_op_in_unsafe_fn)]
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Write;
use std::ptr;
use std::slice;
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

const EOF: i32 = -1;
const EINTR: i32 = 4;
const EBADF: i32 = 9;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const EOVERFLOW: i32 = 75;
const EIO: i32 = 5;
const EFBIG: i32 = 27;
const EOPNOTSUPP: i32 = 95;
// Only fixture-table streams use this mirror cap. Production VFS streams are
// descriptor-backed and do not materialize package data in this Vec.
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;
const O_WRONLY: c_int = 1;
const O_RDWR: c_int = 2;
const O_CREAT: c_int = 64;
const O_TRUNC: c_int = 512;
const O_APPEND: c_int = 1024;

#[repr(C, align(8))]
pub struct AndroidFile {
    opaque: [u8; 152],
}
type ScanCallback = unsafe extern "C" fn(
    input: *const c_char,
    length: usize,
    context: *mut c_void,
    consumed: *mut usize,
) -> c_int;
unsafe extern "C" {
    static mut aim_bionic___sF: [AndroidFile; 3];
    fn aim_bionic_errno_load() -> i32;
    fn aim_bionic_errno_store(value: i32);
    fn aim_bionic_open(path: *const c_char, flags: c_int, mode: u32) -> c_int;
    fn aim_bionic_read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    fn aim_bionic_write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn aim_bionic_lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
    fn aim_bionic_close(fd: c_int) -> c_int;
    fn aim_bionic_wide_stdio_install(backend: *const WideStdioBackend) -> *mut WideStdioActivation;
    fn aim_bionic_wide_stdio_uninstall(activation: *mut *mut WideStdioActivation) -> c_int;
    fn aim_bionic_wide_stdio_reset(file: *mut AndroidFile) -> c_int;
    fn aim_bionic_wide_stdio_forget(file: *mut AndroidFile) -> c_int;
}

#[repr(C)]
struct WideStdioActivation {
    _private: [u8; 0],
}

#[repr(C)]
struct WideStdioBackend {
    abi_version: u32,
    struct_size: u32,
    context: *mut c_void,
    acquire: extern "C" fn(*mut c_void, *mut AndroidFile, *mut *mut c_void) -> c_int,
    release: extern "C" fn(*mut c_void, *mut c_void),
    orient_wide: extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    read_byte: extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> c_int,
    write_bytes: extern "C" fn(*mut c_void, *mut c_void, *const u8, usize) -> c_int,
    set_error: extern "C" fn(*mut c_void, *mut c_void),
    clear_error_and_eof: extern "C" fn(*mut c_void, *mut c_void),
}

struct Stream {
    _token: Option<Box<AndroidFile>>,
    data: Vec<u8>,
    position: usize,
    readable: bool,
    writable: bool,
    pushback: Option<u8>,
    fd: i32,
    owns_fd: bool,
    // A stream backed by the fixture table has no guest descriptor. Keep its
    // path so fclose can commit writes back to that table; real guest files
    // are persisted by the descriptor itself.
    backing_path: Option<Vec<u8>>,
    append: bool,
    orientation: i8,
    error: bool,
    eof: bool,
    wide_leases: usize,
    exclusive: bool,
}
struct Table {
    streams: BTreeMap<usize, Stream>,
    files: BTreeMap<Vec<u8>, Vec<u8>>,
    next_fd: i32,
    shutting_down: bool,
    allow_memory_files: bool,
}
pub struct Provider {
    table: Mutex<Table>,
    idle: Condvar,
}
static ACTIVE: OnceLock<RwLock<Option<Arc<Provider>>>> = OnceLock::new();
fn slot() -> &'static RwLock<Option<Arc<Provider>>> {
    ACTIVE.get_or_init(|| RwLock::new(None))
}

impl Provider {
    pub fn new(files: Vec<(Vec<u8>, Vec<u8>)>, stdin: Vec<u8>) -> Result<Self, &'static str> {
        Self::new_with_mode(files, stdin, true)
    }

    fn new_with_mode(
        files: Vec<(Vec<u8>, Vec<u8>)>,
        stdin: Vec<u8>,
        allow_memory_files: bool,
    ) -> Result<Self, &'static str> {
        if stdin.len() > MAX_STREAM_BYTES {
            return Err("invalid stdin snapshot");
        }
        let mut file_map = BTreeMap::new();
        for (path, data) in files {
            if path.is_empty()
                || path.contains(&0)
                || data.len() > MAX_STREAM_BYTES
                || file_map.insert(path, data).is_some()
            {
                return Err("invalid file snapshot");
            }
        }
        let base = ptr::addr_of_mut!(aim_bionic___sF).cast::<AndroidFile>();
        let mut streams = BTreeMap::new();
        for index in 0..3 {
            // SAFETY: __sF is exactly three contiguous 152-byte aligned tokens.
            let token = unsafe { base.add(index) } as usize;
            streams.insert(
                token,
                Stream {
                    _token: None,
                    data: if index == 0 {
                        stdin.clone()
                    } else {
                        Vec::new()
                    },
                    position: 0,
                    readable: index == 0,
                    writable: index != 0,
                    pushback: None,
                    fd: index as i32,
                    owns_fd: false,
                    backing_path: None,
                    append: false,
                    orientation: 0,
                    error: false,
                    eof: false,
                    wide_leases: 0,
                    exclusive: false,
                },
            );
        }
        Ok(Self {
            table: Mutex::new(Table {
                streams,
                files: file_map,
                next_fd: 20_000,
                shutting_down: false,
                allow_memory_files,
            }),
            idle: Condvar::new(),
        })
    }
    pub fn activate(self: &Arc<Self>) -> Result<Activation, &'static str> {
        let mut active = slot().write().map_err(|_| "activation lock poisoned")?;
        if active.is_some() {
            return Err("stdio provider already active");
        }
        *active = Some(Arc::clone(self));
        let backend = WideStdioBackend {
            abi_version: 1,
            struct_size: std::mem::size_of::<WideStdioBackend>() as u32,
            context: Arc::as_ptr(self).cast_mut().cast(),
            acquire: wide_acquire,
            release: wide_release,
            orient_wide: wide_orient,
            read_byte: wide_read_byte,
            write_bytes: wide_write_bytes,
            set_error: wide_set_error,
            clear_error_and_eof: wide_clear_error_and_eof,
        };
        // SAFETY: the provider Arc outlives the installed callback table and
        // the C++ owner copies the table before returning.
        let wide = unsafe { aim_bionic_wide_stdio_install(&backend) };
        if wide.is_null() {
            active.take();
            return Err("wide stdio backend installation failed");
        }
        Ok(Activation {
            provider: Arc::clone(self),
            wide,
        })
    }
    pub fn stdout_bytes(&self) -> Vec<u8> {
        self.standard_stream_bytes(1)
    }
    pub fn stderr_bytes(&self) -> Vec<u8> {
        self.standard_stream_bytes(2)
    }
    fn standard_stream_bytes(&self, index: usize) -> Vec<u8> {
        let base = ptr::addr_of_mut!(aim_bionic___sF).cast::<AndroidFile>() as usize;
        self.table
            .lock()
            .unwrap()
            .streams
            .get(&(base + 152 * index))
            .map(|s| s.data.clone())
            .unwrap_or_default()
    }
}
pub struct Activation {
    provider: Arc<Provider>,
    wide: *mut WideStdioActivation,
}

// The opaque activation is process-scoped. Its teardown synchronizes all
// callbacks before the pointer is destroyed, so moving the owner is safe.
unsafe impl Send for Activation {}

impl Drop for Activation {
    fn drop(&mut self) {
        let mut table = match self.provider.table.lock() {
            Ok(table) => table,
            Err(_) => std::process::abort(),
        };
        table.shutting_down = true;
        for stream in table.streams.values_mut() {
            stream.exclusive = true;
        }
        while table.streams.values().any(|stream| stream.wide_leases != 0) {
            table = match self.provider.idle.wait(table) {
                Ok(table) => table,
                Err(_) => std::process::abort(),
            };
        }
        let tokens: Vec<usize> = table.streams.keys().copied().collect();
        for token in tokens {
            // SAFETY: shutdown owns an exclusive central lease for every
            // still-live token and no new wide lease is admitted.
            if unsafe { aim_bionic_wide_stdio_forget(token as *mut AndroidFile) } != 0 {
                std::process::abort();
            }
        }
        let owned_fds: Vec<i32> = table
            .streams
            .values()
            .filter(|stream| stream.owns_fd)
            .map(|stream| stream.fd)
            .collect();
        for fd in owned_fds {
            // SAFETY: shutdown holds the exclusive lease for every stream,
            // and each VFS fallback descriptor has exactly one owner.
            unsafe { aim_bionic_close(fd) };
        }
        table.streams.retain(|_, stream| !stream.owns_fd);
        drop(table);
        // SAFETY: all central leases and side-table entries were drained.
        if unsafe { aim_bionic_wide_stdio_uninstall(&mut self.wide) } != 0 || !self.wide.is_null() {
            std::process::abort();
        }
        if let Ok(mut active) = slot().write()
            && active
                .as_ref()
                .is_some_and(|provider| Arc::ptr_eq(provider, &self.provider))
        {
            active.take();
        }
    }
}

struct ProcessOwner {
    users: usize,
    _provider: Arc<Provider>,
    _activation: Activation,
}

static PROCESS_OWNER: OnceLock<Mutex<Option<ProcessOwner>>> = OnceLock::new();

fn process_owner() -> &'static Mutex<Option<ProcessOwner>> {
    PROCESS_OWNER.get_or_init(|| Mutex::new(None))
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_process_install() -> c_int {
    let mut owner = match process_owner().lock() {
        Ok(owner) => owner,
        Err(_) => {
            errno(EIO);
            return -1;
        }
    };
    if let Some(active) = owner.as_mut() {
        active.users = match active.users.checked_add(1) {
            Some(users) => users,
            None => {
                errno(EOVERFLOW);
                return -1;
            }
        };
        return 0;
    }
    let provider = match Provider::new_with_mode(Vec::new(), Vec::new(), false) {
        Ok(provider) => Arc::new(provider),
        Err(_) => {
            errno(EIO);
            return -1;
        }
    };
    let activation = match provider.activate() {
        Ok(activation) => activation,
        Err(_) => {
            errno(EIO);
            return -1;
        }
    };
    *owner = Some(ProcessOwner {
        users: 1,
        _provider: provider,
        _activation: activation,
    });
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_process_uninstall() -> c_int {
    let removed = {
        let mut owner = match process_owner().lock() {
            Ok(owner) => owner,
            Err(_) => {
                errno(EIO);
                return -1;
            }
        };
        let Some(active) = owner.as_mut() else {
            errno(EINVAL);
            return -1;
        };
        active.users -= 1;
        if active.users == 0 {
            owner.take()
        } else {
            None
        }
    };
    drop(removed);
    0
}

fn errno(value: i32) {
    unsafe { aim_bionic_errno_store(value) }
}
fn provider() -> Option<Arc<Provider>> {
    let value = slot().read().ok()?.clone();
    if value.is_none() {
        errno(EIO);
    }
    value
}

struct WideLease {
    provider: Arc<Provider>,
    token: usize,
}

unsafe fn clone_context(context: *mut c_void) -> Option<Arc<Provider>> {
    let pointer = context.cast::<Provider>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: Activation holds one strong Arc for the complete callback ABI
    // lifetime, so incrementing before constructing the temporary Arc is safe.
    unsafe { Arc::increment_strong_count(pointer) };
    // SAFETY: the preceding increment created this owned strong reference.
    Some(unsafe { Arc::from_raw(pointer) })
}

unsafe fn lease_ref<'a>(lease: *mut c_void) -> Option<&'a WideLease> {
    // SAFETY: the C++ facade passes back only the live pointer returned by
    // wide_acquire and calls release exactly once after its final callback.
    unsafe { lease.cast::<WideLease>().as_ref() }
}

extern "C" fn wide_acquire(
    context: *mut c_void,
    file: *mut AndroidFile,
    output: *mut *mut c_void,
) -> c_int {
    if file.is_null() || output.is_null() {
        errno(EBADF);
        return -1;
    }
    // SAFETY: output was validated writable by the embedding callback ABI.
    unsafe { *output = ptr::null_mut() };
    // SAFETY: context is the Arc inner pointer held by Activation.
    let Some(provider) = (unsafe { clone_context(context) }) else {
        errno(EIO);
        return -1;
    };
    let token = file as usize;
    let mut table = match provider.table.lock() {
        Ok(table) => table,
        Err(_) => {
            errno(EIO);
            return -1;
        }
    };
    loop {
        if table.shutting_down {
            errno(EIO);
            return -1;
        }
        let Some(stream) = table.streams.get_mut(&token) else {
            errno(EBADF);
            return -1;
        };
        if !stream.exclusive {
            // The wide backend contract grants one stream lease held through
            // release(). Marking it exclusive prevents byte operations,
            // close, seek/reset, and a second wide operation from
            // interleaving with a multibyte conversion.
            stream.exclusive = true;
            stream.wide_leases = 1;
            break;
        }
        table = match provider.idle.wait(table) {
            Ok(table) => table,
            Err(_) => {
                errno(EIO);
                return -1;
            }
        };
    }
    drop(table);
    let lease = Box::new(WideLease { provider, token });
    // SAFETY: output is live and receives ownership until wide_release.
    unsafe { *output = Box::into_raw(lease).cast() };
    0
}

extern "C" fn wide_release(_: *mut c_void, lease: *mut c_void) {
    if lease.is_null() {
        std::process::abort();
    }
    // SAFETY: release consumes the unique allocation returned by acquire.
    let lease = unsafe { Box::from_raw(lease.cast::<WideLease>()) };
    let mut table = match lease.provider.table.lock() {
        Ok(table) => table,
        Err(_) => std::process::abort(),
    };
    let shutting_down = table.shutting_down;
    let Some(stream) = table.streams.get_mut(&lease.token) else {
        std::process::abort();
    };
    if stream.wide_leases != 1 || !stream.exclusive {
        std::process::abort();
    }
    stream.wide_leases = 0;
    if !shutting_down {
        stream.exclusive = false;
    }
    lease.provider.idle.notify_all();
}

fn with_wide_lease<T>(lease: *mut c_void, error: T, operation: impl FnOnce(&mut Stream) -> T) -> T {
    // SAFETY: callback lifetime is nested within acquire/release.
    let Some(lease) = (unsafe { lease_ref(lease) }) else {
        errno(EIO);
        return error;
    };
    let mut table = match lease.provider.table.lock() {
        Ok(table) => table,
        Err(_) => {
            errno(EIO);
            return error;
        }
    };
    let Some(stream) = table.streams.get_mut(&lease.token) else {
        errno(EIO);
        return error;
    };
    if stream.wide_leases == 0 {
        errno(EIO);
        return error;
    }
    operation(stream)
}

extern "C" fn wide_orient(_: *mut c_void, lease: *mut c_void) -> c_int {
    with_wide_lease(lease, -1, |stream| {
        if stream.orientation == 0 {
            stream.orientation = 1;
        }
        0
    })
}

extern "C" fn wide_read_byte(_: *mut c_void, lease: *mut c_void, output: *mut u8) -> c_int {
    if output.is_null() {
        errno(EFAULT);
        return -1;
    }
    with_wide_lease(lease, -1, |stream| {
        if !stream.readable {
            stream.error = true;
            errno(EBADF);
            return -1;
        }
        let byte = if let Some(byte) = stream.pushback.take() {
            byte
        } else if stream.owns_fd {
            let mut byte = 0_u8;
            loop {
                let result =
                    unsafe { aim_bionic_read(stream.fd, (&mut byte as *mut u8).cast(), 1) };
                if result < 0 && unsafe { aim_bionic_errno_load() } == EINTR {
                    continue;
                }
                if result < 0 {
                    stream.error = true;
                    return -1;
                }
                if result == 0 {
                    stream.eof = true;
                    return 0;
                }
                stream.position = stream.position.saturating_add(1);
                break byte;
            }
        } else if stream.position < stream.data.len() {
            let byte = stream.data[stream.position];
            stream.position += 1;
            byte
        } else {
            stream.eof = true;
            return 0;
        };
        // SAFETY: output was validated and the callback ABI grants one byte.
        unsafe { *output = byte };
        1
    })
}

extern "C" fn wide_write_bytes(
    _: *mut c_void,
    lease: *mut c_void,
    bytes: *const u8,
    length: usize,
) -> c_int {
    if length > isize::MAX as usize || (length != 0 && bytes.is_null()) {
        errno(if bytes.is_null() { EFAULT } else { EOVERFLOW });
        return -1;
    }
    let input: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: nonempty input was validated for the callback length.
        unsafe { slice::from_raw_parts(bytes, length) }
    };
    with_wide_lease(lease, -1, |stream| {
        if !stream.writable {
            stream.error = true;
            errno(EBADF);
            return -1;
        }
        if stream.append && !stream.owns_fd {
            stream.position = stream.data.len();
        }
        let Some(end) = stream.position.checked_add(length) else {
            stream.error = true;
            errno(EOVERFLOW);
            return -1;
        };
        if !stream.owns_fd && end > MAX_STREAM_BYTES {
            stream.error = true;
            errno(EFBIG);
            return -1;
        }
        if stream.owns_fd {
            if stream.pushback.take().is_some()
                && unsafe { aim_bionic_lseek(stream.fd, stream.position as i64, 0) }
                    != stream.position as i64
            {
                stream.error = true;
                return -1;
            }
            let mut written = 0;
            while written < input.len() {
                let result = unsafe {
                    aim_bionic_write(
                        stream.fd,
                        input[written..].as_ptr().cast(),
                        input.len() - written,
                    )
                };
                if result < 0 {
                    if unsafe { aim_bionic_errno_load() } == EINTR {
                        continue;
                    }
                    stream.error = true;
                    return -1;
                }
                if result == 0 {
                    stream.error = true;
                    return -1;
                }
                written += result as usize;
            }
            stream.position = if stream.append {
                let actual = unsafe { aim_bionic_lseek(stream.fd, 0, 1) };
                if actual < 0 {
                    stream.error = true;
                    return -1;
                }
                actual as usize
            } else {
                end
            };
            return 0;
        }
        if stream.position > stream.data.len() {
            stream.data.resize(stream.position, 0);
        }
        if end > stream.data.len() {
            stream.data.resize(end, 0);
        }
        stream.data[stream.position..end].copy_from_slice(input);
        stream.position = end;
        0
    })
}

extern "C" fn wide_set_error(_: *mut c_void, lease: *mut c_void) {
    with_wide_lease(lease, (), |stream| stream.error = true);
}

extern "C" fn wide_clear_error_and_eof(_: *mut c_void, lease: *mut c_void) {
    with_wide_lease(lease, (), |stream| {
        stream.error = false;
        stream.eof = false;
    });
}
unsafe fn bytes<'a>(p: *const c_char) -> Option<&'a [u8]> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_bytes())
    }
}
fn parse_mode(mode: &[u8]) -> Option<(bool, bool, bool, bool)> {
    let (&first, rest) = mode.split_first()?;
    let (mut readable, mut writable, truncate, append) = match first {
        b'r' => (true, false, false, false),
        b'w' => (false, true, true, false),
        b'a' => (false, true, false, true),
        _ => return None,
    };
    let mut plus = false;
    let mut binary = false;
    let mut close_on_exec = false;
    for byte in rest {
        match byte {
            b'+' if !plus => {
                plus = true;
                readable = true;
                writable = true;
            }
            // Bionic's libc++ basic_filebuf uses "e" to request O_CLOEXEC.
            // It is a mode modifier, not a distinct stream orientation.
            b'e' if !close_on_exec => close_on_exec = true,
            b'b' if !binary => binary = true,
            _ => return None,
        }
    }
    let _ = (plus, binary, close_on_exec);
    Some((readable, writable, truncate, append))
}

unsafe fn fopen(path: *const c_char, mode: *const c_char) -> *mut AndroidFile {
    let (Some(path), Some(mode)) = (unsafe { bytes(path) }, unsafe { bytes(mode) }) else {
        errno(EFAULT);
        return ptr::null_mut();
    };
    if path.is_empty() {
        errno(2);
        return ptr::null_mut();
    }
    let Some((readable, writable, truncate, append)) = parse_mode(mode) else {
        errno(EINVAL);
        return ptr::null_mut();
    };
    let Some(provider) = provider() else {
        return ptr::null_mut();
    };
    let mut table = match provider.table.lock() {
        Ok(v) => v,
        Err(_) => {
            errno(EIO);
            return ptr::null_mut();
        }
    };
    if table.shutting_down {
        errno(EIO);
        return ptr::null_mut();
    }
    let mut guest_fd = None;
    let mut backing_path = None;
    let mut initial_position = 0_usize;
    let data = if append {
        // Android append streams create the file through the same guest VFS
        // as open(2). Keep the descriptor live so fileno() observes a real
        // Android descriptor rather than a facade-only snapshot token.
        let access = if readable { O_RDWR } else { O_WRONLY };
        let fd =
            unsafe { aim_bionic_open(path.as_ptr().cast(), access | O_CREAT | O_APPEND, 0o666) };
        if fd >= 0 {
            let end = unsafe { aim_bionic_lseek(fd, 0, 2) };
            let Some(end) = usize::try_from(end).ok() else {
                unsafe { aim_bionic_close(fd) };
                return ptr::null_mut();
            };
            initial_position = end;
            guest_fd = Some(fd);
            Vec::new()
        } else if table.allow_memory_files {
            let Some(v) = table.files.get(path) else {
                errno(2);
                return ptr::null_mut();
            };
            backing_path = Some(path.to_vec());
            initial_position = v.len();
            v.clone()
        } else {
            return ptr::null_mut();
        }
    } else if truncate || (readable && writable) {
        // Writable streams must be real guest descriptors whenever the VFS
        // is installed. The table fallback is retained for the standalone
        // stdio fixture, which intentionally has no filesystem provider.
        let access = if readable { O_RDWR } else { O_WRONLY };
        let flags = if truncate {
            access | O_CREAT | O_TRUNC
        } else {
            access
        };
        let fd = unsafe { aim_bionic_open(path.as_ptr().cast(), flags, 0o666) };
        if fd >= 0 {
            guest_fd = Some(fd);
            Vec::new()
        } else if table.allow_memory_files {
            let v = table.files.get(path);
            if v.is_none() && !truncate {
                return ptr::null_mut();
            }
            backing_path = Some(path.to_vec());
            if truncate {
                Vec::new()
            } else {
                v.cloned().expect("checked fixture file")
            }
        } else {
            return ptr::null_mut();
        }
    } else {
        match table.files.get(path) {
            Some(v) => v.clone(),
            None => {
                // Real read-only files stay descriptor-backed. This avoids
                // imposing MAX_STREAM_BYTES on package data such as Unity's
                // 300+ MiB ExcelDB.db.
                let fd = unsafe { aim_bionic_open(path.as_ptr().cast(), 0, 0) };
                if fd < 0 {
                    return ptr::null_mut();
                }
                guest_fd = Some(fd);
                Vec::new()
            }
        }
    };
    let token = Box::new(AndroidFile { opaque: [0; 152] });
    let pointer = (&*token as *const AndroidFile).cast_mut();
    let fd = guest_fd.unwrap_or_else(|| {
        let fd = table.next_fd;
        table.next_fd += 1;
        fd
    });
    table.streams.insert(
        pointer as usize,
        Stream {
            _token: Some(token),
            data,
            position: initial_position,
            readable,
            writable,
            pushback: None,
            fd,
            owns_fd: guest_fd.is_some(),
            backing_path,
            append,
            orientation: 0,
            error: false,
            eof: false,
            wide_leases: 0,
            exclusive: false,
        },
    );
    pointer
}
fn with_stream<T>(file: *mut AndroidFile, error: T, operation: impl FnOnce(&mut Stream) -> T) -> T
where
    T: Copy,
{
    let Some(provider) = provider() else {
        return error;
    };
    let mut table = match provider.table.lock() {
        Ok(v) => v,
        Err(_) => {
            errno(EIO);
            return error;
        }
    };
    let token = file as usize;
    loop {
        if table.shutting_down {
            errno(EIO);
            return error;
        }
        let Some(stream) = table.streams.get_mut(&token) else {
            errno(EBADF);
            return error;
        };
        if !stream.exclusive {
            return operation(stream);
        }
        table = match provider.idle.wait(table) {
            Ok(table) => table,
            Err(_) => {
                errno(EIO);
                return error;
            }
        };
    }
}

fn orient_byte(stream: &mut Stream) {
    if stream.orientation == 0 {
        stream.orientation = -1;
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `p` and `m` must be readable NUL-terminated Android byte strings.
pub unsafe extern "C" fn aim_bionic_stdio_fopen_core(
    p: *const c_char,
    m: *const c_char,
) -> *mut AndroidFile {
    unsafe { fopen(p, m) }
}
#[unsafe(no_mangle)]
/// # Safety
/// `f` must be null or a live stream token owned by the active provider.
pub unsafe extern "C" fn aim_bionic_stdio_fclose_core(f: *mut AndroidFile) -> c_int {
    let Some(p) = provider() else { return EOF };
    let mut t = match p.table.lock() {
        Ok(v) => v,
        Err(_) => {
            errno(EIO);
            return EOF;
        }
    };
    let token = f as usize;
    loop {
        if t.shutting_down {
            errno(EIO);
            return EOF;
        }
        let Some(stream) = t.streams.get_mut(&token) else {
            errno(EBADF);
            return EOF;
        };
        if !stream.exclusive {
            stream.exclusive = true;
            break;
        }
        t = match p.idle.wait(t) {
            Ok(table) => table,
            Err(_) => {
                errno(EIO);
                return EOF;
            }
        };
    }
    while t
        .streams
        .get(&token)
        .is_some_and(|stream| stream.wide_leases != 0)
    {
        t = match p.idle.wait(t) {
            Ok(table) => table,
            Err(_) => std::process::abort(),
        };
    }
    // SAFETY: close holds the central exclusive lease and all wide leases
    // drained before the token can be removed or its Box reused.
    if unsafe { aim_bionic_wide_stdio_forget(f) } != 0 {
        if let Some(stream) = t.streams.get_mut(&token) {
            stream.exclusive = false;
        }
        p.idle.notify_all();
        return EOF;
    }
    let removed = t.streams.remove(&token);
    if let Some(stream) = removed.as_ref()
        && let Some(path) = stream.backing_path.as_ref()
    {
        // Snapshot-only fixture streams have no descriptor to persist their
        // writes through. Commit them while the table is still locked; real
        // VFS-backed streams have already written to their guest descriptor.
        t.files.insert(path.clone(), stream.data.clone());
    }
    p.idle.notify_all();
    drop(t);
    if let Some(stream) = removed.as_ref()
        && stream.owns_fd
    {
        // SAFETY: this stream exclusively owns the guest descriptor returned
        // by the VFS fallback in fopen.
        unsafe { aim_bionic_close(stream.fd) };
    }
    drop(removed);
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_fflush_core(f: *mut AndroidFile) -> c_int {
    if f.is_null() {
        if provider().is_some() { 0 } else { EOF }
    } else {
        with_stream(f, EOF, |_| 0)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_fileno_core(f: *mut AndroidFile) -> c_int {
    with_stream(f, EOF, |s| s.fd)
}
#[unsafe(no_mangle)]
/// # Safety
/// For a nonzero byte product, `b` must be writable for `size * count` bytes.
pub unsafe extern "C" fn aim_bionic_stdio_fread_core(
    b: *mut c_void,
    size: usize,
    count: usize,
    f: *mut AndroidFile,
) -> usize {
    let Some(total) = size.checked_mul(count) else {
        errno(EOVERFLOW);
        return 0;
    };
    if total == 0 {
        return 0;
    }
    if total > isize::MAX as usize {
        errno(EOVERFLOW);
        return 0;
    }
    if b.is_null() {
        errno(EFAULT);
        return 0;
    }
    with_stream(f, 0, |s| {
        orient_byte(s);
        if !s.readable {
            s.error = true;
            errno(EBADF);
            return 0;
        }
        let out = unsafe { slice::from_raw_parts_mut(b.cast::<u8>(), total) };
        let mut n = 0;
        if let Some(v) = s.pushback.take() {
            out[0] = v;
            n = 1
        }
        let take = if s.owns_fd {
            let mut read = 0;
            while read < total - n {
                let result = unsafe {
                    aim_bionic_read(s.fd, out[n + read..].as_mut_ptr().cast(), total - n - read)
                };
                if result < 0 {
                    if unsafe { aim_bionic_errno_load() } == EINTR {
                        continue;
                    }
                    s.error = true;
                    break;
                }
                if result == 0 {
                    break;
                }
                read += result as usize;
            }
            read
        } else {
            let available = s.data.len().saturating_sub(s.position);
            let take = (total - n).min(available);
            out[n..n + take].copy_from_slice(&s.data[s.position..s.position + take]);
            take
        };
        s.position += take;
        if n + take < total {
            s.eof = true;
        }
        (n + take) / size
    })
}
#[unsafe(no_mangle)]
/// # Safety
/// For a nonzero byte product, `b` must be readable for `size * count` bytes.
pub unsafe extern "C" fn aim_bionic_stdio_fwrite_core(
    b: *const c_void,
    size: usize,
    count: usize,
    f: *mut AndroidFile,
) -> usize {
    let Some(total) = size.checked_mul(count) else {
        errno(EOVERFLOW);
        return 0;
    };
    if total == 0 {
        return 0;
    }
    if total > isize::MAX as usize {
        errno(EOVERFLOW);
        return 0;
    }
    if b.is_null() {
        errno(EFAULT);
        return 0;
    }
    let input = unsafe { slice::from_raw_parts(b.cast::<u8>(), total) };
    let mut mirror_stderr = false;
    let written = with_stream(f, 0, |s| {
        orient_byte(s);
        if !s.writable {
            s.error = true;
            errno(EBADF);
            return 0;
        }
        if s.append && !s.owns_fd {
            s.position = s.data.len();
        }
        let Some(end) = s.position.checked_add(total) else {
            errno(EOVERFLOW);
            return 0;
        };
        if !s.owns_fd && end > MAX_STREAM_BYTES {
            s.error = true;
            errno(EFBIG);
            return 0;
        }
        let bytes_written = if s.owns_fd {
            if s.pushback.take().is_some()
                && unsafe { aim_bionic_lseek(s.fd, s.position as i64, 0) } != s.position as i64
            {
                s.error = true;
                return 0;
            }
            let mut written = 0;
            while written < total {
                let result = unsafe {
                    aim_bionic_write(s.fd, input[written..].as_ptr().cast(), total - written)
                };
                if result < 0 {
                    if unsafe { aim_bionic_errno_load() } == EINTR {
                        continue;
                    }
                    s.error = true;
                    break;
                }
                if result == 0 {
                    s.error = true;
                    break;
                }
                written += result as usize;
            }
            written
        } else {
            if s.position > s.data.len() {
                s.data.resize(s.position, 0)
            }
            if end > s.data.len() {
                s.data.resize(end, 0)
            }
            s.data[s.position..end].copy_from_slice(input);
            total
        };
        if s.owns_fd {
            let new_end = if s.append {
                let actual = unsafe { aim_bionic_lseek(s.fd, 0, 1) };
                if actual < 0 {
                    s.error = true;
                    return 0;
                }
                actual as usize
            } else {
                s.position.saturating_add(bytes_written)
            };
            s.position = new_end;
        } else {
            s.position = end;
        }
        mirror_stderr = s.fd == 2;
        bytes_written / size
    });
    if written == count && mirror_stderr {
        let _ = std::io::stderr().write_all(input);
    }
    written
}
#[unsafe(no_mangle)]
/// # Safety
/// `f` must be a live stream token owned by the active provider.
pub unsafe extern "C" fn aim_bionic_stdio_fseek_core(
    f: *mut AndroidFile,
    offset: i64,
    whence: c_int,
) -> c_int {
    let Some(provider) = provider() else {
        return EOF;
    };
    let token = f as usize;
    let mut table = match provider.table.lock() {
        Ok(table) => table,
        Err(_) => {
            errno(EIO);
            return EOF;
        }
    };
    loop {
        if table.shutting_down {
            errno(EIO);
            return EOF;
        }
        let Some(stream) = table.streams.get_mut(&token) else {
            errno(EBADF);
            return EOF;
        };
        if !stream.exclusive {
            stream.exclusive = true;
            break;
        }
        table = match provider.idle.wait(table) {
            Ok(table) => table,
            Err(_) => {
                errno(EIO);
                return EOF;
            }
        };
    }
    while table
        .streams
        .get(&token)
        .is_some_and(|stream| stream.wide_leases != 0)
    {
        table = match provider.idle.wait(table) {
            Ok(table) => table,
            Err(_) => std::process::abort(),
        };
    }
    let result = {
        let stream = table.streams.get_mut(&token).expect("exclusive stream");
        let fd_position = if stream.owns_fd {
            let position = unsafe { aim_bionic_lseek(stream.fd, 0, 1) };
            if position < 0 {
                stream.error = true;
                stream.exclusive = false;
                provider.idle.notify_all();
                return EOF;
            }
            Some(position)
        } else {
            None
        };
        let base = match whence {
            0 => 0i128,
            1 => match fd_position {
                Some(position) => position as i128 - i128::from(stream.pushback.is_some()),
                None => stream.position as i128,
            },
            2 if stream.owns_fd => {
                let end = unsafe { aim_bionic_lseek(stream.fd, 0, 2) };
                if end < 0 {
                    if let Some(position) = fd_position {
                        let _ = unsafe { aim_bionic_lseek(stream.fd, position, 0) };
                    }
                    stream.error = true;
                    stream.exclusive = false;
                    provider.idle.notify_all();
                    return EOF;
                }
                // Discovering the end must not commit a position change: the
                // requested offset or wide-state reset may still fail below.
                if let Some(position) = fd_position
                    && unsafe { aim_bionic_lseek(stream.fd, position, 0) } != position
                {
                    stream.error = true;
                    stream.exclusive = false;
                    provider.idle.notify_all();
                    return EOF;
                }
                end as i128
            }
            2 => stream.data.len() as i128,
            _ => {
                errno(EINVAL);
                stream.exclusive = false;
                provider.idle.notify_all();
                return EOF;
            }
        };
        let value = base + offset as i128;
        if value < 0 || value > i64::MAX as i128 {
            if let Some(position) = fd_position {
                let _ = unsafe { aim_bionic_lseek(stream.fd, position, 0) };
            }
            errno(EINVAL);
            stream.exclusive = false;
            provider.idle.notify_all();
            return EOF;
        }
        value as usize
    };
    // SAFETY: seek owns the central exclusive lease and all wide calls drained.
    if unsafe { aim_bionic_wide_stdio_reset(f) } != 0 {
        let stream = table.streams.get_mut(&token).expect("exclusive stream");
        stream.exclusive = false;
        provider.idle.notify_all();
        return EOF;
    }
    let stream = table.streams.get_mut(&token).expect("exclusive stream");
    if stream.owns_fd && unsafe { aim_bionic_lseek(stream.fd, result as i64, 0) } != result as i64 {
        stream.error = true;
        stream.exclusive = false;
        provider.idle.notify_all();
        return EOF;
    }
    stream.position = result;
    stream.pushback = None;
    stream.eof = false;
    stream.exclusive = false;
    provider.idle.notify_all();
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_ftello_core(f: *mut AndroidFile) -> i64 {
    with_stream(f, -1, |s| {
        if s.owns_fd && s.pushback.is_none() {
            let position = unsafe { aim_bionic_lseek(s.fd, 0, 1) };
            if position < 0 {
                s.error = true;
                return -1;
            }
            s.position = position as usize;
        }
        match i64::try_from(s.position) {
            Ok(v) => v,
            Err(_) => {
                errno(EOVERFLOW);
                -1
            }
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_fputc_core(c: c_int, f: *mut AndroidFile) -> c_int {
    let byte = (c & 255) as u8;
    with_stream(f, EOF, |s| {
        orient_byte(s);
        if !s.writable {
            s.error = true;
            errno(EBADF);
            return EOF;
        }
        if s.append && !s.owns_fd {
            s.position = s.data.len();
        }
        let Some(end) = s.position.checked_add(1) else {
            errno(EOVERFLOW);
            return EOF;
        };
        if !s.owns_fd && end > MAX_STREAM_BYTES {
            s.error = true;
            errno(EFBIG);
            return EOF;
        }
        if s.owns_fd {
            if s.pushback.take().is_some()
                && unsafe { aim_bionic_lseek(s.fd, s.position as i64, 0) } != s.position as i64
            {
                s.error = true;
                return EOF;
            }
            loop {
                let result = unsafe { aim_bionic_write(s.fd, (&byte as *const u8).cast(), 1) };
                if result < 0 && unsafe { aim_bionic_errno_load() } == EINTR {
                    continue;
                }
                if result != 1 {
                    s.error = true;
                    return EOF;
                }
                break;
            }
            s.position = if s.append {
                let actual = unsafe { aim_bionic_lseek(s.fd, 0, 1) };
                if actual < 0 {
                    s.error = true;
                    return EOF;
                }
                actual as usize
            } else {
                s.position.saturating_add(1)
            };
        } else {
            if s.position > s.data.len() {
                s.data.resize(s.position, 0)
            }
            if s.position == s.data.len() {
                s.data.push(byte)
            } else {
                s.data[s.position] = byte
            }
            s.position = end;
        }
        byte as i32
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_getc_core(f: *mut AndroidFile) -> c_int {
    with_stream(f, EOF, |s| {
        orient_byte(s);
        if !s.readable {
            s.error = true;
            errno(EBADF);
            return EOF;
        }
        if let Some(v) = s.pushback.take() {
            return v as i32;
        }
        if s.owns_fd {
            let mut byte = 0_u8;
            loop {
                let result = unsafe { aim_bionic_read(s.fd, (&mut byte as *mut u8).cast(), 1) };
                if result < 0 && unsafe { aim_bionic_errno_load() } == EINTR {
                    continue;
                }
                if result < 0 {
                    s.error = true;
                    return EOF;
                }
                if result == 0 {
                    s.eof = true;
                    return EOF;
                }
                break;
            }
            s.position = s.position.saturating_add(1);
            return byte as i32;
        }
        if s.position >= s.data.len() {
            s.eof = true;
            EOF
        } else {
            let v = s.data[s.position];
            s.position += 1;
            v as i32
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_ungetc_core(c: c_int, f: *mut AndroidFile) -> c_int {
    if c == EOF {
        return EOF;
    }
    with_stream(f, EOF, |s| {
        orient_byte(s);
        if !s.readable || s.pushback.is_some() {
            return EOF;
        }
        let v = (c & 255) as u8;
        s.pushback = Some(v);
        s.eof = false;
        v as i32
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_feof_core(f: *mut AndroidFile) -> c_int {
    with_stream(f, 0, |stream| i32::from(stream.eof))
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_ferror_core(f: *mut AndroidFile) -> c_int {
    with_stream(f, 0, |stream| i32::from(stream.error))
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_stdio_clearerr_core(f: *mut AndroidFile) {
    with_stream(f, (), |stream| {
        stream.error = false;
        stream.eof = false;
    });
}

#[unsafe(no_mangle)]
/// Runs a Bionic scanner while holding the provider-owned FILE cursor lease.
///
/// # Safety
/// `file` must be null or a live provider token. `callback`, when present,
/// must obey the callback ABI and may read the NUL-terminated input only for
/// the duration of this call. `context` is owned by the caller.
pub unsafe extern "C" fn aim_bionic_stdio_scan_core(
    file: *mut AndroidFile,
    callback: Option<ScanCallback>,
    context: *mut c_void,
) -> c_int {
    let Some(callback) = callback else {
        errno(EINVAL);
        return EOF;
    };
    with_stream(file, EOF, |stream| {
        orient_byte(stream);
        if !stream.readable {
            stream.error = true;
            errno(EBADF);
            return EOF;
        }
        // The callback ABI requires one contiguous NUL-terminated view of
        // the remaining stream. A descriptor-backed production stream is
        // intentionally unbounded (e.g. ExcelDB.db), so never index its
        // empty mirror or materialize the whole file here. Callers needing
        // scanner semantics must use fread on the FD-backed stream.
        if stream.owns_fd {
            stream.error = true;
            errno(EOPNOTSUPP);
            return EOF;
        }

        let had_pushback = stream.pushback.is_some();
        let remaining = stream.data.len().saturating_sub(stream.position);
        let Some(input_length) = remaining.checked_add(usize::from(had_pushback)) else {
            stream.error = true;
            errno(EOVERFLOW);
            return EOF;
        };
        let Some(capacity) = input_length.checked_add(1) else {
            stream.error = true;
            errno(EOVERFLOW);
            return EOF;
        };
        let mut input = Vec::with_capacity(capacity);
        if let Some(byte) = stream.pushback {
            input.push(byte);
        }
        input.extend_from_slice(&stream.data[stream.position..]);
        input.push(0);

        let mut consumed = 0_usize;
        // SAFETY: the callback and context satisfy this function's contract;
        // input and consumed remain live and uniquely borrowed for the call.
        let result =
            unsafe { callback(input.as_ptr().cast(), input_length, context, &mut consumed) };
        if consumed > input_length {
            stream.error = true;
            errno(EIO);
            return EOF;
        }

        if had_pushback && consumed != 0 {
            stream.pushback = None;
            consumed -= 1;
        }
        stream.position += consumed;
        if stream.pushback.is_none() && consumed == remaining {
            stream.eof = true;
        }
        result
    })
}
