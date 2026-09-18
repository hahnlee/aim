//! Actual C++-factory -> Rust-inheritance-boundary coverage.
//!
//! The test builds only the adapter translation unit into a private dylib and
//! loads it through dlsym.  It deliberately pauses after the C++ operation has
//! created and CLOEXEC-protected the pipe, while `with_native_operation` still
//! owns the same Rust guard used by `spawn_owned`.  This proves callback/guard
//! closure; it is not a pre-CLOEXEC unsafe-window injection test.

#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]

use darwin_art_scm_transfer::inheritance::{NativeFdOperation, spawn_owned, with_native_operation};
use std::{
    ffi::{CStr, CString},
    io, mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    sync::{Arc, Condvar, Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

type NativeBoundary = unsafe extern "C" fn(Option<NativeFdOperation>, *mut libc::c_void) -> isize;
type InstallBoundary = unsafe extern "C" fn(Option<NativeBoundary>) -> libc::c_int;
type PipeFactory = unsafe extern "C" fn(*mut libc::c_int) -> libc::c_int;

const INSTALL_SYMBOL: &[u8] = b"darwin_art_bionic_install_fd_inheritance_boundary\0";
const PIPE_FACTORY_SYMBOL: &[u8] =
    b"_ZN10darwin_art6bionic14fd_inheritance21CreateCloseOnExecPipeEPi\0";

struct TempStage {
    path: PathBuf,
}

impl TempStage {
    fn create() -> io::Result<Self> {
        let base = std::env::temp_dir();
        let pid = std::process::id();
        for serial in 0..1000u32 {
            let path = base.join(format!("darwin-art-native-boundary-{pid}-{serial}"));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique native-boundary test directory",
        ))
    }

    fn dylib_path(&self) -> PathBuf {
        self.path.join("libdarwin_art_fd_inheritance_test.dylib")
    }
}

impl Drop for TempStage {
    fn drop(&mut self) {
        // The stage owns only this exact path; NativeLibrary is dropped before
        // the stage so dlclose always precedes removal of the loaded image.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct NativeLibrary {
    handle: *mut libc::c_void,
    install: InstallBoundary,
    create_pipe: PipeFactory,
}

impl NativeLibrary {
    fn load(path: &Path) -> io::Result<Self> {
        let encoded = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "dylib path contains NUL"))?;
        let handle = unsafe { libc::dlopen(encoded.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(dl_error("dlopen"));
        }

        let install = match unsafe { load_symbol(handle, INSTALL_SYMBOL) } {
            Ok(address) => unsafe { mem::transmute::<*mut libc::c_void, InstallBoundary>(address) },
            Err(error) => {
                unsafe { libc::dlclose(handle) };
                return Err(error);
            }
        };
        let create_pipe = match unsafe { load_symbol(handle, PIPE_FACTORY_SYMBOL) } {
            Ok(address) => unsafe { mem::transmute::<*mut libc::c_void, PipeFactory>(address) },
            Err(error) => {
                unsafe { libc::dlclose(handle) };
                return Err(error);
            }
        };
        Ok(Self {
            handle,
            install,
            create_pipe,
        })
    }
}

impl Drop for NativeLibrary {
    fn drop(&mut self) {
        // All factory and spawn threads are joined before this owner leaves
        // scope.  No callback can execute against an unloaded image.
        if !self.handle.is_null() {
            unsafe { libc::dlclose(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}

unsafe fn load_symbol(handle: *mut libc::c_void, name: &[u8]) -> io::Result<*mut libc::c_void> {
    let address = unsafe { libc::dlsym(handle, name.as_ptr().cast()) };
    if address.is_null() {
        Err(dl_error("dlsym"))
    } else {
        Ok(address)
    }
}

fn dl_error(operation: &str) -> io::Error {
    let message = unsafe { libc::dlerror() };
    if message.is_null() {
        io::Error::new(io::ErrorKind::Other, operation.to_owned())
    } else {
        let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
        io::Error::new(io::ErrorKind::Other, format!("{operation}: {message}"))
    }
}

fn build_native_dylib(stage: &TempStage) -> io::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "crate has no workspace root"))?;
    let source = root.join("tools/bionic-socket-broker-adapter/src/fd_inheritance.cc");
    let output_path = stage.dylib_path();
    let mut command = Command::new("clang++");
    command
        .arg("-std=c++20")
        .arg("-dynamiclib")
        .arg("-O0")
        .arg("-g")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-I")
        .arg(root)
        .arg(source)
        .arg("-o")
        .arg(&output_path);

    // This compilation is itself a spawn operation and is intentionally
    // protected by the production Rust boundary. Waiting for compiler output
    // happens after spawn_owned releases the guard.
    let output = darwin_art_scm_transfer::inheritance::output_owned(&mut command)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "clang++ failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

struct PauseState {
    state: Mutex<PauseInner>,
    changed: Condvar,
}

struct PauseInner {
    entered: bool,
    release: bool,
}

impl PauseState {
    fn new() -> Self {
        Self {
            state: Mutex::new(PauseInner {
                entered: false,
                release: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn wait_until_entered(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap();
        while !state.entered {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
            if result.timed_out() && !state.entered {
                return false;
            }
        }
        true
    }

    fn enter_and_wait(&self) {
        let mut state = self.state.lock().unwrap();
        state.entered = true;
        self.changed.notify_all();
        while !state.release {
            state = self.changed.wait(state).unwrap();
        }
    }

    fn release(&self) {
        let mut state = self.state.lock().unwrap();
        state.release = true;
        self.changed.notify_all();
    }
}

static PAUSE: OnceLock<Arc<PauseState>> = OnceLock::new();

struct NativeCall {
    operation: NativeFdOperation,
    context: *mut libc::c_void,
}

unsafe extern "C" fn invoke_cpp_operation_and_pause(context: *mut libc::c_void) -> isize {
    let call = unsafe { &*(context.cast::<NativeCall>()) };
    let result = unsafe { (call.operation)(call.context) };
    let native_errno = unsafe { *libc::__error() };
    // This is the only blocking wait in the test. Production callbacks never
    // wait for input while holding the inheritance guard.
    PAUSE.get().unwrap().enter_and_wait();
    unsafe { *libc::__error() = native_errno };
    result
}

unsafe extern "C" fn rust_boundary(
    operation: Option<NativeFdOperation>,
    context: *mut libc::c_void,
) -> isize {
    let Some(operation) = operation else {
        unsafe { *libc::__error() = libc::EINVAL };
        return -1;
    };
    let call = NativeCall { operation, context };
    unsafe {
        with_native_operation(
            Some(invoke_cpp_operation_and_pause),
            (&call as *const NativeCall).cast_mut().cast(),
        )
    }
}

struct ChildCleanup(Option<Child>);

impl Drop for ChildCleanup {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

#[test]
fn cpp_pipe_factory_uses_same_rust_guard_as_owned_spawn() {
    let stage = TempStage::create().unwrap();
    build_native_dylib(&stage).unwrap();
    let library = NativeLibrary::load(&stage.dylib_path()).unwrap();

    let pause = Arc::new(PauseState::new());
    assert!(PAUSE.set(pause.clone()).is_ok());
    assert_eq!(unsafe { (library.install)(Some(rust_boundary)) }, 0);

    let (factory_tx, factory_rx) = mpsc::channel();
    let create_pipe = library.create_pipe;
    let factory_thread = thread::spawn(move || {
        let mut descriptors = [-1; 2];
        let result = unsafe { create_pipe(descriptors.as_mut_ptr()) };
        factory_tx.send((result, descriptors)).unwrap();
    });

    if !pause.wait_until_entered(Duration::from_secs(2)) {
        pause.release();
        factory_thread.join().unwrap();
        panic!("C++ factory did not enter the Rust inheritance callback");
    }
    let (spawn_tx, spawn_rx) = mpsc::channel();
    let spawn_thread = thread::spawn(move || {
        let mut command = Command::new("/bin/sleep");
        command
            .arg("10")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        spawn_tx.send(spawn_owned(&mut command)).unwrap();
    });

    // The exact guard acquired by with_native_operation is still held after
    // the C++ operation has set both CLOEXEC bits. No unsafe pre-CLOEXEC fork
    // window is injected here; this is a closure/serialization proof only.
    match spawn_rx.recv_timeout(Duration::from_millis(150)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Ok(Ok(mut child)) => {
            pause.release();
            let _ = factory_thread.join();
            spawn_thread.join().unwrap();
            let _ = child.kill();
            let _ = child.wait();
            panic!("spawn_owned completed while the native callback held the guard");
        }
        Ok(Err(error)) => {
            pause.release();
            let _ = factory_thread.join();
            spawn_thread.join().unwrap();
            panic!("spawn_owned failed before the native callback was released: {error}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            pause.release();
            let _ = factory_thread.join();
            spawn_thread.join().unwrap();
            panic!("spawn_owned worker disconnected before release");
        }
    }
    pause.release();

    let spawn_result = spawn_rx.recv_timeout(Duration::from_secs(3));
    let factory_result = factory_rx.recv_timeout(Duration::from_secs(3));
    // Keep the native library loaded until both workers have left its code,
    // including when a timed receive failed.
    spawn_thread.join().unwrap();
    factory_thread.join().unwrap();
    let mut child = ChildCleanup(Some(spawn_result.unwrap().unwrap()));
    let (result, descriptors) = factory_result.unwrap();
    assert_eq!(result, 0);
    assert!(descriptors.iter().all(|descriptor| *descriptor >= 0));

    let reader = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    for descriptor in [&reader, &writer] {
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
    }
    drop(writer);

    let mut poll = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    assert!(unsafe { libc::poll(&mut poll, 1, 1000) } > 0);
    let mut byte = 0u8;
    assert_eq!(
        unsafe { libc::read(reader.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) },
        0
    );
    assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
}
