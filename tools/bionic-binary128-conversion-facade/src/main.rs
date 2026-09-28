use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_binary128_conversion_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_binary128_conversion_capability(capability: *const c_char) -> i32;
    fn aim_bionic_binary128_conversion_test_prepare_host_state();
    fn aim_bionic_binary128_conversion_test_host_state_is_preserved() -> i32;
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
                    "binary128 imports require public libc.so LIBC version".into(),
                ));
            }
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        let address = if request.symbol == "__errno" {
            unsafe { aim_bionic_errno_resolve(name.as_ptr()) }.map(|function| function as usize)
        } else {
            let pointer = unsafe {
                aim_bionic_binary128_conversion_resolve(
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

fn resolve(soname: &str, symbol: &str, version: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let soname = CString::new(soname)?;
    let symbol = CString::new(symbol)?;
    let version = CString::new(version)?;
    Ok(unsafe {
        aim_bionic_binary128_conversion_resolve(soname.as_ptr(), symbol.as_ptr(), version.as_ptr())
            as usize
    })
}

fn capability(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let name = CString::new(name)?;
    Ok(unsafe { aim_bionic_binary128_conversion_capability(name.as_ptr()) } != 0)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for supported in ["strtold", "strtold_l", "wcstold"] {
        if resolve("libc.so", supported, "LIBC")? == 0 {
            return Err(format!("missing provider: {supported}").into());
        }
    }
    for (soname, symbol, version) in [
        ("libm.so", "strtold", "LIBC"),
        ("libc.so", "strtold", "GLIBC_2.17"),
        ("libc.so", "strtod", "LIBC"),
        ("libc.so", "wcstold_l", "LIBC"),
    ] {
        if resolve(soname, symbol, version)? != 0 {
            return Err(format!("closed resolver escaped: {soname}:{symbol}@{version}").into());
        }
    }
    for required in [
        "Android-AAPCS64-binary128-q0",
        "AOSP-gdtoa-strtorQ",
        "strtold_l-locale-ignored",
        "Android-unsigned-wchar32",
        "ICU76-iswspace",
        "conversion-only-no-binary128-arithmetic",
    ] {
        if !capability(required)? {
            return Err(format!("missing capability: {required}").into());
        }
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    unsafe { aim_bionic_binary128_conversion_test_prepare_host_state() };
    let result = image.call_exported_i32("binary128_fixture_basic")?;
    if result != 42 {
        return Err(format!("Android binary128 fixture failed at step {result}").into());
    }
    if unsafe { aim_bionic_binary128_conversion_test_host_state_is_preserved() } == 0 {
        return Err("binary128 provider changed host errno/fenv".into());
    }

    let worker = image.lookup_exported("binary128_fixture_thread")?;
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for thread in 0..8_u32 {
            workers.push(scope.spawn(move || {
                let function: unsafe extern "C" fn(u32) -> i32 =
                    unsafe { std::mem::transmute(worker) };
                unsafe { *__error() = 34_000 + thread as i32 };
                for round in 0..500_u32 {
                    if unsafe { function(thread * 500 + round) } != 42 {
                        return false;
                    }
                }
                unsafe { *__error() == 34_000 + thread as i32 }
            }));
        }
        for worker in workers {
            assert!(worker.join().expect("binary128 worker"));
        }
    });
    let expected = ["__errno", "strtold", "strtold_l", "wcstold"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("binary128 import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-binary128-conversion-facade: PASS AndroidELF=3+errno raw-binary128=q0 bits=zero+subnormal+normal+max+overflow+inf+nan ICU76-wchar32 threads=8x500 host-errno+fenv=preserved"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-binary128-conversion-facade: {error}");
            ExitCode::from(2)
        }
    }
}
