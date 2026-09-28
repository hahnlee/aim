use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{c_char, c_int, c_void, CString};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const EXPECTED: [&str; 25] = [
    "pthread_cond_broadcast",
    "pthread_cond_destroy",
    "pthread_cond_signal",
    "pthread_cond_timedwait",
    "pthread_cond_wait",
    "pthread_create",
    "pthread_detach",
    "pthread_getspecific",
    "pthread_key_create",
    "pthread_key_delete",
    "pthread_join",
    "pthread_mutex_destroy",
    "pthread_mutex_init",
    "pthread_mutex_lock",
    "pthread_mutex_trylock",
    "pthread_mutex_unlock",
    "pthread_mutexattr_destroy",
    "pthread_mutexattr_init",
    "pthread_mutexattr_settype",
    "pthread_once",
    "pthread_rwlock_rdlock",
    "pthread_rwlock_unlock",
    "pthread_rwlock_wrlock",
    "pthread_self",
    "pthread_setspecific",
];

unsafe extern "C" {
    fn aim_bionic_pthread_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
    fn aim_bionic_pthread_provider_reset() -> c_int;
    fn aim_bionic_pthread_capability(capability: *const c_char) -> c_int;
    fn aim_bionic_pthread_key_create(
        key: *mut i32,
        destructor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    fn aim_bionic_pthread_key_delete(key: i32) -> c_int;
    fn aim_bionic_pthread_setspecific(key: i32, value: *const c_void) -> c_int;
    fn aim_bionic_pthread_getspecific(key: i32) -> *mut c_void;
}

#[derive(Default)]
struct ClosedResolver {
    requested: BTreeSet<String>,
    addresses: BTreeSet<usize>,
}

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.needed_libraries != ["libc.so"] {
            return Err(ResolveError::UnknownSoname(
                request.needed_libraries.join(","),
            ));
        }
        let version = request
            .version
            .ok_or_else(|| ResolveError::VersionMismatch {
                soname: "libc.so".to_owned(),
                symbol: request.symbol.to_owned(),
                requested: "<unversioned>".to_owned(),
            })?;
        if version.soname != "libc.so"
            || version.name != "LIBC"
            || version.hidden
            || version.flags != 0
        {
            return Err(ResolveError::VersionMismatch {
                soname: version.soname.to_owned(),
                symbol: request.symbol.to_owned(),
                requested: version.name.to_owned(),
            });
        }
        if !EXPECTED.contains(&request.symbol) {
            return Err(ResolveError::Rejected(request.symbol.to_owned()));
        }
        let soname = CString::new("libc.so").unwrap();
        let symbol = CString::new(request.symbol).unwrap();
        let required_version = CString::new("LIBC").unwrap();
        let address = unsafe {
            aim_bionic_pthread_resolve(soname.as_ptr(), symbol.as_ptr(), required_version.as_ptr())
        } as usize;
        let address = NonZeroUsize::new(address)
            .ok_or_else(|| ResolveError::Rejected(request.symbol.to_owned()))?;
        self.requested.insert(request.symbol.to_owned());
        self.addresses.insert(address.get());
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

fn capability(name: &str) -> bool {
    let name = CString::new(name).unwrap();
    unsafe { aim_bionic_pthread_capability(name.as_ptr()) != 0 }
}

fn expect_closed_resolver_negatives() -> Result<(), Box<dyn std::error::Error>> {
    let libc = CString::new("libc.so")?;
    let symbol = CString::new("pthread_self")?;
    let version = CString::new("LIBC")?;
    let wrong_soname = CString::new("libSystem.B.dylib")?;
    let wrong_version = CString::new("DARWIN")?;
    let unknown = CString::new("pthread_cancel")?;
    let unowned_barrier = CString::new("pthread_barrier_init")?;
    unsafe {
        if !aim_bionic_pthread_resolve(wrong_soname.as_ptr(), symbol.as_ptr(), version.as_ptr())
            .is_null()
            || !aim_bionic_pthread_resolve(libc.as_ptr(), symbol.as_ptr(), wrong_version.as_ptr())
                .is_null()
            || !aim_bionic_pthread_resolve(libc.as_ptr(), unknown.as_ptr(), version.as_ptr())
                .is_null()
            || !aim_bionic_pthread_resolve(
                libc.as_ptr(),
                unowned_barrier.as_ptr(),
                version.as_ptr(),
            )
            .is_null()
        {
            return Err("closed resolver accepted a fallback".into());
        }
    }
    Ok(())
}

fn wait_for_exported_count(
    image: &LoadedElf,
    symbol: &str,
    expected: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let address = image.lookup_exported(symbol)?;
    let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = unsafe { function() };
        if value == expected {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("{symbol} remained {value}, expected {expected}").into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture_path = arguments.next().ok_or("missing Android pthread fixture")?;
    if arguments.next().is_some() {
        return Err("usage: android-bionic-pthread-provider-runner FIXTURE.so".into());
    }
    expect_closed_resolver_negatives()?;
    let bytes = fs::read(fixture_path)?;
    let mut resolver = ClosedResolver::default();
    let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    if image.needed_libraries() != ["libc.so"] {
        return Err("fixture dependency namespace is not exactly libc.so".into());
    }
    image.run_initializers()?;
    if image.call_exported_i32("pthread_fixture_setup")? != 0 {
        return Err("Android fixture setup failed".into());
    }
    if image.call_exported_i32("pthread_fixture_errorcheck_hold")? != 0 {
        return Err("errorcheck self-lock did not return EDEADLK".into());
    }
    let errorcheck_wrong = image.lookup_exported("pthread_fixture_errorcheck_wrong_unlock")?;
    let wrong_result = std::thread::spawn(move || {
        let function: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(errorcheck_wrong) };
        unsafe { function() }
    })
    .join()
    .map_err(|_| "errorcheck wrong-owner worker panicked")?;
    if wrong_result != 0 {
        return Err(format!("errorcheck wrong-owner unlock failed: {wrong_result}").into());
    }
    if image.call_exported_i32("pthread_fixture_errorcheck_release")? != 0 {
        return Err("errorcheck owner release failed".into());
    }
    if image.call_exported_i32("pthread_fixture_lifecycle_basic")? != 0 {
        return Err("Android create/join/detach lifecycle basic path failed".into());
    }
    wait_for_exported_count(&image, "pthread_fixture_lifecycle_detached_done", 1)?;
    if image.call_exported_i32("pthread_fixture_lifecycle_race_setup")? != 0 {
        return Err("lifecycle join/detach race setup failed".into());
    }
    let race_join = image.lookup_exported("pthread_fixture_lifecycle_race_join")?;
    let race_detach = image.lookup_exported("pthread_fixture_lifecycle_race_detach")?;
    let join_thread = std::thread::spawn(move || {
        let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(race_join) };
        unsafe { function() }
    });
    let detach_thread = std::thread::spawn(move || {
        let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(race_detach) };
        unsafe { function() }
    });
    std::thread::sleep(Duration::from_millis(10));
    if image.call_exported_i32("pthread_fixture_lifecycle_race_release")? != 0 {
        return Err("lifecycle race target release failed".into());
    }
    let join_result = join_thread
        .join()
        .map_err(|_| "lifecycle join caller panicked")?;
    let detach_result = detach_thread
        .join()
        .map_err(|_| "lifecycle detach caller panicked")?;
    let successes = usize::from(join_result == 0) + usize::from(detach_result == 0);
    let valid_loser = |result: i32| result == 0 || result == 3 || result == 22;
    if successes != 1 || !valid_loser(join_result) || !valid_loser(detach_result) {
        return Err(format!(
            "join/detach race mismatch: join={join_result} detach={detach_result}"
        )
        .into());
    }
    if resolver.requested != EXPECTED.into_iter().map(str::to_owned).collect() {
        return Err(format!("resolver import set mismatch: {:?}", resolver.requested).into());
    }
    if resolver.addresses.len() != EXPECTED.len() {
        return Err("provider resolver returned aliased or zero addresses".into());
    }

    let worker_address = image.lookup_exported("pthread_fixture_worker")?;
    let mut threads = Vec::new();
    for _ in 0..8 {
        threads.push(std::thread::spawn(move || {
            let worker: unsafe extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(worker_address) };
            unsafe { worker() }
        }));
    }
    for thread in threads {
        let result = thread.join().map_err(|_| "worker panicked")?;
        if result != 0 {
            return Err(format!("Android fixture worker failed: {result}").into());
        }
    }
    let cond_waiter = image.lookup_exported("pthread_fixture_cond_waiter")?;
    let mut cond_threads = Vec::new();
    for _ in 0..4 {
        cond_threads.push(std::thread::spawn(move || {
            let function: unsafe extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(cond_waiter) };
            unsafe { function() }
        }));
    }
    wait_for_exported_count(&image, "pthread_fixture_cond_waiting", 4)?;
    if image.call_exported_i32("pthread_fixture_cond_spurious_signal")? != 0 {
        return Err("condition predicate-loop signal failed".into());
    }
    std::thread::sleep(Duration::from_millis(20));
    if image.call_exported_i32("pthread_fixture_cond_completed")? != 0 {
        return Err("condition waiter escaped without predicate".into());
    }
    if image.call_exported_i32("pthread_fixture_cond_destroy_busy")? != 0 {
        return Err("condition destroy/wait race policy failed".into());
    }
    if image.call_exported_i32("pthread_fixture_cond_signal_one")? != 0 {
        return Err("condition signal failed".into());
    }
    wait_for_exported_count(&image, "pthread_fixture_cond_completed", 1)?;
    if image.call_exported_i32("pthread_fixture_cond_broadcast")? != 0 {
        return Err("condition broadcast failed".into());
    }
    for thread in cond_threads {
        let result = thread.join().map_err(|_| "condition waiter panicked")?;
        if result != 0 {
            return Err(format!("condition waiter failed: {result}").into());
        }
    }
    if image.call_exported_i32("pthread_fixture_cond_timedwait")? != 0 {
        return Err("condition CLOCK_MONOTONIC timeout/relock failed".into());
    }
    if image.call_exported_i32("pthread_fixture_rwlock_recursive_read")? != 0 {
        return Err("rwlock recursive reader semantics failed".into());
    }
    let rw_reader = image.lookup_exported("pthread_fixture_rwlock_reader")?;
    let mut rw_readers = Vec::new();
    for _ in 0..4 {
        rw_readers.push(std::thread::spawn(move || {
            let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(rw_reader) };
            unsafe { function() }
        }));
    }
    wait_for_exported_count(&image, "pthread_fixture_rwlock_reader_entries", 4)?;
    let rw_writer = image.lookup_exported("pthread_fixture_rwlock_writer")?;
    let writer = std::thread::spawn(move || {
        let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(rw_writer) };
        unsafe { function() }
    });
    std::thread::sleep(Duration::from_millis(20));
    if image.call_exported_i32("pthread_fixture_rwlock_writer_entered")? != 0 {
        return Err("rwlock writer entered while readers were active".into());
    }
    if image.call_exported_i32("pthread_fixture_rwlock_release_readers")? != 0 {
        return Err("rwlock reader release failed".into());
    }
    for reader in rw_readers {
        let result = reader.join().map_err(|_| "rwlock reader panicked")?;
        if result != 0 {
            return Err(format!("rwlock reader failed: {result}").into());
        }
    }
    let writer_result = writer.join().map_err(|_| "rwlock writer panicked")?;
    if writer_result != 0 {
        return Err(format!("rwlock writer failed: {writer_result}").into());
    }
    if image.call_exported_i32("pthread_fixture_rwlock_writer_hold")? != 0 {
        return Err("rwlock writer hold failed".into());
    }
    let wrong_unlock = image.lookup_exported("pthread_fixture_rwlock_wrong_unlock")?;
    let wrong_result = std::thread::spawn(move || {
        let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(wrong_unlock) };
        unsafe { function() }
    })
    .join()
    .map_err(|_| "rwlock wrong-owner worker panicked")?;
    if wrong_result != 0 {
        return Err(format!("rwlock wrong-owner unlock failed: {wrong_result}").into());
    }
    if image.call_exported_i32("pthread_fixture_rwlock_writer_release")? != 0 {
        return Err("rwlock writer owner could not release".into());
    }
    let lookup_race = image.lookup_exported("pthread_fixture_mutex_lookup_race")?;
    let destroy_race = image.lookup_exported("pthread_fixture_mutex_destroy_race")?;
    let mut race_threads = Vec::new();
    for _ in 0..8 {
        race_threads.push(std::thread::spawn(move || {
            let function: unsafe extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(lookup_race) };
            unsafe { function() }
        }));
    }
    race_threads.push(std::thread::spawn(move || {
        let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(destroy_race) };
        unsafe { function() }
    }));
    for thread in race_threads {
        let result = thread
            .join()
            .map_err(|_| "destroy/lookup race worker panicked")?;
        if result != 0 {
            return Err(format!("destroy/lookup race failed: {result}").into());
        }
    }
    let finish = image.call_exported_i32("pthread_fixture_finish")?;
    if finish != 0 {
        return Err(format!("Android fixture finish failed: {finish}").into());
    }
    drop(image);
    let reset_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reset = unsafe { aim_bionic_pthread_provider_reset() };
        if reset == 0 {
            break;
        }
        if reset != 16 || Instant::now() >= reset_deadline {
            return Err(format!("provider reset failed after lifecycle cleanup: {reset}").into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    // Bionic exposes only valid-bit|slot. Its generation is internal key-data
    // state, so use of a deleted integer key is POSIX-undefined and aliases a
    // newly allocated key when that slot is reused. Prove that exact ABI rather
    // than inventing guest-visible generation bits.
    let mut stale_key = 0;
    let mut reused_key = 0;
    unsafe {
        if aim_bionic_pthread_key_create(&mut stale_key, None) != 0
            || aim_bionic_pthread_key_delete(stale_key) != 0
            || aim_bionic_pthread_key_create(&mut reused_key, None) != 0
            || stale_key != reused_key
            || aim_bionic_pthread_setspecific(stale_key, 0x1234usize as *const c_void) != 0
            || aim_bionic_pthread_getspecific(reused_key) as usize != 0x1234
            || aim_bionic_pthread_setspecific(reused_key, std::ptr::null()) != 0
            || aim_bionic_pthread_key_delete(reused_key) != 0
            || aim_bionic_pthread_provider_reset() != 0
        {
            return Err("Bionic stale-key slot-reuse semantics mismatch".into());
        }
    }
    for supported in [
        "thread-identity-token",
        "tls-key-destructor",
        "once-private",
        "mutex-normal-private",
        "mutex-recursive-private",
        "mutex-errorcheck-private",
        "cond-private",
        "cond-monotonic-clock",
        "rwlock-private-reader-preferred",
        "thread-create-join-detach-owner",
    ] {
        if !capability(supported) {
            return Err(format!("missing capability {supported}").into());
        }
    }
    for unsupported in [
        "fork",
        "robust",
        "pshared",
        "priority-inheritance",
        "recursive",
        "errorcheck",
        "cancellation",
    ] {
        if capability(unsupported) {
            return Err(format!("unsupported capability escaped: {unsupported}").into());
        }
    }
    println!("android-bionic-pthread-provider: ELF=executed resolver=libc.so@LIBC imports=24/24 extra-owner=pthread_create");
    println!(
        "pthread-self=stable+unique tls-destructor=2-pass once=1 mutex=8x2000-contention destroy-lookup=race-safe"
    );
    println!(
        "mutex-attr=normal+recursive+errorcheck recursive-depth=2 errorcheck-self=EDEADLK wrong-unlock=EPERM destroyed-attr=EINVAL pshared+PI=ENOTSUP"
    );
    println!(
        "cond=4-waiters signal=1 broadcast=3 predicate-loop=held monotonic-timeout=110 destroy-wait=EBUSY pshared=ENOTSUP"
    );
    println!(
        "rwlock=4-concurrent-readers writer=blocked-then-progress wrong-unlock=EPERM recursive-read=2 lazy-zero-init reset=idle"
    );
    println!(
        "thread-lifecycle=create+join+detach token=provider-owned result=roundtrip foreign=ESRCH self-join=EDEADLK race=one-winner target-clean=reset"
    );
    println!(
        "opaque-layout=side-table no-Darwin-reinterpret stale-key=reuse-alias(Bionic-undefined) unsupported=fork+robust+pshared+PI+thread-attrs"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("android-bionic-pthread-provider: {error}");
            ExitCode::from(2)
        }
    }
}
