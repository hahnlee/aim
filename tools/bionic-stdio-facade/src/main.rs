use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use bionic_stdio_facade::Provider;
use std::env;
use std::ffi::c_char;
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::Arc;
unsafe extern "C" {
    fn __error() -> *mut i32;
    fn aim_bionic_stdio_resolve(n: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_errno_resolve(n: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_wide_stdio_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> Option<unsafe extern "C" fn()>;
    static mut aim_bionic___sF: [u8; 456];
    static mut aim_bionic_stdin: *mut u8;
    static mut aim_bionic_stdout: *mut u8;
    static mut aim_bionic_stderr: *mut u8;
}
struct Resolver;
impl SymbolResolver for Resolver {
    fn resolve(&mut self, r: SymbolRequest<'_>) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if r.version.is_some() {
            return Err(ResolveError::Rejected("stdio imports unversioned".into()));
        }
        let address = if r.symbol == "__sF" {
            Some(std::ptr::addr_of_mut!(aim_bionic___sF) as usize)
        } else if r.symbol == "stdin" {
            Some(std::ptr::addr_of_mut!(aim_bionic_stdin) as usize)
        } else if r.symbol == "stdout" {
            Some(std::ptr::addr_of_mut!(aim_bionic_stdout) as usize)
        } else if r.symbol == "stderr" {
            Some(std::ptr::addr_of_mut!(aim_bionic_stderr) as usize)
        } else {
            let n = match r.symbol {
                "__errno" => c"__errno",
                "fclose" => c"fclose",
                "fflush" => c"fflush",
                "fileno" => c"fileno",
                "fopen" => c"fopen",
                "fputc" => c"fputc",
                "fputs" => c"fputs",
                "ferror" => c"ferror",
                "fgetc" => c"fgetc",
                "fgets" => c"fgets",
                "fputwc" => c"fputwc",
                "fread" => c"fread",
                "fseek" => c"fseek",
                "fseeko" => c"fseeko",
                "ftello" => c"ftello",
                "ftell" => c"ftell",
                "fwrite" => c"fwrite",
                "getc" => c"getc",
                "getwc" => c"getwc",
                "setbuf" => c"setbuf",
                "ungetc" => c"ungetc",
                "ungetwc" => c"ungetwc",
                _ => return Ok(None),
            };
            unsafe {
                aim_bionic_stdio_resolve(n.as_ptr())
                    .or_else(|| aim_bionic_errno_resolve(n.as_ptr()))
                    .or_else(|| {
                        aim_bionic_wide_stdio_resolve(
                            c"libc.so".as_ptr(),
                            n.as_ptr(),
                            c"LIBC".as_ptr(),
                        )
                    })
            }
            .map(|f| f as usize)
        };
        Ok(address.map(|a| unsafe { ResolvedSymbol::new(NonZeroUsize::new(a).unwrap()) }))
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = env::args_os().nth(1).ok_or("fixture")?;
    let provider = Arc::new(Provider::new(
        vec![(b"/system/input.bin".to_vec(), b"abcdef".to_vec())],
        b"input".to_vec(),
    )?);
    let _activation = provider.activate()?;
    for rejected in [c"fprintf", c"vfprintf"] {
        // SAFETY: resolver reads only the static NUL-terminated name.
        if unsafe { aim_bionic_stdio_resolve(rejected.as_ptr()) }.is_some() {
            return Err("formatted stdio escaped central resolver rejection".into());
        }
    }
    if unsafe { aim_bionic_stdio_resolve(c"perror".as_ptr()) }.is_none() {
        return Err("perror resolver".into());
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = Resolver;
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    // SAFETY: host errno is audit-only and never guest-resolved.
    unsafe { *__error() = 33_301 };
    let basic_result = image.call_exported_i32("bionic_stdio_fixture_basic")?;
    if basic_result != 42 {
        return Err(format!("basic fixture returned {basic_result}").into());
    }
    // SAFETY: same current pthread errno cell.
    if unsafe { *__error() } != 33_301 {
        return Err("stdio facade changed host errno".into());
    }
    if provider.stdout_bytes() != b"!" {
        return Err("stdout capture".into());
    }
    if provider.stderr_bytes() != b"ok" {
        return Err("stderr capture".into());
    }
    if image.call_exported_i32("bionic_stdio_fixture_race_setup")? != 42 {
        return Err("race setup".into());
    }
    let w = image.lookup_exported("bionic_stdio_fixture_race_write")?;
    let c = image.lookup_exported("bionic_stdio_fixture_race_close")?;
    let writer: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(w) };
    let close: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(c) };
    std::thread::scope(|s| {
        let mut h = Vec::new();
        for _ in 0..7 {
            h.push(s.spawn(move || {
                // SAFETY: Darwin host errno is pthread-local and audit-only.
                unsafe { *__error() = 33_350 };
                for _ in 0..1000 {
                    if unsafe { writer() } != 42 {
                        return false;
                    }
                }
                // SAFETY: same worker pthread errno cell.
                unsafe { *__error() == 33_350 }
            }))
        }
        h.push(s.spawn(move || unsafe { close() } == 42));
        for t in h {
            assert!(t.join().unwrap())
        }
    });
    if image.call_exported_i32("bionic_stdio_fixture_race_after")? != 42 {
        return Err("race lifetime".into());
    }
    if image.call_exported_i32("bionic_stdio_fixture_wide_race_setup")? != 42 {
        return Err("wide race setup".into());
    }
    let wide_get: unsafe extern "C" fn() -> i32 = unsafe {
        std::mem::transmute(image.lookup_exported("bionic_stdio_fixture_wide_race_get")?)
    };
    let wide_seek: unsafe extern "C" fn() -> i32 = unsafe {
        std::mem::transmute(image.lookup_exported("bionic_stdio_fixture_wide_race_seek")?)
    };
    let wide_close: unsafe extern "C" fn() -> i32 = unsafe {
        std::mem::transmute(image.lookup_exported("bionic_stdio_fixture_wide_race_close")?)
    };
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..5 {
            workers.push(scope.spawn(move || {
                for _ in 0..1000 {
                    if unsafe { wide_get() } != 42 {
                        return false;
                    }
                }
                true
            }));
        }
        for _ in 0..2 {
            workers.push(scope.spawn(move || {
                for _ in 0..1000 {
                    if unsafe { wide_seek() } != 42 {
                        return false;
                    }
                }
                true
            }));
        }
        workers.push(scope.spawn(move || unsafe { wide_close() } == 42));
        for worker in workers {
            assert!(worker.join().expect("wide stdio worker"));
        }
    });
    if image.call_exported_i32("bionic_stdio_fixture_wide_race_after")? != 42 {
        return Err("wide race lifetime".into());
    }
    println!("bionic-stdio-facade: PASS binary+wide FILE tokens __sF concurrent-close seek-reset");
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bionic-stdio-facade: {e}");
            ExitCode::from(2)
        }
    }
}
