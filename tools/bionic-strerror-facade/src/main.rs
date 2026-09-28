use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::env;
use std::ffi::{c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_strerror_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
}

struct ClosedResolver;

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "strerror fixture imports must be unversioned".to_owned(),
            ));
        }
        let name = match request.symbol {
            "__errno" => c"__errno",
            "strerror_r" => c"strerror_r",
            _ => return Ok(None),
        };
        // SAFETY: both resolvers read one static NUL-terminated name and return
        // only manifest-audited functions linked for the image lifetime.
        let address = unsafe {
            aim_bionic_strerror_resolve(name.as_ptr())
                .or_else(|| aim_bionic_errno_resolve(name.as_ptr()))
        };
        Ok(address.map(|address| {
            let address = NonZeroUsize::new(address as usize).expect("function address");
            // SAFETY: the manifest and Android compile probe pin the import ABI.
            unsafe { ResolvedSymbol::new(address) }
        }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments.next().ok_or("missing Android strerror fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-strerror-facade FIXTURE.so".into());
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver;
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    // SAFETY: host-only audit cell; it is never exposed through the resolver.
    unsafe { *__error() = 33_005 };
    let result = image.call_exported_i32("bionic_strerror_fixture_run")?;
    if result != 42 {
        return Err(format!("Android strerror fixture failed at step {result}").into());
    }
    // SAFETY: same pthread-local host errno cell set immediately above.
    if unsafe { *__error() } != 33_005 {
        return Err("strerror provider changed Darwin errno".into());
    }
    println!("bionic-strerror-facade: PASS Android ELF XSI strerror_r");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-strerror-facade: {error}");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
fn _function_pointer_is_data_pointer_sized() {
    const _: () = assert!(size_of::<unsafe extern "C" fn()>() == size_of::<*mut c_void>());
}
