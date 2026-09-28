use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use bionic_stdio_facade::Provider;
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::Arc;

unsafe extern "C" {
    fn aim_bionic_scanf_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_scanf_capability(name: *const c_char) -> *const c_char;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_stdio_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
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
            Some(v) if v.soname == "libc.so" && v.name == "LIBC" && !v.hidden && v.flags == 0 => {}
            _ => {
                return Err(ResolveError::Rejected(
                    "scanf imports require public libc.so@LIBC".into(),
                ));
            }
        }
        self.requested.insert(request.symbol.to_owned());
        let symbol =
            CString::new(request.symbol).map_err(|e| ResolveError::Rejected(e.to_string()))?;
        let address = if request.symbol == "__errno" {
            unsafe { aim_bionic_errno_resolve(symbol.as_ptr()) }.map(|f| f as usize)
        } else if matches!(request.symbol, "fopen" | "fclose") {
            unsafe { aim_bionic_stdio_resolve(symbol.as_ptr()) }.map(|f| f as usize)
        } else {
            let value = unsafe {
                aim_bionic_scanf_resolve(c"libc.so".as_ptr(), symbol.as_ptr(), c"LIBC".as_ptr())
            };
            (!value.is_null()).then_some(value as usize)
        };
        Ok(address.map(|value| unsafe {
            ResolvedSymbol::new(NonZeroUsize::new(value).expect("provider"))
        }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    let provider = Arc::new(Provider::new(
        vec![(
            b"/system/scan.txt".to_vec(),
            b"42 9000000000 tail 7".to_vec(),
        )],
        Vec::new(),
    )?);
    let _activation = provider.activate()?;
    for name in [
        "Android-AAPCS64-variadic",
        "Android-va_list32",
        "Bionic-string-reader",
        "Bionic-FILE-reader",
        "binary128-raw-output",
    ] {
        let name = CString::new(name)?;
        let status = unsafe { aim_bionic_scanf_capability(name.as_ptr()) };
        if status.is_null()
            || unsafe { std::ffi::CStr::from_ptr(status) }.to_bytes() != b"supported"
        {
            return Err("capability missing".into());
        }
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    let result = image.call_exported_i32("scanf_fixture")?;
    if result != 42 {
        return Err(format!("fixture failed at {result}").into());
    }
    let expected = ["__errno", "fclose", "fopen", "fscanf", "sscanf", "vsscanf"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-scanf-facade: PASS AndroidELF sscanf+vsscanf+fscanf FILE cursor GP/FP/stack binary128"
    );
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-scanf-facade: {error}");
            ExitCode::from(2)
        }
    }
}
