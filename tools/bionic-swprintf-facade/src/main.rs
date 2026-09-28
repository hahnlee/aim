use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::env;
use std::ffi::{CString, c_char};
use std::fs;
use std::num::NonZeroUsize;

unsafe extern "C" {
    fn aim_bionic_swprintf_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> Option<unsafe extern "C" fn()>;
}

struct Resolver {
    requests: usize,
}

impl SymbolResolver for Resolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        self.requests += 1;
        let version = request.version.ok_or_else(|| {
            ResolveError::Rejected(format!("unversioned import: {}", request.symbol))
        })?;
        let soname = CString::new(version.soname).unwrap();
        let symbol = CString::new(request.symbol).unwrap();
        let name = CString::new(version.name).unwrap();
        // SAFETY: provider resolver returns process-lifetime fixed entrypoints.
        let address =
            unsafe { aim_bionic_swprintf_resolve(soname.as_ptr(), symbol.as_ptr(), name.as_ptr()) };
        Ok(address.and_then(|function| {
            NonZeroUsize::new(function as usize).map(|pointer| {
                // SAFETY: exact fixture import signature is checked by the build gate.
                unsafe { ResolvedSymbol::new(pointer) }
            })
        }))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os().nth(1).ok_or("usage: runner FIXTURE.so")?;
    let bytes = fs::read(path)?;
    let mut resolver = Resolver { requests: 0 };
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    let result = image.call_exported_i32("swprintf_fixture")?;
    if result != 0 {
        return Err(format!("Android swprintf fixture failed at {result}").into());
    }
    if resolver.requests != 1 {
        return Err(format!("unexpected import count: {}", resolver.requests).into());
    }
    let bad = CString::new("unknown").unwrap();
    let libc = CString::new("libc.so").unwrap();
    let version = CString::new("LIBC").unwrap();
    // SAFETY: constant NUL-terminated strings.
    if unsafe { aim_bionic_swprintf_resolve(libc.as_ptr(), bad.as_ptr(), version.as_ptr()) }
        .is_some()
    {
        return Err("closed resolver escaped".into());
    }
    println!("bionic-swprintf-facade: PASS AndroidELF=%f+%Lf binary64+binary128");
    Ok(())
}
