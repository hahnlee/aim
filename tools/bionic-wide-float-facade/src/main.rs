use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_wide_float_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_wide_float_capability(capability: *const c_char) -> i32;
    fn aim_bionic_wide_float_test_prepare_host_state();
    fn aim_bionic_wide_float_test_host_state_is_preserved() -> i32;
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
                    "wide float imports require public libc.so LIBC version".into(),
                ));
            }
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        let address = if request.symbol == "__errno" {
            // SAFETY: resolver reads only the live NUL-terminated name.
            unsafe { aim_bionic_errno_resolve(name.as_ptr()) }.map(|function| function as usize)
        } else {
            // SAFETY: all strings remain alive during this closed lookup.
            let pointer = unsafe {
                aim_bionic_wide_float_resolve(c"libc.so".as_ptr(), name.as_ptr(), c"LIBC".as_ptr())
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
    // SAFETY: fixed strings remain live during this closed lookup.
    Ok(unsafe {
        aim_bionic_wide_float_resolve(soname.as_ptr(), symbol.as_ptr(), version.as_ptr()) as usize
    })
}

fn capability(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let name = CString::new(name)?;
    // SAFETY: provider reads only the live NUL-terminated name.
    Ok(unsafe { aim_bionic_wide_float_capability(name.as_ptr()) } != 0)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for supported in ["wcstod", "wcstof"] {
        if resolve("libc.so", supported, "LIBC")? == 0 {
            return Err(format!("missing provider: {supported}").into());
        }
    }
    for (soname, symbol, version) in [
        ("libc.so", "wcstold", "LIBC"),
        ("libc.so", "wcstod_l", "LIBC"),
        ("libc.so", "wcstof_l", "LIBC"),
        ("libm.so", "wcstod", "LIBC"),
        ("libc.so", "wcstod", "GLIBC_2.17"),
    ] {
        if resolve(soname, symbol, version)? != 0 {
            return Err(format!("closed resolver escaped: {soname}:{symbol}@{version}").into());
        }
    }
    for supported in [
        "wcstod-binary64",
        "wcstof-binary32",
        "Android-unsigned-wchar32",
        "ICU76-iswspace",
        "AOSP-gdtoa",
        "Bionic-allowed-ASCII-span",
    ] {
        if !capability(supported)? {
            return Err(format!("missing capability: {supported}").into());
        }
    }
    if capability("Android-binary128-long-double")? {
        return Err("binary128 wcstold incorrectly advertised".into());
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    // SAFETY: helper changes only this pthread's audit-only Darwin state.
    unsafe { aim_bionic_wide_float_test_prepare_host_state() };
    let result = image.call_exported_i32("bionic_wide_float_fixture_basic")?;
    if result != 42 {
        return Err(format!("Android wide float fixture failed at step {result}").into());
    }
    // SAFETY: checks this same pthread's host state.
    if unsafe { aim_bionic_wide_float_test_host_state_is_preserved() } == 0 {
        return Err("wide float provider changed host errno/fenv".into());
    }

    let worker = image.lookup_exported("bionic_wide_float_fixture_thread")?;
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for thread in 0..8_u32 {
            workers.push(scope.spawn(move || {
                let function: unsafe extern "C" fn(u32) -> i32 =
                    unsafe { std::mem::transmute(worker) };
                // SAFETY: Darwin errno is pthread-local and audit-only.
                unsafe { *__error() = 33_100 + thread as i32 };
                for round in 0..500_u32 {
                    if unsafe { function(thread * 500 + round) } != 42 {
                        return false;
                    }
                }
                // SAFETY: same worker pthread's Darwin errno cell.
                unsafe { *__error() == 33_100 + thread as i32 }
            }));
        }
        for worker in workers {
            assert!(worker.join().expect("wide float worker"));
        }
    });
    let expected = ["__errno", "wcstod", "wcstof"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("wide float import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-wide-float-facade: PASS AndroidELF=2+errno ICU76-whitespace AOSP-gdtoa threads=8x500 host-errno+fenv=preserved wcstold=binary128-rejected"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-wide-float-facade: {error}");
            ExitCode::from(2)
        }
    }
}
