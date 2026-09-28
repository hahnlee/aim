use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use bionic_dso_lifecycle_facade::{Hooks, Lifecycle, Registration};
use std::env;
use std::ffi::{c_char, c_void};
use std::fs;
use std::mem;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

unsafe extern "C" {
    fn aim_bionic_dso_lifecycle_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
}

static ATFORK_CALLS: AtomicUsize = AtomicUsize::new(0);
static LAST_ATFORK_DSO: AtomicUsize = AtomicUsize::new(0);
static STDIO_CLEANUPS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn unregister_atfork(dso: *mut c_void) {
    LAST_ATFORK_DSO.store(dso as usize, Ordering::Release);
    ATFORK_CALLS.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn stdio_cleanup() {
    STDIO_CLEANUPS.fetch_add(1, Ordering::AcqRel);
}

struct ClosedResolver;

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "DSO lifecycle imports must be unversioned".to_owned(),
            ));
        }
        let name = match request.symbol {
            "__cxa_atexit" => c"__cxa_atexit",
            "__cxa_finalize" => c"__cxa_finalize",
            _ => return Ok(None),
        };
        // SAFETY: the provider only reads the static NUL-terminated name.
        let address = unsafe { aim_bionic_dso_lifecycle_resolve(name.as_ptr()) };
        Ok(address.map(|function| {
            let address = NonZeroUsize::new(function as usize).unwrap();
            // SAFETY: the manifest pins both functions to fixed-register Android arm64 PCS.
            unsafe { ResolvedSymbol::new(address) }
        }))
    }
}

unsafe fn exported<T: Copy>(
    image: &LoadedElf,
    name: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    if mem::size_of::<T>() != mem::size_of::<usize>() {
        return Err("function pointer width drift".into());
    }
    let address = image.lookup_exported(name)?;
    // SAFETY: the caller supplies the exact fixture signature for a validated code export.
    Ok(unsafe { mem::transmute_copy::<usize, T>(&address) })
}

