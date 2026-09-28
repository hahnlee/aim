use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::env;
use std::ffi::{c_char, c_int, c_long, c_void};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn getpagesize() -> i32;
    #[link_name = "sysconf"]
    fn host_sysconf(name: c_int) -> c_long;
    fn aim_bionic_time_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn aim_bionic_time_capability_failed() -> i32;
    fn aim_bionic_errno_load() -> i32;
    fn aim_bionic_time_test_arm_alarm(microseconds: u32) -> i32;
    fn aim_bionic_time_test_finish_alarm();
    fn aim_bionic_time_test_force_boottime_overflow() -> i32;
}

struct ClosedResolver;

impl ClosedResolver {
    fn resolved(address: unsafe extern "C" fn()) -> ResolvedSymbol {
        let address = NonZeroUsize::new(address as usize).expect("function address is non-zero");
        // SAFETY: the closed manifests pin each fixed-register Android arm64 ABI.
        unsafe { ResolvedSymbol::new(address) }
    }

    fn lookup(name: &str) -> Option<unsafe extern "C" fn()> {
        let bytes = match name {
            "__errno" => c"__errno",
            "clock_gettime" => c"clock_gettime",
            "gettimeofday" => c"gettimeofday",
            "nanosleep" => c"nanosleep",
            "sysconf" => c"sysconf",
            _ => return None,
        };
        // SAFETY: providers read only the static NUL-terminated symbol name.
        unsafe {
            aim_bionic_time_resolve(bytes.as_ptr())
                .or_else(|| aim_bionic_errno_resolve(bytes.as_ptr()))
        }
    }
}

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "time fixture imports must be unversioned".to_owned(),
            ));
        }
        Ok(Self::lookup(request.symbol).map(Self::resolved))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments.next().ok_or("missing Android time fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-time-facade <android-arm64-fixture.so>".into());
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver;
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    // SAFETY: this hidden test seam executes the production 128-bit conversion
    // with a value that cannot fit Android's signed seconds field.
    unsafe { *__error() = 33_100 };
    if unsafe { aim_bionic_time_test_force_boottime_overflow() } != -1
        || unsafe { aim_bionic_errno_load() } != 75
        || unsafe { *__error() } != 33_100
        || unsafe { aim_bionic_time_capability_failed() } != 0
    {
        return Err("BOOTTIME overflow gate did not publish isolated Android EOVERFLOW".into());
    }

    // SAFETY: host errno is audit-only and never resolver-visible.
    unsafe { *__error() = 33_101 };
    let basic = image.call_exported_i32("bionic_time_fixture_basic")?;
    if basic != 42 {
        return Err(format!("basic Android time fixture failed at step {basic}").into());
    }
    // SAFETY: same current-pthread host errno cell set immediately above.
    if unsafe { *__error() } != 33_101 {
        return Err("basic time facade call changed host errno".into());
    }
    let guest_page = image.call_exported_i32("bionic_time_fixture_page_size")?;
    // The Android sysconf selector and the host VM provider must describe the
    // same process mapping granularity (16 KiB on current Apple Silicon).
    let host_page = unsafe { getpagesize() };
    if guest_page != host_page {
        return Err(format!("page-size mismatch guest={guest_page} host={host_page}").into());
    }
    // Check both CPU selectors against the corresponding Darwin host values;
    // this catches a facade that merely reports a positive fixed constant.
    let host_configured = unsafe { host_sysconf(57) };
    let host_online = unsafe { host_sysconf(58) };
    let guest_configured = image.call_exported_i32("bionic_time_fixture_configured_processors")?;
    let guest_online = image.call_exported_i32("bionic_time_fixture_online_processors")?;
    if host_configured <= 0
        || host_online <= 0
        || guest_configured as c_long != host_configured
        || guest_online as c_long != host_online
    {
        return Err(format!(
            "processor-count mismatch guest=({guest_configured},{guest_online}) host=({host_configured},{host_online})"
        )
        .into());
    }

    // SAFETY: test-only helper installs one process SIGALRM handler and timer.
    if unsafe { aim_bionic_time_test_arm_alarm(50_000) } != 0 {
        // SAFETY: report the current pthread's host errno for this harness failure.
        return Err(
            format!("could not arm EINTR acceptance alarm: errno {}", unsafe {
                *__error()
            })
            .into(),
        );
    }
    // SAFETY: audit host errno is set after signal setup.
    unsafe { *__error() = 33_102 };
    let interrupted = image.call_exported_i32("bionic_time_fixture_interrupted");
    // SAFETY: always cancel and restore the prior handler before propagating.
    unsafe { aim_bionic_time_test_finish_alarm() };
    let interrupted = interrupted?;
    if interrupted != 42 {
        return Err(format!("EINTR Android time fixture failed at step {interrupted}").into());
    }
    // SAFETY: same current-pthread host errno cell set before guest execution.
    if unsafe { *__error() } != 33_102 {
        return Err("interrupted nanosleep changed host errno".into());
    }
    // SAFETY: value-only sticky diagnostic from the linked facade.
    if unsafe { aim_bionic_time_capability_failed() } != 0 {
        return Err("time facade encountered an unknown host capability failure".into());
    }
    println!("bionic-time-facade: PASS Android ELF clocks nanosleep/EINTR sysconf errno");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-time-facade: {error}");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
fn _function_pointer_is_data_pointer_sized() {
    const _: () = assert!(size_of::<unsafe extern "C" fn()>() == size_of::<*mut c_void>());
}
