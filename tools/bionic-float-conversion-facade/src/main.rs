use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn aim_bionic_float_conversion_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_float_conversion_capability(capability: *const c_char) -> i32;
    fn aim_bionic_float_conversion_test_prepare_host_state();
    fn aim_bionic_float_conversion_test_host_state_is_preserved() -> i32;
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
        let expected_version = match request.symbol {
            "strtod_l" | "strtof_l" => "LIBC_O",
            _ => "LIBC",
        };
        let valid_version = request.version.is_some_and(|version| {
            version.soname == "libc.so"
                && version.name == expected_version
                && !version.hidden
                && version.flags == 0
        });
        if !valid_version {
            return Err(ResolveError::Rejected(format!(
                "float conversion version rejected: {}",
                request.symbol
            )));
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        let address = if request.symbol == "__errno" {
            // SAFETY: resolver reads the NUL-terminated name only.
            unsafe { aim_bionic_errno_resolve(name.as_ptr()) }.map(|function| function as usize)
        } else {
            // SAFETY: all strings remain alive during the fixed resolver call.
            let version = CString::new(expected_version)
                .map_err(|error| ResolveError::Rejected(error.to_string()))?;
            let pointer = unsafe {
                aim_bionic_float_conversion_resolve(
                    c"libc.so".as_ptr(),
                    name.as_ptr(),
                    version.as_ptr(),
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
    // SAFETY: provider reads the NUL-terminated capability only.
    Ok(unsafe { aim_bionic_float_conversion_capability(name.as_ptr()) } != 0)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for rejected in ["strtold", "strtold_l", "strtod_l", "strtof_l"] {
        let name = CString::new(rejected)?;
        // SAFETY: fixed strings remain alive during the resolver call.
        let address = unsafe {
            aim_bionic_float_conversion_resolve(
                c"libc.so".as_ptr(),
                name.as_ptr(),
                c"LIBC".as_ptr(),
            )
        };
        if !address.is_null() {
            return Err(format!("unsupported conversion escaped: {rejected}").into());
        }
    }
    for supported in ["strtod_l", "strtof_l"] {
        let name = CString::new(supported)?;
        // SAFETY: fixed strings remain alive during the resolver call.
        let address = unsafe {
            aim_bionic_float_conversion_resolve(
                c"libc.so".as_ptr(),
                name.as_ptr(),
                c"LIBC_O".as_ptr(),
            )
        };
        if address.is_null() {
            return Err(format!("missing locale wrapper: {supported}").into());
        }
    }
    for supported in [
        "strtod-binary64",
        "strtof-binary32",
        "AOSP-gdtoa",
        "C-locale-only",
        "locale-argument-ignored",
    ] {
        if !capability(supported)? {
            return Err(format!("missing capability: {supported}").into());
        }
    }
    if capability("Android-binary128-long-double")? {
        return Err("binary128 long double incorrectly advertised".into());
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    // SAFETY: helper changes only this pthread's host errno/fenv.
    unsafe { aim_bionic_float_conversion_test_prepare_host_state() };
    let result = image.call_exported_i32("bionic_float_fixture_basic")?;
    if result != 42 {
        return Err(format!("Android float fixture failed at step {result}").into());
    }
    // SAFETY: checks this same pthread's host state.
    if unsafe { aim_bionic_float_conversion_test_host_state_is_preserved() } == 0 {
        return Err("provider changed host errno/fenv".into());
    }

    let worker = image.lookup_exported("bionic_float_fixture_thread")?;
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for index in 0..8_u32 {
            workers.push(scope.spawn(move || {
                let function: unsafe extern "C" fn(u32) -> i32 =
                    unsafe { std::mem::transmute(worker) };
                for round in 0..500_u32 {
                    if unsafe { function(index * 500 + round) } != 42 {
                        return false;
                    }
                }
                true
            }));
        }
        for worker in workers {
            assert!(worker.join().expect("float worker"));
        }
    });
    let expected = ["__errno", "strtod", "strtod_l", "strtof", "strtof_l"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("float import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-float-conversion-facade: PASS AndroidELF=4+errno AOSP-gdtoa binary32+binary64 locale-ignored threads=8x500 host-errno+fenv=preserved long-double=rejected"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-float-conversion-facade: {error}");
            ExitCode::from(2)
        }
    }
}
