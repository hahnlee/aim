use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_wide_integer_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_wide_integer_capability(capability: *const c_char) -> i32;
    fn aim_bionic_wide_integer_test_prepare_host_state();
    fn aim_bionic_wide_integer_test_host_state_is_preserved() -> i32;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
}

#[derive(Default)]
struct Resolver {
    requested: BTreeSet<String>,
}

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
                    "wide integer imports require public libc.so LIBC version".into(),
                ));
            }
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        let address = if request.symbol == "__errno" {
            // SAFETY: the errno resolver reads only the live NUL-terminated name.
            unsafe { aim_bionic_errno_resolve(name.as_ptr()) }.map(|function| function as usize)
        } else {
            // SAFETY: all three strings remain live for this closed resolver call.
            let pointer = unsafe {
                aim_bionic_wide_integer_resolve(
                    c"libc.so".as_ptr(),
                    name.as_ptr(),
                    c"LIBC".as_ptr(),
                )
            };
            (!pointer.is_null()).then_some(pointer as usize)
        };
        Ok(address.map(|value| unsafe {
            ResolvedSymbol::new(NonZeroUsize::new(value).expect("provider address"))
        }))
    }
}

fn capability(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let name = CString::new(name)?;
    // SAFETY: the provider reads only the live NUL-terminated capability.
    Ok(unsafe { aim_bionic_wide_integer_capability(name.as_ptr()) } != 0)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for rejected in [
        "wcstod",
        "wcstof",
        "wcstold",
        "wcstol_l",
        "wcstoll_l",
        "wcstoul_l",
        "wcstoull_l",
        "strtol",
    ] {
        let name = CString::new(rejected)?;
        // SAFETY: fixed strings remain live during the closed resolver call.
        let address = unsafe {
            aim_bionic_wide_integer_resolve(c"libc.so".as_ptr(), name.as_ptr(), c"LIBC".as_ptr())
        };
        if !address.is_null() {
            return Err(format!("unsupported conversion escaped: {rejected}").into());
        }
    }
    for supported in ["Android-wchar32", "AOSP-wide-integer", "base-0,2..36"] {
        if !capability(supported)? {
            return Err(format!("missing capability: {supported}").into());
        }
    }
    if capability("wide-floating")? || capability("Android-binary128-long-double")? {
        return Err("wide floating conversion incorrectly advertised".into());
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    // SAFETY: helper changes only this pthread's audit-only Darwin state.
    unsafe { aim_bionic_wide_integer_test_prepare_host_state() };
    let result = image.call_exported_i32("bionic_wide_integer_fixture_basic")?;
    if result != 42 {
        return Err(format!("Android wide integer fixture failed at step {result}").into());
    }
    // SAFETY: checks the same pthread's audit-only Darwin state.
    if unsafe { aim_bionic_wide_integer_test_host_state_is_preserved() } == 0 {
        return Err("wide integer provider changed host errno/fenv".into());
    }

    let worker = image.lookup_exported("bionic_wide_integer_fixture_thread")?;
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for thread in 0..8_u32 {
            workers.push(scope.spawn(move || {
                let function: unsafe extern "C" fn(u32) -> i32 =
                    unsafe { std::mem::transmute(worker) };
                // SAFETY: Darwin errno is pthread-local and audit-only.
                unsafe { *__error() = 33_000 + thread as i32 };
                for round in 0..1_000_u32 {
                    if unsafe { function(thread * 1_000 + round) } != 42 {
                        return false;
                    }
                }
                // SAFETY: same worker pthread's Darwin errno cell.
                unsafe { *__error() == 33_000 + thread as i32 }
            }));
        }
        for worker in workers {
            assert!(worker.join().expect("wide integer worker"));
        }
    });
    let expected = ["__errno", "wcstol", "wcstoll", "wcstoul", "wcstoull"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("wide integer import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-wide-integer-facade: PASS AndroidELF=4+errno AOSP-wchar32 base=0,2..36 threads=8x1000 host-errno+fenv=preserved wide-float=separate"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-wide-integer-facade: {error}");
            ExitCode::from(2)
        }
    }
}
