use bionic_process_state_facade::{AuxSnapshot, Snapshot, capability_failed, drop_count};
use darwin_art_elf_loader::{
    LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver,
};
use std::env;
use std::ffi::{CStr, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::Arc;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn mprotect(address: *mut c_void, length: usize, protection: i32) -> i32;
    fn munmap(address: *mut c_void, length: usize) -> i32;
    fn darwin_art_bionic_getrlimit(resource: i32, limit: *mut c_void) -> i32;
    fn darwin_art_bionic_errno_load() -> i32;
    fn darwin_art_bionic_process_state_resolve(
        name: *const c_char,
    ) -> Option<unsafe extern "C" fn()>;
    fn darwin_art_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn darwin_art_bionic___system_property_find(name: *const c_char) -> *const c_void;
    fn darwin_art_bionic_android_get_device_api_level() -> i32;
    fn darwin_art_bionic___system_property_read_callback(
        property: *const c_void,
        callback: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, u32),
        cookie: *mut c_void,
    );
}

#[derive(Default)]
struct PropertyCapture {
    name: Vec<u8>,
    value: Vec<u8>,
    serial: u32,
}

unsafe extern "C" fn capture_property(
    cookie: *mut c_void,
    name: *const c_char,
    value: *const c_char,
    serial: u32,
) {
    // SAFETY: this callback is invoked synchronously with the local capture
    // cookie and snapshot-owned NUL-terminated strings.
    let capture = unsafe { &mut *cookie.cast::<PropertyCapture>() };
    capture.name = unsafe { CStr::from_ptr(name) }.to_bytes().to_vec();
    capture.value = unsafe { CStr::from_ptr(value) }.to_bytes().to_vec();
    capture.serial = serial;
}

