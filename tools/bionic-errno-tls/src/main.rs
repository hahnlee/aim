use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::env;
use std::ffi::c_char;
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::{Arc, Barrier};
use std::thread;

unsafe extern "C" {
    fn aim_bionic___errno() -> *mut i32;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_errno_fixture_prepare(value: i32, host_errno_value: i32);
    fn aim_errno_fixture_thread_value() -> i32;
    fn aim_errno_fixture_host_errno_is(expected: i32) -> i32;
    fn aim_errno_fixture_cells_are_disjoint() -> i32;
}

struct ClosedResolver;

impl ClosedResolver {
    fn symbol(address: usize) -> ResolvedSymbol {
        let address = NonZeroUsize::new(address).expect("function address is non-zero");
        // SAFETY: each accepted address below has a fixed register-only signature matching
        // its Android declaration and remains linked for every loaded image's lifetime.
        unsafe { ResolvedSymbol::new(address) }
    }
}

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "errno fixture imports must be unversioned".to_owned(),
            ));
        }
        match request.symbol {
            "__errno" => {
                // SAFETY: the byte string is statically NUL-terminated.
                let address = unsafe { aim_bionic_errno_resolve(c"__errno".as_ptr()) }
                    .ok_or_else(|| ResolveError::Rejected("missing __errno provider".to_owned()))?;
                Ok(Some(Self::symbol(address as usize)))
            }
            "aim_errno_fixture_thread_value" => {
                Ok(Some(Self::symbol(aim_errno_fixture_thread_value as usize)))
            }
            _ => Ok(None),
        }
    }
}

#[derive(Debug)]
struct ThreadOutcome {
    wanted: i32,
    observed: i32,
    host_unchanged: bool,
    host_disjoint: bool,
    bionic_address: usize,
}

fn run_thread(
    bytes: Arc<Vec<u8>>,
    barrier: Arc<Barrier>,
    wanted: i32,
    host_sentinel: i32,
) -> Result<ThreadOutcome, String> {
    let mut resolver = ClosedResolver;
    let mut image =
        LoadedElf::load_with_resolver(&bytes, &mut resolver).map_err(|error| error.to_string())?;
    image
        .run_initializers()
        .map_err(|error| error.to_string())?;
    barrier.wait();
    // SAFETY: these helpers touch only this pthread's fixture input and host errno.
    unsafe { aim_errno_fixture_prepare(wanted, host_sentinel) };
    let observed = image
        .call_exported_i32("bionic_errno_fixture_run")
        .map_err(|error| error.to_string())?;
    // SAFETY: all three addresses/values belong to the current pthread and are sampled
    // while both test pthreads are alive.
    let (bionic_address, host_unchanged, host_disjoint) = unsafe {
        (
            aim_bionic___errno() as usize,
            aim_errno_fixture_host_errno_is(host_sentinel) == 1,
            aim_errno_fixture_cells_are_disjoint() == 1,
        )
    };
    barrier.wait();
    Ok(ThreadOutcome {
        wanted,
        observed,
        host_unchanged,
        host_disjoint,
        bionic_address,
    })
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments.next().ok_or("missing Android errno fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-errno-tls <android-arm64-fixture.so>".into());
    }
    // SAFETY: reads the main pthread's private Bionic cell before workers exist.
    if unsafe { *aim_bionic___errno() } != 0 {
        return Err("main pthread Bionic errno did not start at zero".into());
    }

    let bytes = Arc::new(fs::read(fixture)?);
    let barrier = Arc::new(Barrier::new(2));
    let handles = [(111, 31001), (222, 32002)].map(|(wanted, host_sentinel)| {
        let bytes = Arc::clone(&bytes);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || run_thread(bytes, barrier, wanted, host_sentinel))
    });
    let [first, second] = handles.map(|handle| {
        handle
            .join()
            .map_err(|_| "errno fixture pthread panicked".to_owned())?
    });
    let first = first.map_err(|error| format!("first pthread: {error}"))?;
    let second = second.map_err(|error| format!("second pthread: {error}"))?;

    for outcome in [&first, &second] {
        if outcome.observed != outcome.wanted {
            return Err(format!("Android ELF errno mismatch: {outcome:?}").into());
        }
        if !outcome.host_unchanged {
            return Err(format!("host errno changed: {outcome:?}").into());
        }
        if outcome.bionic_address == 0 || !outcome.host_disjoint {
            return Err(format!("Bionic cell aliases host errno: {outcome:?}").into());
        }
    }
    if first.bionic_address == second.bionic_address {
        return Err("different pthreads received the same Bionic errno cell".into());
    }
    // SAFETY: worker writes must not affect the main pthread cell.
    if unsafe { *aim_bionic___errno() } != 0 {
        return Err("worker write escaped into main pthread Bionic errno".into());
    }
    println!(
        "bionic-errno-tls: PASS Android ELF __errno import cells={:#x}!={:#x} host-isolated",
        first.bionic_address, second.bionic_address
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-errno-tls: {error}");
            ExitCode::from(2)
        }
    }
}