fn expect_log(
    count: unsafe extern "C" fn() -> u32,
    at: unsafe extern "C" fn(u32) -> u32,
    expected: &[u32],
) -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: both functions are fixed-register fixture getters and the image remains mapped.
    if unsafe { count() } as usize != expected.len() {
        return Err(format!("callback log length mismatch for {expected:?}").into());
    }
    for (index, value) in expected.iter().copied().enumerate() {
        // SAFETY: index is within the count just read and the image remains mapped.
        if unsafe { at(index as u32) } != value {
            return Err(format!("callback log mismatch at {index} for {expected:?}").into());
        }
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments
        .next()
        .ok_or("missing Android DSO lifecycle fixture")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-dso-lifecycle-facade <fixture.so>".into());
    }

    let lifecycle = Arc::new(Lifecycle::new(Hooks {
        unregister_atfork: Some(unregister_atfork),
        stdio_cleanup: Some(stdio_cleanup),
    }));
    let activation = lifecycle.activate()?;
    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver;
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;

    // SAFETY: every binding below matches its C definition in probes/fixture.c.
    let reset: unsafe extern "C" fn() = unsafe { exported(&image, "bionic_dso_fixture_reset")? };
    let main_handle: unsafe extern "C" fn() -> usize =
        unsafe { exported(&image, "bionic_dso_fixture_main_handle")? };
    let other_handle: unsafe extern "C" fn() -> usize =
        unsafe { exported(&image, "bionic_dso_fixture_other_handle")? };
    let callback: unsafe extern "C" fn(u32) -> usize =
        unsafe { exported(&image, "bionic_dso_fixture_callback")? };
    let argument: unsafe extern "C" fn(u32) -> usize =
        unsafe { exported(&image, "bionic_dso_fixture_argument")? };
    let register_triples: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_triples")? };
    let register_global: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_global_set")? };
    let register_reentrant: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_reentrant")? };
    let register_concurrent: unsafe extern "C" fn(u32) -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_concurrent")? };
    let register_blocking: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_blocking")? };
    let register_after_unpublish: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_after_unpublish")? };
    let register_null: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_register_null_callback")? };
    let finalize_main: unsafe extern "C" fn() =
        unsafe { exported(&image, "bionic_dso_fixture_finalize_main")? };
    let finalize_global: unsafe extern "C" fn() =
        unsafe { exported(&image, "bionic_dso_fixture_finalize_global")? };
    let log_count: unsafe extern "C" fn() -> u32 =
        unsafe { exported(&image, "bionic_dso_fixture_log_count")? };
    let log_at: unsafe extern "C" fn(u32) -> u32 =
        unsafe { exported(&image, "bionic_dso_fixture_log_at")? };
    let errors: unsafe extern "C" fn() -> u32 =
        unsafe { exported(&image, "bionic_dso_fixture_errors")? };
    let concurrent_complete: unsafe extern "C" fn() -> i32 =
        unsafe { exported(&image, "bionic_dso_fixture_concurrent_complete")? };
    let blocking_entered: unsafe extern "C" fn() -> u32 =
        unsafe { exported(&image, "bionic_dso_fixture_blocking_entered")? };
    let release_blocking: unsafe extern "C" fn() =
        unsafe { exported(&image, "bionic_dso_fixture_release_blocking")? };

    // SAFETY: getters use no arguments and return image-local object addresses.
    let main_dso = unsafe { main_handle() };
    let other_dso = unsafe { other_handle() };
    if main_dso == 0 || other_dso == 0 || main_dso == other_dso {
        return Err("fixture DSO handles are not distinct image-local addresses".into());
    }
    lifecycle.publish(main_dso)?;
    lifecycle.publish(other_dso)?;

    // SAFETY: fixture state is idle before reset and registration.
    unsafe { reset() };
    if unsafe { register_triples() } != 42 {
        return Err("Android triple registration failed".into());
    }
    let registrations = lifecycle.registrations()?;
    let mut expected = Vec::new();
    for index in 1..=3 {
        // SAFETY: getters accept indexes 1..=3.
        expected.push(Registration {
            function: unsafe { callback(index) },
            argument: unsafe { argument(index) },
            dso: main_dso,
        });
    }
    if registrations != expected {
        return Err("function/argument/DSO triples changed in provider storage".into());
    }
    // SAFETY: the DSO remains published and the image remains executable.
    unsafe { finalize_main() };
    expect_log(log_count, log_at, &[3, 2, 1])?;
    if ATFORK_CALLS.load(Ordering::Acquire) != 1
        || LAST_ATFORK_DSO.load(Ordering::Acquire) != main_dso
    {
        return Err("non-null finalize did not invoke unregister_atfork exactly once".into());
    }
    unsafe { finalize_main() };
    expect_log(log_count, log_at, &[3, 2, 1])?;
    if lifecycle.registration_count(main_dso)? != 0 || ATFORK_CALLS.load(Ordering::Acquire) != 2 {
        return Err("DSO callbacks were not removed exactly once".into());
    }

    unsafe { reset() };
    if unsafe { register_global() } != 42 {
        return Err("global finalize registration set failed".into());
    }
    unsafe { finalize_global() };
    expect_log(log_count, log_at, &[6, 5, 4])?;
    if lifecycle.registration_count(0)? != 0 || STDIO_CLEANUPS.load(Ordering::Acquire) != 1 {
        return Err("finalize(NULL) did not drain globally and run stdio cleanup".into());
    }
    unsafe { finalize_global() };
    if STDIO_CLEANUPS.load(Ordering::Acquire) != 2 {
        return Err("each finalize(NULL) did not invoke stdio cleanup exactly once".into());
    }

    unsafe { reset() };
    if unsafe { register_reentrant() } != 42 {
        return Err("reentrant callback registration failed".into());
    }
    unsafe { finalize_global() };
    expect_log(log_count, log_at, &[7, 8])?;
    if lifecycle.registration_count(main_dso)? != 0 || STDIO_CLEANUPS.load(Ordering::Acquire) != 3 {
        return Err("global reentrant registration was not drained".into());
    }
    if unsafe { register_null() } != -1 || lifecycle.registration_count(main_dso)? != 0 {
        return Err("null __cxa_atexit callback was not rejected atomically".into());
    }

    unsafe { reset() };
    thread::scope(|scope| {
        for worker in 0..4_u32 {
            scope.spawn(move || {
                for index in (worker * 16)..((worker + 1) * 16) {
                    // SAFETY: all 64 object indexes are distinct and the image remains mapped.
                    assert_eq!(unsafe { register_concurrent(index) }, 0);
                }
            });
        }
        for _ in 0..2 {
            scope.spawn(move || {
                for _ in 0..128 {
                    // SAFETY: provider serializes extraction and releases its lock for callbacks.
                    unsafe { finalize_main() };
                }
            });
        }
    });
    unsafe { finalize_main() };
    if unsafe { concurrent_complete() } != 42
        || unsafe { errors() } != 0
        || lifecycle.registration_count(main_dso)? != 0
    {
        return Err("concurrent registration/finalization lost or duplicated a callback".into());
    }

    unsafe { reset() };
    if unsafe { register_blocking() } != 42 {
        return Err("blocking callback registration failed".into());
    }
    thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        let finalizer = scope.spawn(move || {
            // SAFETY: mapping stays alive until the scoped finalizer joins.
            unsafe { finalize_main() };
        });
        let mut observed = false;
        for _ in 0..1_000_000 {
            // SAFETY: atomic fixture getter remains mapped.
            if unsafe { blocking_entered() } != 0 {
                observed = true;
                break;
            }
            thread::yield_now();
        }
        if !observed {
            return Err("blocking callback did not enter".into());
        }
        if lifecycle.active_callback_count(main_dso)? != 1
            || lifecycle.try_unpublish(main_dso).is_ok()
        {
            return Err("unpublish did not reject an active Android callback".into());
        }
        // SAFETY: release is an atomic store into the live fixture.
        unsafe { release_blocking() };
        finalizer
            .join()
            .map_err(|_| "blocking finalizer panicked")?;
        Ok(())
    })?;
    expect_log(log_count, log_at, &[9])?;
    lifecycle.try_unpublish(main_dso)?;
    if unsafe { register_after_unpublish() } != -1 {
        return Err("registration through an unpublished DSO did not fail closed".into());
    }
    lifecycle.try_unpublish(other_dso)?;

    // The production loader publishes a mapping range, not the hidden/local
    // `__dso_handle`. Admission therefore happens lazily on the first guest
    // registration and range teardown discovers/drains the exact handle.
    unsafe { reset() };
    let range_start = main_dso.min(other_dso) - 1;
    let range_end = main_dso.max(other_dso) + 1;
    lifecycle.publish_image(range_start, range_end)?;
    if unsafe { register_triples() } != 42 {
        return Err("range-backed lazy DSO handle admission failed".into());
    }
    lifecycle.finalize_image(range_start, range_end)?;
    expect_log(log_count, log_at, &[3, 2, 1])?;
    if lifecycle.registration_count(main_dso)? != 0
        || lifecycle.finalize_image(range_start, range_end).is_ok()
        || unsafe { register_after_unpublish() } != -1
    {
        return Err("range teardown was not quiescent and exactly once".into());
    }
    if lifecycle.registration_count(0)? != 0 || lifecycle.active_callback_count(0)? != 0 {
        return Err("provider did not reach unload quiescence".into());
    }
    if ATFORK_CALLS.load(Ordering::Acquire) == 0
        || LAST_ATFORK_DSO.load(Ordering::Acquire) != main_dso
    {
        return Err("Bionic unregister_atfork composition hook was not invoked".into());
    }

    drop(activation);
    drop(image);
    println!(
        "bionic-dso-lifecycle-facade: PASS exact-triples lifo \
         atfork-hook=2-calls/2-hooks stdio-hook=3-calls/3-hooks global-reentrant \
         concurrent-callbacks=64-exactly-once range-lazy-admit=exactly-once \
         unpublish=busy-drain-success"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-dso-lifecycle-facade: {error}");
            ExitCode::from(2)
        }
    }
}
