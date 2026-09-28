use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use bionic_stdio_facade::Provider;
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::Arc;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_formatted_stdio_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_formatted_stdio_capability(name: *const c_char) -> *const c_char;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_fopen(path: *const c_char, mode: *const c_char) -> *mut c_void;
    fn aim_bionic_fclose(file: *mut c_void) -> i32;
    fn aim_bionic_fwrite(
        bytes: *const c_void,
        size: usize,
        count: usize,
        file: *mut c_void,
    ) -> usize;
    fn aim_bionic_ftello(file: *mut c_void) -> i64;
    static mut aim_bionic___sF: [u8; 456];
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
        if request.needed_libraries != ["libc.so"] {
            return Err(ResolveError::Rejected(format!(
                "closed graph requires only libc.so: {:?}",
                request.needed_libraries
            )));
        }
        match request.version {
            Some(version)
                if version.soname == "libc.so"
                    && version.name == "LIBC"
                    && !version.hidden
                    && version.flags == 0 => {}
            _ => {
                return Err(ResolveError::Rejected(
                    "formatted stdio requires public libc.so@LIBC".into(),
                ));
            }
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        let address = if request.symbol == "__errno" {
            // SAFETY: resolver reads the live NUL-terminated name only.
            unsafe { aim_bionic_errno_resolve(name.as_ptr()) }.map(|function| function as usize)
        } else {
            // SAFETY: fixed SONAME/version and live symbol string.
            let pointer = unsafe {
                aim_bionic_formatted_stdio_resolve(
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
    // SAFETY: provider returns a process-lifetime static string.
    let value = unsafe { aim_bionic_formatted_stdio_capability(name.as_ptr()) };
    if value.is_null() {
        return Ok(false);
    }
    // SAFETY: provider returns a process-lifetime NUL-terminated static string.
    Ok(unsafe { CStr::from_ptr(value) } == c"supported")
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("Android fixture path")?;
    for name in [
        "Android-AAPCS64-fprintf",
        "Android-va_list-vfprintf",
        "provider-local-FILE",
        "precommit-semantic-failure-atomic",
    ] {
        if !capability(name)? {
            return Err(format!("missing capability: {name}").into());
        }
    }

    let provider = Arc::new(Provider::new(Vec::new(), Vec::new())?);
    let _activation = provider.activate()?;
    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    let call = |name: &str, file: *mut c_void| -> Result<i32, Box<dyn std::error::Error>> {
        let address = image.lookup_exported(name)?;
        let function: unsafe extern "C" fn(*mut c_void) -> i32 =
            unsafe { std::mem::transmute(address) };
        // SAFETY: every fixture export has the audited one-pointer signature.
        Ok(unsafe { function(file) })
    };
    let stdout = unsafe {
        std::ptr::addr_of_mut!(aim_bionic___sF)
            .cast::<u8>()
            .add(152)
            .cast::<c_void>()
    };
    // SAFETY: host errno is audit-only and must survive every facade call.
    unsafe { *__error() = 33_701 };

    let first = b"F:1,2,3,4,5,6,7,8|0.5,1.5,2.5,3.5,4.5,5.5,6.5,7.5,8.5,9.5\n";
    if call("bionic_formatted_stdio_fixture_fprintf", stdout)? != first.len() as i32 {
        return Err("fprintf result".into());
    }
    let second = b"V:-1,-2,-3,-4,-5,-6,-7,-8|10.5,11.5,12.5,13.5,14.5,15.5,16.5,17.5,18.5,19.5\n";
    if call("bionic_formatted_stdio_fixture_vfprintf", stdout)? != second.len() as i32 {
        return Err("vfprintf result".into());
    }
    let mut expected = first.to_vec();
    expected.extend_from_slice(second);
    if provider.stdout_bytes() != expected {
        return Err("formatted output/PCS mismatch".into());
    }
    if call("bionic_formatted_stdio_fixture_rejected", stdout)? != 42
        || provider.stdout_bytes() != expected
    {
        return Err("bounded/rejected format mutated stream".into());
    }
    // SAFETY: the active provider copies both live NUL-terminated strings.
    let capacity_file = unsafe { aim_bionic_fopen(c"/capacity".as_ptr(), c"w".as_ptr()) };
    if capacity_file.is_null() {
        return Err("capacity stream open".into());
    }
    let fill = vec![b'x'; 16 * 1024 * 1024 - 512 * 1024];
    // SAFETY: fill remains readable and capacity_file remains provider-owned.
    let filled = unsafe { aim_bionic_fwrite(fill.as_ptr().cast(), 1, fill.len(), capacity_file) };
    // SAFETY: capacity_file is live until the checked close below.
    let before = unsafe { aim_bionic_ftello(capacity_file) };
    if filled != fill.len()
        || before != fill.len() as i64
        || call("bionic_formatted_stdio_fixture_capacity", capacity_file)? != 42
        || unsafe { aim_bionic_ftello(capacity_file) } != before
    {
        return Err("stdio capacity failure was not atomic EFBIG".into());
    }
    // SAFETY: capacity_file has not been closed yet.
    if unsafe { aim_bionic_fclose(capacity_file) } != 0 {
        return Err("capacity stream close".into());
    }
    let mut foreign = [0_u64; 19];
    if call(
        "bionic_formatted_stdio_fixture_foreign_file",
        foreign.as_mut_ptr().cast(),
    )? != 42
        || provider.stdout_bytes() != expected
    {
        return Err("foreign FILE escaped provider namespace".into());
    }
    // SAFETY: same pthread's host errno cell.
    if unsafe { *__error() } != 33_701 {
        return Err("formatted stdio changed host errno".into());
    }
    let expected_imports = ["__errno", "fprintf", "vfprintf"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected_imports {
        return Err(format!("import drift: {:?}", resolver.requested).into());
    }
    println!(
        "bionic-formatted-stdio-facade: PASS AndroidELF fprintf-AAPCS64+vfprintf-va_list FILE=provider-local output<=1048576 semantic/provider-failure=precommit host-errno=preserved"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-formatted-stdio-facade: {error}");
            ExitCode::from(2)
        }
    }
}
