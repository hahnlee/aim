use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

#[repr(C)]
struct AndroidFile {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_wide_stdio_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn wide_stdio_elf_backend_install() -> i32;
    fn wide_stdio_elf_backend_input() -> *mut AndroidFile;
    fn wide_stdio_elf_backend_output() -> *mut AndroidFile;
    fn wide_stdio_elf_backend_verify() -> i32;
    fn wide_stdio_elf_backend_uninstall() -> i32;
}

#[derive(Default)]
struct ClosedResolver {
    requested: BTreeSet<String>,
}

impl SymbolResolver for ClosedResolver {
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
                    "wide stdio imports require public libc.so LIBC version".into(),
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
            // SAFETY: all strings remain live for the closed resolver call.
            let pointer = unsafe {
                aim_bionic_wide_stdio_resolve(c"libc.so".as_ptr(), name.as_ptr(), c"LIBC".as_ptr())
            };
            (!pointer.is_null()).then_some(pointer as usize)
        };
        Ok(address.map(|value| {
            // SAFETY: manifests pin the three Android arm64 function signatures.
            unsafe { ResolvedSymbol::new(NonZeroUsize::new(value).expect("provider address")) }
        }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments
        .next()
        .ok_or("missing Android wide stdio fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-wide-stdio-facade <android-arm64-fixture.so>".into());
    }
    // SAFETY: installs the process-scoped audit backend before any fixture call.
    if unsafe { wide_stdio_elf_backend_install() } != 0 {
        return Err("wide stdio backend installation failed".into());
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    let function_address = image.lookup_exported("bionic_wide_stdio_fixture_run")?;
    let function: unsafe extern "C" fn(*mut AndroidFile, *mut AndroidFile) -> i32 =
        unsafe { std::mem::transmute(function_address) };
    // SAFETY: audit backend returns two live 152-byte Android FILE tokens.
    let input = unsafe { wide_stdio_elf_backend_input() };
    let output = unsafe { wide_stdio_elf_backend_output() };
    if input.is_null() || output.is_null() {
        return Err("wide stdio backend did not publish tokens".into());
    }
    // SAFETY: current pthread's Darwin errno is used only as a preservation sentinel.
    unsafe { *__error() = 31_337 };
    // SAFETY: export signature and both argument tokens are pinned by the fixture gate.
    let result = unsafe { function(input, output) };
    if result != 42 {
        return Err(format!("Android wide stdio fixture failed at step {result}").into());
    }
    // SAFETY: checks the same pthread's host errno sentinel.
    if unsafe { *__error() } != 31_337 {
        return Err("wide stdio provider changed Darwin errno".into());
    }
    // SAFETY: value-only verification of output bytes and Android stream error.
    if unsafe { wide_stdio_elf_backend_verify() } != 0 {
        return Err("wide stdio backend verification failed".into());
    }
    let expected = ["__errno", "fputwc", "getwc", "ungetwc"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("wide stdio import drift: {:?}", resolver.requested).into());
    }
    // SAFETY: closes both tokens, forgets side-table state, then drains the backend.
    if unsafe { wide_stdio_elf_backend_uninstall() } != 0 {
        return Err("wide stdio backend teardown failed".into());
    }
    println!(
        "bionic-wide-stdio-facade: PASS AndroidELF=3+errno FILE=opaque152 UTF-8=Android-wchar32 host-errno=preserved"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-wide-stdio-facade: {error}");
            ExitCode::from(2)
        }
    }
}