struct ClosedResolver;

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "process-state imports must be unversioned".to_owned(),
            ));
        }
        let name = match request.symbol {
            "__errno" => c"__errno",
            "__system_property_get" => c"__system_property_get",
            "__system_property_read" => c"__system_property_read",
            "basename" => c"basename",
            "getauxval" => c"getauxval",
            "getentropy" => c"getentropy",
            "getenv" => c"getenv",
            "raise" => c"raise",
            "sigaction" => c"sigaction",
            "signal" => c"signal",
            _ => return Ok(None),
        };
        // SAFETY: providers only read the static NUL-terminated name.
        let address = unsafe {
            darwin_art_bionic_process_state_resolve(name.as_ptr())
                .or_else(|| darwin_art_bionic_errno_resolve(name.as_ptr()))
        };
        Ok(address.map(|function| {
            let address = NonZeroUsize::new(function as usize).unwrap();
            // SAFETY: closed manifest pins the fixed-register Android arm64 ABI.
            unsafe { ResolvedSymbol::new(address) }
        }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments
        .next()
        .ok_or("missing Android process-state fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-process-state-facade <fixture.so>".into());
    }
    let boundary = vec![b'X'; 91];
    let snapshot = Arc::new(Snapshot::new(
        vec![
            (b"ANDROID_ROOT".to_vec(), b"/system".to_vec()),
            (b"LANG".to_vec(), b"en-US".to_vec()),
        ],
        vec![
            (b"ro.build.version.sdk".to_vec(), b"36".to_vec()),
            (b"ro.product.cpu.abi".to_vec(), b"arm64-v8a".to_vec()),
            (b"test.boundary".to_vec(), boundary),
        ],
        AuxSnapshot {
            page_size: 16_384,
            hwcap: 3,
            hwcap2: 0,
            secure: false,
            random: [
                0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xa9, 0xba, 0xcb, 0xdc, 0xed,
                0xfe, 0x0f,
            ],
        },
    )?);
    let activation = snapshot.activate()?;

    const PAGE_SIZE: usize = 16_384;
    // Chromium verifies protected memory by asking getrlimit() to copy its
    // result into a read-only page. Linux returns EFAULT for that kernel
    // copy-to-user; the compatibility syscall must not fault the host process.
    let probe_page = unsafe { mmap(std::ptr::null_mut(), PAGE_SIZE, 3, 0x1002, -1, 0) };
    if probe_page as isize == -1 {
        return Err("protected getrlimit probe mmap failed".into());
    }
    if unsafe { darwin_art_bionic_getrlimit(6, probe_page) } != 0 {
        return Err("writable getrlimit copy-out failed".into());
    }
    if unsafe { mprotect(probe_page, PAGE_SIZE, 1) } != 0 {
        return Err("protected getrlimit probe mprotect failed".into());
    }
    if unsafe { darwin_art_bionic_getrlimit(6, probe_page) } != -1
        || unsafe { darwin_art_bionic_errno_load() } != 14
    {
        return Err("read-only getrlimit copy-out did not return EFAULT".into());
    }
    if unsafe { munmap(probe_page, PAGE_SIZE) } != 0 {
        return Err("protected getrlimit probe munmap failed".into());
    }
    let mut property = PropertyCapture::default();
    // SAFETY: the active snapshot and local callback cookie outlive the
    // synchronous property lookup and callback.
    unsafe {
        let token = darwin_art_bionic___system_property_find(c"ro.build.version.sdk".as_ptr());
        if token.is_null() {
            return Err("property token lookup failed".into());
        }
        darwin_art_bionic___system_property_read_callback(
            token,
            capture_property,
            (&mut property as *mut PropertyCapture).cast(),
        );
    }
    if property.name != b"ro.build.version.sdk" || property.value != b"36" {
        return Err("property callback snapshot mismatch".into());
    }
    // SAFETY: the original C getter reads the installed Rust property area.
    if unsafe { darwin_art_bionic_android_get_device_api_level() } != 36 {
        return Err("device API getter disagrees with property snapshot".into());
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver;
    let image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    // SAFETY: host errno is audit-only and never guest-resolved.
    unsafe { *__error() = 33_201 };
    if image.call_exported_i32("bionic_process_fixture_basic")? != 42 {
        return Err("basic Android process-state fixture failed".into());
    }
    // SAFETY: same current pthread errno cell.
    if unsafe { *__error() } != 33_201 || capability_failed() {
        return Err("basic lookup leaked host errno or capability failure".into());
    }

    let concurrent_address = image.lookup_exported("bionic_process_fixture_concurrent")?;
    // SAFETY: loader validated an executable no-argument/i32 AArch64 function.
    let concurrent: unsafe extern "C" fn() -> i32 =
        unsafe { std::mem::transmute(concurrent_address) };
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for _ in 0..8 {
            threads.push(scope.spawn(move || {
                // SAFETY: Darwin host errno is pthread-local and audit-only.
                unsafe { *__error() = 33_250 };
                for _ in 0..1_000 {
                    // SAFETY: image and active immutable snapshot outlive this scope.
                    if unsafe { concurrent() } != 42 {
                        return false;
                    }
                }
                // SAFETY: same worker pthread errno cell set before guest calls.
                unsafe { *__error() == 33_250 }
            }));
        }
        for thread in threads {
            assert!(thread.join().unwrap(), "concurrent Android lookup failed");
        }
    });
    if image.call_exported_i32("bionic_process_fixture_verify_pointers")? != 42 {
        return Err("facade-lifetime pointer stability failed".into());
    }

    if image.call_exported_i32("bionic_process_fixture_signal_legacy")? != 42 {
        return Err("legacy signal trampoline dispatch failed".into());
    }
    if image.call_exported_i32("bionic_process_fixture_sigaction_query")? != 42 {
        return Err("sigaction guest flags/query failed".into());
    }
    if image.call_exported_i32("bionic_process_fixture_sigaction_context_recovery")? != 42 {
        return Err("sigaction guest ucontext recovery failed".into());
    }

    drop(activation);
    drop(snapshot);
    if drop_count() != 1 {
        return Err("snapshot backing allocations did not tear down exactly once".into());
    }
    // SAFETY: post-teardown call is expected to fail closed without dereferencing old pointers.
    unsafe { *__error() = 33_202 };
    if image.call_exported_i32("bionic_process_fixture_after_teardown")? != 42
        || unsafe { *__error() } != 33_202
        || !capability_failed()
    {
        return Err("post-teardown Android calls did not fail closed".into());
    }
    println!(
        "bionic-process-state-facade: PASS immutable snapshot concurrent teardown signal-trampoline ucontext-recovery"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-process-state-facade: {error}");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
fn _function_pointer_is_data_pointer_sized() {
    const _: () = assert!(size_of::<unsafe extern "C" fn()>() == size_of::<*mut c_void>());
}
