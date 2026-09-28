use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use bionic_vm_facade::Provider;
use std::env;
use std::ffi::c_char;
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::{Arc, Barrier};

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_vm_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
}

struct Resolver;

impl SymbolResolver for Resolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        match request.version {
            Some(version)
                if version.soname == "libc.so"
                    && version.name == "LIBC"
                    && !version.hidden
                    && version.flags == 0 => {}
            _ => {
                return Err(ResolveError::Rejected(
                    "VM imports require public libc.so LIBC version".into(),
                ));
            }
        }
        let name = match request.symbol {
            "__errno" => c"__errno",
            "madvise" => c"madvise",
            "mincore" => c"mincore",
            "mmap" => c"mmap",
            "mmap64" => c"mmap64",
            "mprotect" => c"mprotect",
            "mremap" => c"mremap",
            "munmap" => c"munmap",
            _ => return Ok(None),
        };
        // SAFETY: both resolvers only read a static NUL-terminated name.
        let address = unsafe {
            aim_bionic_vm_resolve(name.as_ptr()).or_else(|| aim_bionic_errno_resolve(name.as_ptr()))
        }
        .map(|function| function as usize);
        Ok(address.map(|value| unsafe {
            ResolvedSymbol::new(NonZeroUsize::new(value).expect("function address"))
        }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    let provider = Arc::new(Provider::new()?);
    let _activation = provider.activate()?;
    if provider.page_size() < 4096 || !provider.page_size().is_power_of_two() {
        return Err("unexpected Darwin page size".into());
    }
    for rejected in [c"ashmem_create_region"] {
        // SAFETY: resolver reads only the static NUL-terminated name.
        if unsafe { aim_bionic_vm_resolve(rejected.as_ptr()) }.is_some() {
            return Err("unsupported VM symbol escaped closed resolver".into());
        }
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver;
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    // SAFETY: host errno is audit-only and never guest-resolved.
    unsafe { *__error() = 34_401 };
    let basic = image.call_exported_i32("bionic_vm_fixture_basic")?;
    if basic != 42 {
        return Err(format!("basic VM fixture returned {basic}").into());
    }
    if provider.mapping_count() != 0 || provider.capability_failed() {
        return Err("basic mapping ownership".into());
    }
    // SAFETY: same host pthread errno cell.
    if unsafe { *__error() } != 34_401 {
        return Err("VM facade changed host errno".into());
    }

    if image.call_exported_i32("bionic_vm_fixture_race_setup")? != 42
        || provider.mapping_count() != 1
    {
        return Err("race setup ownership".into());
    }
    let protect_address = image.lookup_exported("bionic_vm_fixture_race_protect")?;
    let unmap_address = image.lookup_exported("bionic_vm_fixture_race_unmap")?;
    let protect: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(protect_address) };
    let unmap: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(unmap_address) };
    let barrier = Arc::new(Barrier::new(9));
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for _ in 0..8 {
            let barrier = Arc::clone(&barrier);
            threads.push(scope.spawn(move || {
                // SAFETY: host errno is pthread-local and audit-only.
                unsafe { *__error() = 34_450 };
                barrier.wait();
                let result = unsafe { protect() } == 42;
                result && unsafe { *__error() == 34_450 }
            }));
        }
        barrier.wait();
        let unmapped = unsafe { unmap() } == 42;
        for thread in threads {
            assert!(thread.join().expect("VM worker"));
        }
        assert!(unmapped);
    });
    if provider.mapping_count() != 0 || provider.protection(0).is_some() {
        return Err("race mapping owner did not retire mapping".into());
    }
    if provider.capability_failed() {
        return Err("unexpected VM capability failure".into());
    }
    println!(
        "bionic-vm-facade: PASS page={} RW-RX-exec owner-race",
        provider.page_size()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-vm-facade: {error}");
            ExitCode::from(2)
        }
    }
}
