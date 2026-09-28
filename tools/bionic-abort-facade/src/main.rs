use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn aim_bionic_abort_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_abort_capability(capability: *const c_char) -> i32;
    fn aim_abort_probe_death(function_address: usize, mode: i32) -> i32;
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
                    "abort imports require public libc.so LIBC version".into(),
                ));
            }
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        // SAFETY: all strings remain live for this closed resolver call.
        let pointer = unsafe {
            aim_bionic_abort_resolve(c"libc.so".as_ptr(), name.as_ptr(), c"LIBC".as_ptr())
        };
        Ok((!pointer.is_null()).then(|| unsafe {
            ResolvedSymbol::new(NonZeroUsize::new(pointer as usize).expect("provider address"))
        }))
    }
}

fn capability(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let name = CString::new(name)?;
    // SAFETY: provider reads only the live NUL-terminated capability.
    Ok(unsafe { aim_bionic_abort_capability(name.as_ptr()) } != 0)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for supported in [
        "SIGABRT=6-raw-forwarding",
        "abort-message-first-wins",
        "abort-message-process-lifetime",
        "abort-message-magic-layout",
    ] {
        if !capability(supported)? {
            return Err(format!("missing capability: {supported}").into());
        }
    }
    if capability("abort-message-vma-name")? || capability("Linux-prctl")? {
        return Err("Darwin prctl/VMA naming gap was incorrectly advertised".into());
    }
    for rejected in ["raise", "tgkill", "prctl", "exit", "_exit"] {
        let name = CString::new(rejected)?;
        // SAFETY: fixed strings remain live during the closed resolver call.
        let pointer = unsafe {
            aim_bionic_abort_resolve(c"libc.so".as_ptr(), name.as_ptr(), c"LIBC".as_ptr())
        };
        if !pointer.is_null() {
            return Err(format!("unsupported symbol escaped: {rejected}").into());
        }
    }

    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    if image.call_exported_i32("bionic_abort_fixture_message")? != 42 {
        return Err("Android abort-message fixture failed".into());
    }
    let abort_function = image.lookup_exported("bionic_abort_fixture_abort")?;
    for mode in 0..4 {
        // SAFETY: helper forks; only the child calls the verified void export.
        if unsafe { aim_abort_probe_death(abort_function, mode) } == 0 {
            return Err(format!("Android abort death mode {mode} failed").into());
        }
    }
    let expected = ["abort", "android_set_abort_message"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("abort import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-abort-facade: PASS AndroidELF=2@LIBC message=first-wins+magic death=default+blocked+ignored+returning SIGABRT=6 prctl-vma-name=gap"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-abort-facade: {error}");
            ExitCode::from(2)
        }
    }
}
