use darwin_art_elf_loader::{
    LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver,
};
use std::env;
use std::ffi::c_char;
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn darwin_art_bionic_numeric_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn darwin_art_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
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
                    "numeric imports require public libc.so LIBC version".into(),
                ));
            }
        }
        let name = match request.symbol {
            "__errno" => c"__errno",
            "strtol" => c"strtol",
            "strtoll" => c"strtoll",
            "strtoimax" => c"strtoimax",
            "strtoll_l" => c"strtoll_l",
            "strtoul" => c"strtoul",
            "strtoull" => c"strtoull",
            "strtoull_l" => c"strtoull_l",
            _ => return Ok(None),
        };
        // SAFETY: both closed resolvers only read a static NUL-terminated name.
        let address = unsafe {
            darwin_art_bionic_numeric_resolve(name.as_ptr())
                .or_else(|| darwin_art_bionic_errno_resolve(name.as_ptr()))
        }
        .map(|function| function as usize);
        Ok(address.map(|value| unsafe {
            ResolvedSymbol::new(NonZeroUsize::new(value).expect("function address"))
        }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for rejected in [
        c"strtod",
        c"strtof",
        c"strtold",
        c"strtol_l",
        c"strtoul_l",
        c"wcstoll",
        c"snprintf",
    ] {
        // SAFETY: resolver reads only the static NUL-terminated name.
        if unsafe { darwin_art_bionic_numeric_resolve(rejected.as_ptr()) }.is_some() {
            return Err("unsupported numeric symbol escaped closed resolver".into());
        }
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver;
    let image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    // SAFETY: host errno is audit-only and is never guest-resolved.
    unsafe { *__error() = 32_701 };
    let basic = image.call_exported_i32("bionic_numeric_fixture_basic")?;
    if basic != 42 {
        return Err(format!("Android numeric fixture boundary case {basic} failed").into());
    }
    // SAFETY: same current pthread's Darwin errno cell.
    if unsafe { *__error() } != 32_701 {
        return Err("numeric provider changed host errno".into());
    }

    let address = image.lookup_exported("bionic_numeric_fixture_thread")?;
    let thread_case: unsafe extern "C" fn(u32) -> i32 = unsafe { std::mem::transmute(address) };
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for thread in 0..8_u32 {
            workers.push(scope.spawn(move || {
                // SAFETY: Darwin host errno is pthread-local and audit-only.
                unsafe { *__error() = 32_750 + thread as i32 };
                for round in 0..1_000_u32 {
                    if unsafe { thread_case(thread * 1_000 + round) } != 42 {
                        return false;
                    }
                }
                // SAFETY: same worker pthread's Darwin errno cell.
                unsafe { *__error() == 32_750 + thread as i32 }
            }));
        }
        for worker in workers {
            assert!(worker.join().expect("numeric worker"));
        }
    });
    println!("bionic-numeric-facade: PASS integer=7@LIBC base=0,2..36 threads=8x1000");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-numeric-facade: {error}");
            ExitCode::from(2)
        }
    }
}
