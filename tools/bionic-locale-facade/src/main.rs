use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

const LOCALE_IMPORTS: [&str; 53] = [
    "__ctype_get_mb_cur_max",
    "btowc",
    "freelocale",
    "isalnum",
    "isalpha",
    "iswalpha_l",
    "iswalpha",
    "iswblank_l",
    "iswblank",
    "iswcntrl_l",
    "iswcntrl",
    "iswdigit_l",
    "iswdigit",
    "iswlower_l",
    "iswlower",
    "iswprint_l",
    "iswprint",
    "iswpunct_l",
    "iswpunct",
    "iswspace_l",
    "iswupper_l",
    "iswupper",
    "iswxdigit_l",
    "iswxdigit",
    "islower",
    "isupper",
    "isxdigit",
    "localeconv",
    "mbrlen",
    "mbrtowc",
    "mbsnrtowcs",
    "mbsrtowcs",
    "mbtowc",
    "newlocale",
    "setlocale",
    "strcoll_l",
    "strxfrm_l",
    "strxfrm",
    "tolower",
    "tolower_l",
    "towlower_l",
    "towupper_l",
    "towupper",
    "toupper",
    "toupper_l",
    "uselocale",
    "wcrtomb",
    "wcscoll_l",
    "wcscoll",
    "wcsnrtombs",
    "wcsxfrm_l",
    "wcsxfrm",
    "wctob",
];

unsafe extern "C" {
    fn aim_bionic_locale_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_locale_capability(capability: *const c_char) -> i32;
    fn aim_bionic_locale_live_handle_count() -> usize;
    fn aim_bionic_locale_test_prepare_host_state();
    fn aim_bionic_locale_test_host_state_is_preserved() -> i32;
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
        let Some(version) = request.version else {
            return Err(ResolveError::Rejected(format!(
                "locale fixture requires LIBC version: {}",
                request.symbol
            )));
        };
        if version.soname != "libc.so" || version.name != "LIBC" || version.hidden {
            return Err(ResolveError::Rejected(format!(
                "locale fixture version drift: {} {:?}",
                request.symbol, version
            )));
        }
        self.requested.insert(request.symbol.to_owned());
        let name = CString::new(request.symbol)
            .map_err(|error| ResolveError::Rejected(format!("invalid locale symbol: {error}")))?;
        let address = if request.symbol == "__errno" {
            // SAFETY: the errno resolver reads only the NUL-terminated name.
            unsafe { aim_bionic_errno_resolve(name.as_ptr()) }.map(|function| function as usize)
        } else {
            // SAFETY: all three strings live across the closed resolver call.
            let pointer = unsafe {
                aim_bionic_locale_resolve(c"libc.so".as_ptr(), name.as_ptr(), c"LIBC".as_ptr())
            };
            (!pointer.is_null()).then_some(pointer as usize)
        };
        address
            .map(|value| {
                let nonzero = NonZeroUsize::new(value).expect("provider address is non-zero");
                // SAFETY: the manifest pins each fixed Android arm64 signature.
                unsafe { ResolvedSymbol::new(nonzero) }
            })
            .map_or_else(
                || {
                    Err(ResolveError::Rejected(format!(
                        "closed locale namespace rejected {}",
                        request.symbol
                    )))
                },
                |resolved| Ok(Some(resolved)),
            )
    }
}

fn capability(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let name = CString::new(name)?;
    // SAFETY: provider reads the NUL-terminated capability name only.
    Ok(unsafe { aim_bionic_locale_capability(name.as_ptr()) } != 0)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments.next().ok_or("missing Android locale fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-locale-facade <android-arm64-fixture.so>".into());
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    // SAFETY: hidden gate helper only changes this pthread's host errno/fenv.
    unsafe { aim_bionic_locale_test_prepare_host_state() };
    for (symbol, expected) in [
        ("bionic_locale_fixture_basic", 42),
        ("bionic_locale_fixture_multibyte", 42),
        ("bionic_locale_fixture_collation", 42),
        ("bionic_locale_fixture_wctype", 42),
    ] {
        let result = image.call_exported_i32(symbol)?;
        if result != expected {
            return Err(format!("{symbol} failed at step {result}").into());
        }
    }
    // SAFETY: checks the host state installed immediately above.
    if unsafe { aim_bionic_locale_test_host_state_is_preserved() } == 0 {
        return Err("locale provider changed Darwin errno or fenv".into());
    }

    let worker_address = image.lookup_exported("bionic_locale_fixture_thread")?;
    let mut workers = Vec::new();
    for index in 0..8 {
        workers.push(std::thread::spawn(move || {
            let function: unsafe extern "C" fn(i32) -> i32 =
                unsafe { std::mem::transmute(worker_address) };
            // SAFETY: the Android fixture export has the pinned one-int signature.
            unsafe { function(index & 1) }
        }));
    }
    for worker in workers {
        let result = worker.join().map_err(|_| "locale worker panicked")?;
        if result != 42 {
            return Err(format!("locale TLS worker failed at step {result}").into());
        }
    }
    // SAFETY: value-only provider diagnostic after all worker joins.
    if unsafe { aim_bionic_locale_live_handle_count() } != 0 {
        return Err("locale handle survived fixture teardown".into());
    }

    let expected: BTreeSet<String> = LOCALE_IMPORTS
        .into_iter()
        .chain(["__errno"])
        .map(str::to_owned)
        .collect();
    if resolver.requested != expected {
        return Err(format!("locale resolver manifest drift: {:?}", resolver.requested).into());
    }
    for supported in [
        "C",
        "POSIX",
        "C.UTF-8",
        "en_US.UTF-8",
        "utf8-mbstate8",
        "C-collation",
        "Unicode-wide-ctype",
        "Android-ICU-76.1",
    ] {
        if !capability(supported)? {
            return Err(format!("missing locale capability {supported}").into());
        }
    }
    for unsupported in ["tzcode-strftime", "host-global-setlocale"] {
        if capability(unsupported)? {
            return Err(format!("unsupported locale capability escaped: {unsupported}").into());
        }
    }
    println!(
        "bionic-locale-facade: PASS AndroidELF imports=53+errno locale=C+POSIX+C.UTF-8 thread-local UTF-8-state collate=C wide=Android-ICU-76.1 host-errno+fenv=preserved"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-locale-facade: {error}");
            ExitCode::from(2)
        }
    }
}
