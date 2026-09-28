use aim_elf_loader::{
    Capability, LoadError, LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver,
};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::num::NonZeroUsize;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_TLS: u32 = 7;
const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const PT_GNU_RELRO: u32 = 0x6474_e552;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const DT_NULL: i64 = 0;
const DT_NEEDED: i64 = 1;
const DT_STRTAB: i64 = 5;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_FINI: i64 = 13;
const DT_FINI_ARRAYSZ: i64 = 28;
const DT_VERNEED: i64 = 0x6fff_fffe;
const DT_GNU_HASH: i64 = 0x6fff_fef5;
const DT_AARCH64_BTI_PLT: i64 = 0x7000_0001;
const R_AARCH64_TLS_TPREL64: u32 = 1030;
const R_AARCH64_TLSDESC: u32 = 1031;
const PROVIDER_SONAME: &str = "libaim_provider.so";
const PROVIDER_VERSION: &str = "AIM_1";
const LIFECYCLE_SONAME: &str = "liblifecycle_sink.so";

static PROVIDER_DATA: i32 = 11;
static LIBCXX_OBJECT_STORAGE: usize = 0;
static FINALIZER_EVENTS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

unsafe extern "C" fn provider_value() -> i32 {
    77
}

unsafe extern "C" fn libcxx_placeholder() {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--relro-write-child")
    {
        let path = arguments.get(2).ok_or("missing RELRO child fixture")?;
        let bytes = fs::read(path)?;
        let mut resolver = FixtureResolver::default();
        let mut image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
        image.run_initializers()?;
        let _ = image.call_exported_i32("relro_write_attempt")?;
        return Err("PT_GNU_RELRO page remained writable".into());
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--tls-live-unload-child")
    {
        let path = arguments.get(2).ok_or("missing TLS child fixture")?;
        let bytes = fs::read(path)?;
        let mut image = LoadedElf::load(&bytes)?;
        image.run_initializers()?;
        let exchange = image.lookup_exported("fixture_tls_exchange")?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let parked = Arc::new(Barrier::new(2));
        let worker_parked = parked.clone();
        let worker = thread::spawn(move || {
            // SAFETY: the fixture export has the audited int(int) ABI and image is retained by
            // the parent until this call has returned and its TLS allocation is live.
            let function: unsafe extern "C" fn(i32) -> i32 =
                unsafe { std::mem::transmute(exchange) };
            if unsafe { function(91) } != 7000 {
                std::process::abort();
            }
            ready_tx.send(()).unwrap();
            worker_parked.wait();
        });
        ready_rx.recv()?;
        drop(image);
        parked.wait();
        worker.join().unwrap();
        return Err("TLS image unloaded with a live thread".into());
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--tls-forged-descriptor-child")
    {
        let path = arguments.get(2).ok_or("missing forged TLS child fixture")?;
        let bytes = fs::read(path)?;
        let mut image = LoadedElf::load(&bytes)?;
        image.run_initializers()?;
        let descriptor_export = image.lookup_exported("fixture_tls_descriptor")?;
        // SAFETY: this trusted fixture export has the audited void*() ABI.
        let descriptor_fn: unsafe extern "C" fn() -> *const usize =
            unsafe { std::mem::transmute(descriptor_export) };
        let descriptor = unsafe { descriptor_fn() };
        if descriptor.is_null() {
            return Err("TLS fixture returned a null descriptor".into());
        }
        // SAFETY: the trusted fixture returns its live, mapped two-word GOT descriptor.
        let resolver = unsafe { descriptor.read_unaligned() };
        let token = unsafe { descriptor.add(1).read_unaligned() };
        let forged = [resolver, token];
        // SAFETY: the first word is the installed resolver. Passing a copied descriptor must
        // fail-stop at the exact registered-address check before any TLS state is returned.
        let resolver_fn: unsafe extern "C" fn(*const usize) -> isize =
            unsafe { std::mem::transmute(resolver) };
        let _ = unsafe { resolver_fn(forged.as_ptr()) };
        return Err("forged TLS descriptor address was accepted".into());
    }
    if arguments.len() != 10 {
        return Err(
            "usage: elf-loader-gate POSITIVE.so IMPORT.so WEAK.so LAZY.so RELRO.so TLS.so IFUNC.so FINALIZER.so LIBCXX.so".into(),
        );
    }
    let positive = fs::read(&arguments[1])?;
    let import = fs::read(&arguments[2])?;
    let weak = fs::read(&arguments[3])?;
    let lazy = fs::read(&arguments[4])?;
    let relro = fs::read(&arguments[5])?;
    let tls = fs::read(&arguments[6])?;
    let ifunc = fs::read(&arguments[7])?;
    let finalizer = fs::read(&arguments[8])?;
    let libcxx = fs::read(&arguments[9])?;

    run_positive(&positive)?;
    run_import(&import)?;
    run_weak(&weak)?;
    run_resolver_negative_matrix(&import)?;
    expect_capability(&lazy, |capability| {
        matches!(capability, Capability::LazyBinding)
    })?;
    run_relro(&relro, &arguments[5])?;
    run_tls(&tls, &arguments[6])?;
    run_ifunc(&ifunc)?;
    run_malformed_matrix(&positive)?;
    run_finalizer_lifecycle(&finalizer)?;
    run_aarch64_bti_plt(&libcxx)?;

    println!(
        "elf-loader-gate: positive=constructor-order import=ABS64+GLOB_DAT+JUMP_SLOT \
         resolver=closed+versioned weak=zero NOW=required RELRO=read-only \
         TLS=TLSDESC+per-thread-template+quiescent-unload \
         IFUNC=IRELATIVE-resolver \
         wx=reject overflow=reject overlap=reject bounds=reject \
         finalizers=array-reverse+DT_FINI exactly-once cleanup=drop \
         DT_AARCH64_BTI_PLT=zero-presence real-libcxx=relocated-without-init"
    );
    Ok(())
}

fn run_ifunc(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut image = LoadedElf::load(bytes)?;
    image.run_initializers()?;
    if image.call_exported_i32("fixture_ifunc_value")? != 1032 {
        return Err("R_AARCH64_IRELATIVE resolver result mismatch".into());
    }
    Ok(())
}

#[derive(Default)]
struct LibcxxStructuralResolver {
    requests: HashSet<String>,
    weak_thread_atexit_requests: usize,
}

impl SymbolResolver for LibcxxStructuralResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.needed_libraries != ["libc.so", "libm.so", "libdl.so"] {
            return Err(ResolveError::Rejected(format!(
                "unexpected libc++ dependency list: {:?}",
                request.needed_libraries
            )));
        }
        if request.symbol == "__cxa_thread_atexit_impl" && request.version.is_none() {
            self.weak_thread_atexit_requests += 1;
            return Ok(None);
        }
        let version = request.version.ok_or_else(|| {
            ResolveError::Rejected(format!("unversioned libc++ import: {}", request.symbol))
        })?;
        let expected_soname = if request.symbol == "dl_iterate_phdr" {
            "libdl.so"
        } else {
            "libc.so"
        };
        if version.soname != expected_soname
            || version.name != "LIBC"
            || version.hidden
            || version.flags != 0
        {
            return Err(ResolveError::Rejected(format!(
                "unexpected libc++ import version: {} from {}@{} hidden={} flags={}",
                request.symbol, version.soname, version.name, version.hidden, version.flags
            )));
        }
        if !self.requests.insert(request.symbol.to_owned()) {
            return Err(ResolveError::Rejected(format!(
                "duplicate libc++ resolver request: {}",
                request.symbol
            )));
        }
        let address = if request.symbol == "stderr" {
            (&raw const LIBCXX_OBJECT_STORAGE).cast::<u8>() as usize
        } else {
            libcxx_placeholder as usize
        };
        let address = NonZeroUsize::new(address)
            .ok_or_else(|| ResolveError::Rejected("null structural provider".to_owned()))?;
        // SAFETY: this gate deliberately does not run libc++ initializers or code. The static
        // addresses remain valid until the image is dropped and suffice to prove eager
        // relocation, GNU version routing, final protection, and export parsing.
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

fn run_aarch64_bti_plt(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if dynamic_tag_value(bytes, DT_AARCH64_BTI_PLT)? != 0 {
        return Err("real libc++ DT_AARCH64_BTI_PLT value drifted".into());
    }
    let mut resolver = LibcxxStructuralResolver::default();
    let image = LoadedElf::load_with_resolver(bytes, &mut resolver)?;
    if image.soname() != Some("libc++_shared.so")
        || image.needed_libraries() != ["libc.so", "libm.so", "libdl.so"]
        || resolver.requests.len() != 160
        || resolver.weak_thread_atexit_requests != 1
        || image.lookup_exported("_Znwm").is_err()
    {
        return Err(format!(
            "real libc++ structural load mismatch: soname={:?} needed={:?} requests={} weak-thread-atexit={}",
            image.soname(),
            image.needed_libraries(),
            resolver.requests.len(),
            resolver.weak_thread_atexit_requests
        )
        .into());
    }
    drop(image);

    let mut nonzero = bytes.to_vec();
    let bti = dynamic_tag_entry(bytes, DT_AARCH64_BTI_PLT)?;
    nonzero[bti + 8..bti + 16].copy_from_slice(&1_u64.to_le_bytes());
    let mut resolver = LibcxxStructuralResolver::default();
    match LoadedElf::load_with_resolver(&nonzero, &mut resolver) {
        Err(LoadError::Capability(Capability::DynamicFlags { tag, value }))
            if tag == DT_AARCH64_BTI_PLT && value == 1 => {}
        Err(error) => return Err(format!("nonzero BTI_PLT returned wrong error: {error}").into()),
        Ok(_) => return Err("nonzero DT_AARCH64_BTI_PLT loaded".into()),
    }

    let mut duplicate = bytes.to_vec();
    let spare = dynamic_tag_entry(bytes, DT_GNU_HASH)?;
    duplicate[spare..spare + 8].copy_from_slice(&DT_AARCH64_BTI_PLT.to_le_bytes());
    let mut resolver = LibcxxStructuralResolver::default();
    match LoadedElf::load_with_resolver(&duplicate, &mut resolver) {
        Err(LoadError::Format("duplicate DT_AARCH64_BTI_PLT")) => {}
        Err(error) => return Err(format!("duplicate BTI_PLT returned wrong error: {error}").into()),
        Ok(_) => return Err("duplicate DT_AARCH64_BTI_PLT loaded".into()),
    }
    Ok(())
}

fn run_relro(
    bytes: &[u8],
    fixture_path: &std::ffi::OsStr,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut resolver = FixtureResolver::default();
    let mut image = LoadedElf::load_with_resolver(bytes, &mut resolver)?;
    image.run_initializers()?;
    if image.call_exported_i32("imported_value")? != 165 {
        return Err("RELRO fixture import execution mismatch".into());
    }

    let status = Command::new(env::current_exe()?)
        .arg("--relro-write-child")
        .arg(fixture_path)
        .status()?;
    if status.signal().is_none() {
        return Err(format!("RELRO write child did not fault: {status}").into());
    }
    run_relro_malformed_matrix(bytes)?;
    Ok(())
}

fn run_relro_malformed_matrix(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let header = *load_program_headers(bytes, Some(PT_GNU_RELRO))?
        .first()
        .ok_or("RELRO fixture has no PT_GNU_RELRO")?;

    let mut writable_flags = bytes.to_vec();
    writable_flags[header + 4..header + 8].copy_from_slice(&(PF_W | 4).to_le_bytes());
    expect_rejected(&writable_flags, "writable PT_GNU_RELRO flags")?;

    let mut zero_size = bytes.to_vec();
    zero_size[header + 40..header + 48].copy_from_slice(&0_u64.to_le_bytes());
    expect_rejected(&zero_size, "zero-sized PT_GNU_RELRO")?;

    let mut outside_load = bytes.to_vec();
    outside_load[header + 16..header + 24].copy_from_slice(&0x4000_0000_u64.to_le_bytes());
    expect_rejected(&outside_load, "PT_GNU_RELRO outside PT_LOAD")?;

    let mut duplicate = bytes.to_vec();
    let eh_frame = *load_program_headers(bytes, Some(PT_GNU_EH_FRAME))?
        .first()
        .ok_or("RELRO fixture has no PT_GNU_EH_FRAME")?;
    duplicate[eh_frame..eh_frame + 4].copy_from_slice(&PT_GNU_RELRO.to_le_bytes());
    expect_rejected(&duplicate, "duplicate PT_GNU_RELRO")?;
    Ok(())
}

fn run_tls(bytes: &[u8], fixture_path: &std::ffi::OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let mut image = LoadedElf::load(bytes)?;
    image.run_initializers()?;
    let exchange = image.lookup_exported("fixture_tls_exchange")?;
    let alignment = image.lookup_exported("fixture_tls_alignment")?;
    let rendezvous = Arc::new(Barrier::new(5));
    let mut workers = Vec::new();
    for value in [11_i32, 22, 33, 44] {
        let rendezvous = rendezvous.clone();
        workers.push(thread::spawn(move || -> Result<(), String> {
            // SAFETY: both addresses are audited fixture exports with the stated AAPCS64 ABI;
            // the image remains mapped until every worker is joined below.
            let exchange_fn: unsafe extern "C" fn(i32) -> i32 =
                unsafe { std::mem::transmute(exchange) };
            let alignment_fn: unsafe extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(alignment) };
            let initial = unsafe { exchange_fn(value) };
            rendezvous.wait();
            let retained = unsafe { exchange_fn(value + 1) };
            let aligned = unsafe { alignment_fn() };
            if initial != 7000 || retained != value * 1000 + value || aligned != 0 {
                return Err(format!(
                    "thread TLS mismatch value={value} initial={initial} retained={retained} aligned={aligned}"
                ));
            }
            Ok(())
        }));
    }
    rendezvous.wait();
    for worker in workers {
        worker.join().map_err(|_| "TLS worker panicked")??;
    }
    // Joining destroys each Darwin pthread's host TLS map. Drop therefore also proves that the
    // guest allocations released their module ownership before unmapping the image.
    drop(image);

    let status = Command::new(env::current_exe()?)
        .arg("--tls-live-unload-child")
        .arg(fixture_path)
        .status()?;
    if status.signal().is_none() {
        return Err(format!("live-thread TLS unload did not fail closed: {status}").into());
    }
    let status = Command::new(env::current_exe()?)
        .arg("--tls-forged-descriptor-child")
        .arg(fixture_path)
        .status()?;
    if status.signal().is_none() {
        return Err(format!("forged TLS descriptor did not fail closed: {status}").into());
    }
    run_tls_malformed_matrix(bytes)?;
    Ok(())
}

fn run_tls_malformed_matrix(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let tls = *load_program_headers(bytes, Some(PT_TLS))?
        .first()
        .ok_or("TLS fixture has no PT_TLS")?;

    let mut filesz_exceeds_memsz = bytes.to_vec();
    let memory_size = read_u64(bytes, tls + 40)?;
    filesz_exceeds_memsz[tls + 32..tls + 40]
        .copy_from_slice(&memory_size.saturating_add(1).to_le_bytes());
    expect_rejected(&filesz_exceeds_memsz, "PT_TLS filesz exceeds memsz")?;

    let mut bad_alignment = bytes.to_vec();
    bad_alignment[tls + 48..tls + 56].copy_from_slice(&3_u64.to_le_bytes());
    expect_rejected(&bad_alignment, "PT_TLS non-power-of-two alignment")?;

    let mut outside_load = bytes.to_vec();
    outside_load[tls + 16..tls + 24].copy_from_slice(&0x4000_0000_u64.to_le_bytes());
    expect_rejected(&outside_load, "PT_TLS outside PT_LOAD")?;

    let mut duplicate = bytes.to_vec();
    let eh_frame = *load_program_headers(bytes, Some(PT_GNU_EH_FRAME))?
        .first()
        .ok_or("TLS fixture has no PT_GNU_EH_FRAME")?;
    duplicate[eh_frame..eh_frame + 4].copy_from_slice(&PT_TLS.to_le_bytes());
    expect_rejected(&duplicate, "duplicate PT_TLS")?;

    let relocation = first_relocation(bytes, R_AARCH64_TLSDESC)?;
    let mut outside_tls = bytes.to_vec();
    outside_tls[relocation + 16..relocation + 24].copy_from_slice(&memory_size.to_le_bytes());
    expect_rejected(&outside_tls, "TLSDESC offset outside PT_TLS")?;

    let mut unaligned_target = bytes.to_vec();
    let target = read_u64(bytes, relocation)?;
    unaligned_target[relocation..relocation + 8]
        .copy_from_slice(&target.saturating_add(1).to_le_bytes());
    expect_rejected(&unaligned_target, "unaligned TLSDESC target")?;

    let mut unsupported_model = bytes.to_vec();
    let info = read_u64(bytes, relocation + 8)?;
    let changed = (info & !u64::from(u32::MAX)) | u64::from(R_AARCH64_TLS_TPREL64);
    unsupported_model[relocation + 8..relocation + 16].copy_from_slice(&changed.to_le_bytes());
    expect_capability(&unsupported_model, |capability| {
        matches!(
            capability,
            Capability::UnsupportedRelocation {
                relocation_type: R_AARCH64_TLS_TPREL64,
                ..
            }
        )
    })?;
    Ok(())
}

fn first_relocation(
    bytes: &[u8],
    requested_type: u32,
) -> Result<usize, Box<dyn std::error::Error>> {
    let address = dynamic_tag_value(bytes, DT_RELA)?;
    let size = usize::try_from(dynamic_tag_value(bytes, DT_RELASZ)?)?;
    let file = virtual_to_file(bytes, address, size)?;
    for entry in (file..file.checked_add(size).ok_or("RELA file range overflow")?).step_by(24) {
        if read_u64(bytes, entry + 8)? as u32 == requested_type {
            return Ok(entry);
        }
    }
    Err(format!("relocation type {requested_type} missing").into())
}

unsafe extern "C" fn lifecycle_record(value: i32) {
    FINALIZER_EVENTS
        .lock()
        .expect("finalizer recorder poisoned")
        .push(value);
}

struct LifecycleResolver;

impl SymbolResolver for LifecycleResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.needed_libraries != [LIFECYCLE_SONAME]
            || request.symbol != "lifecycle_record"
            || request.version.is_some()
        {
            return Err(ResolveError::Rejected(format!(
                "unexpected lifecycle request: {}",
                request.symbol
            )));
        }
        let address = NonZeroUsize::new(lifecycle_record as usize)
            .ok_or_else(|| ResolveError::Rejected("null lifecycle recorder".to_owned()))?;
        // SAFETY: the recorder has static lifetime and the exact Android AArch64 void(int) ABI.
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

fn run_finalizer_lifecycle(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    FINALIZER_EVENTS.lock().unwrap().clear();
    {
        let mut resolver = LifecycleResolver;
        let image = LoadedElf::load_with_resolver(bytes, &mut resolver)?;
        drop(image);
    }
    if !FINALIZER_EVENTS.lock().unwrap().is_empty() {
        return Err("uninitialized image executed finalizers".into());
    }

    for explicit_close in [true, false] {
        let mut resolver = LifecycleResolver;
        let mut image = LoadedElf::load_with_resolver(bytes, &mut resolver)?;
        image.run_initializers()?;
        if image.call_exported_i32("finalizer_value")? != 42 {
            return Err("finalizer fixture export mismatch".into());
        }
        if explicit_close {
            image.close();
        } else {
            drop(image);
        }
    }
    if *FINALIZER_EVENTS.lock().unwrap() != [12, 11, 19, 12, 11, 19] {
        return Err(format!(
            "standalone finalizer order/exactly-once mismatch: {:?}",
            *FINALIZER_EVENTS.lock().unwrap()
        )
        .into());
    }

    let mut malformed_size = bytes.to_vec();
    let size_entry = dynamic_tag_entry(bytes, DT_FINI_ARRAYSZ)?;
    malformed_size[size_entry + 8..size_entry + 16].copy_from_slice(&7_u64.to_le_bytes());
    let mut resolver = LifecycleResolver;
    match LoadedElf::load_with_resolver(&malformed_size, &mut resolver) {
        Err(LoadError::Format("invalid DT_FINI_ARRAY pair")) => {}
        Err(error) => return Err(format!("bad FINI_ARRAYSZ returned wrong error: {error}").into()),
        Ok(_) => return Err("misaligned DT_FINI_ARRAYSZ loaded".into()),
    }

    let mut malformed_fini = bytes.to_vec();
    let fini_entry = dynamic_tag_entry(bytes, DT_FINI)?;
    malformed_fini[fini_entry + 8..fini_entry + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut resolver = LifecycleResolver;
    match LoadedElf::load_with_resolver(&malformed_fini, &mut resolver) {
        Err(LoadError::Bounds("DT_FINI function")) => {}
        Err(error) => return Err(format!("bad DT_FINI returned wrong error: {error}").into()),
        Ok(_) => return Err("out-of-range DT_FINI loaded".into()),
    }
    Ok(())
}

fn run_positive(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..32 {
        let mut image = LoadedElf::load(bytes)?;
        if !matches!(
            image.call_exported_i32("fixture_value"),
            Err(LoadError::InitializersNotRun)
        ) {
            return Err("export was callable before DT_INIT_ARRAY".into());
        }
        image.run_initializers()?;
        if image.call_exported_i32("fixture_value")? != 42 {
            return Err("constructor order/export result mismatch".into());
        }
        if !matches!(
            image.run_initializers(),
            Err(LoadError::InitializersAlreadyRun)
        ) {
            return Err("DT_INIT_ARRAY ran more than once".into());
        }
    }
    Ok(())
}

fn run_import(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(
        LoadedElf::load(bytes),
        Err(LoadError::UnresolvedSymbol { .. })
    ) {
        return Err("default loader unexpectedly searched a host/global namespace".into());
    }
    for _ in 0..16 {
        let mut resolver = FixtureResolver::default();
        let mut image = LoadedElf::load_with_resolver(bytes, &mut resolver)?;
        if image.needed_libraries() != [PROVIDER_SONAME] {
            return Err("DT_NEEDED order/string was not preserved".into());
        }
        if image.soname() != Some("libaim_import.so") {
            return Err("DT_SONAME was not preserved".into());
        }
        image.run_initializers()?;
        if image.call_exported_i32("imported_value")? != 165 {
            return Err("resolved import execution mismatch".into());
        }
        if resolver.requests != 2 {
            return Err("resolver was not cached per dynsym".into());
        }
    }
    Ok(())
}

fn run_weak(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut resolver = MissingResolver::default();
    let mut image = LoadedElf::load_with_resolver(bytes, &mut resolver)?;
    image.run_initializers()?;
    if image.call_exported_i32("weak_value")? != 19 || resolver.requests != 1 {
        return Err("unresolved weak symbol did not resolve to zero".into());
    }
    Ok(())
}

#[derive(Default)]
struct FixtureResolver {
    requests: usize,
}

impl SymbolResolver for FixtureResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        self.requests += 1;
        if request.needed_libraries != [PROVIDER_SONAME] {
            return Err(ResolveError::UnknownSoname(
                request
                    .needed_libraries
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "<none>".to_owned()),
            ));
        }
        let version = request
            .version
            .ok_or_else(|| ResolveError::VersionMismatch {
                soname: PROVIDER_SONAME.to_owned(),
                symbol: request.symbol.to_owned(),
                requested: "<unversioned>".to_owned(),
            })?;
        if version.soname != PROVIDER_SONAME {
            return Err(ResolveError::UnknownSoname(version.soname.to_owned()));
        }
        if version.name != PROVIDER_VERSION || version.hidden || version.flags != 0 {
            return Err(ResolveError::VersionMismatch {
                soname: version.soname.to_owned(),
                symbol: request.symbol.to_owned(),
                requested: version.name.to_owned(),
            });
        }
        let address = match request.symbol {
            "provider_value" => provider_value as usize,
            "provider_data" => (&raw const PROVIDER_DATA).cast::<u8>() as usize,
            _ => return Ok(None),
        };
        let address = NonZeroUsize::new(address).ok_or_else(|| {
            ResolveError::Rejected("fixture provider has null address".to_owned())
        })?;
        // SAFETY: both fixture providers have static process lifetime and the exact AArch64
        // function/data ABI declared by import.c.
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

#[derive(Default)]
struct MissingResolver {
    requests: usize,
}

impl SymbolResolver for MissingResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        self.requests += 1;
        if request.symbol != "optional_provider" || request.version.is_some() {
            return Err(ResolveError::Rejected("unexpected weak request".to_owned()));
        }
        Ok(None)
    }
}

struct VersionMismatchResolver;

impl SymbolResolver for VersionMismatchResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        let version = request
            .version
            .ok_or_else(|| ResolveError::VersionMismatch {
                soname: "<none>".to_owned(),
                symbol: request.symbol.to_owned(),
                requested: "<unversioned>".to_owned(),
            })?;
        Err(ResolveError::VersionMismatch {
            soname: version.soname.to_owned(),
            symbol: request.symbol.to_owned(),
            requested: version.name.to_owned(),
        })
    }
}

fn run_resolver_negative_matrix(original: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut unknown_soname = original.to_vec();
    replace_dynamic_string(
        &mut unknown_soname,
        dynamic_tag_value(original, DT_NEEDED)?,
        PROVIDER_SONAME,
        "libunknow_art_provider.so",
    )?;
    let mut resolver = FixtureResolver::default();
    match LoadedElf::load_with_resolver(&unknown_soname, &mut resolver) {
        Err(LoadError::Resolver {
            source: ResolveError::UnknownSoname(_),
            ..
        }) => {}
        Err(error) => return Err(format!("unknown SONAME returned wrong error: {error}").into()),
        Ok(_) => return Err("unknown SONAME resolved".into()),
    }

    let mut resolver = VersionMismatchResolver;
    match LoadedElf::load_with_resolver(original, &mut resolver) {
        Err(LoadError::Resolver {
            source: ResolveError::VersionMismatch { .. },
            ..
        }) => {}
        Err(error) => return Err(format!("version mismatch returned wrong error: {error}").into()),
        Ok(_) => return Err("wrong symbol version resolved".into()),
    }

    let mut malformed_hash = original.to_vec();
    let vernaux_file = first_vernaux_file(original)?;
    malformed_hash[vernaux_file..vernaux_file + 4].copy_from_slice(&0_u32.to_le_bytes());
    let mut resolver = FixtureResolver::default();
    match LoadedElf::load_with_resolver(&malformed_hash, &mut resolver) {
        Err(LoadError::Format("Elf64_Vernaux hash mismatch")) => {}
        Err(error) => {
            return Err(format!("malformed VERNEED hash returned wrong error: {error}").into());
        }
        Ok(_) => return Err("malformed VERNEED hash loaded".into()),
    }

    let mut malformed_version = original.to_vec();
    let verneed_address = dynamic_tag_value(original, DT_VERNEED)?;
    let verneed_file = virtual_to_file(original, verneed_address, 16)?;
    malformed_version[verneed_file..verneed_file + 2].copy_from_slice(&2_u16.to_le_bytes());
    let mut resolver = FixtureResolver::default();
    match LoadedElf::load_with_resolver(&malformed_version, &mut resolver) {
        Err(LoadError::Format("invalid Elf64_Verneed header")) => {}
        Err(error) => return Err(format!("malformed VERNEED returned wrong error: {error}").into()),
        Ok(_) => return Err("malformed VERNEED loaded".into()),
    }
    Ok(())
}

fn first_vernaux_file(bytes: &[u8]) -> Result<usize, Box<dyn std::error::Error>> {
    let address = dynamic_tag_value(bytes, DT_VERNEED)?;
    let file = virtual_to_file(bytes, address, 16)?;
    let auxiliary_offset = read_u32(bytes, file + 8)?;
    file.checked_add(usize::try_from(auxiliary_offset)?)
        .ok_or_else(|| "Vernaux offset overflow".into())
}

fn replace_dynamic_string(
    bytes: &mut [u8],
    string_offset: u64,
    expected: &str,
    replacement: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if expected.len() != replacement.len() {
        return Err("replacement string length differs".into());
    }
    let table = dynamic_tag_value(bytes, DT_STRTAB)?;
    let address = table
        .checked_add(string_offset)
        .ok_or("dynamic string address overflow")?;
    let file = virtual_to_file(bytes, address, expected.len() + 1)?;
    if bytes.get(file..file + expected.len()) != Some(expected.as_bytes()) {
        return Err("dynamic string mutation expectation mismatch".into());
    }
    bytes[file..file + replacement.len()].copy_from_slice(replacement.as_bytes());
    Ok(())
}

fn expect_capability(
    bytes: &[u8],
    predicate: impl FnOnce(&Capability) -> bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match LoadedElf::load(bytes) {
        Err(LoadError::Capability(capability)) if predicate(&capability) => Ok(()),
        Err(error) => Err(format!("wrong capability rejection: {error}").into()),
        Ok(_) => Err("unsupported ELF unexpectedly loaded".into()),
    }
}

fn run_malformed_matrix(original: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    expect_rejected(&original[..32], "truncated header")?;

    let mut wrong_machine = original.to_vec();
    wrong_machine[18..20].copy_from_slice(&0_u16.to_le_bytes());
    expect_rejected(&wrong_machine, "wrong machine")?;

    let headers = load_program_headers(original, Some(PT_LOAD))?;
    let first = *headers.first().ok_or("fixture has no PT_LOAD")?;
    let second = *headers.get(1).ok_or("fixture has only one PT_LOAD")?;

    let mut writable_executable = original.to_vec();
    let flags = read_u32(&writable_executable, first + 4)? | PF_W | PF_X;
    writable_executable[first + 4..first + 8].copy_from_slice(&flags.to_le_bytes());
    match LoadedElf::load(&writable_executable) {
        Err(LoadError::Protection(_)) => {}
        Err(error) => return Err(format!("W+X returned wrong error: {error}").into()),
        Ok(_) => return Err("W+X PT_LOAD unexpectedly loaded".into()),
    }

    let mut overlap = original.to_vec();
    let first_offset = read_u64(&overlap, first + 8)?;
    let first_address = read_u64(&overlap, first + 16)?;
    overlap[second + 8..second + 16].copy_from_slice(&first_offset.to_le_bytes());
    overlap[second + 16..second + 24].copy_from_slice(&first_address.to_le_bytes());
    expect_rejected(&overlap, "overlapping PT_LOAD")?;

    let mut overflowing = original.to_vec();
    overflowing[first + 16..first + 24].copy_from_slice(&(u64::MAX - 0x1000).to_le_bytes());
    overflowing[first + 40..first + 48].copy_from_slice(&0x4000_u64.to_le_bytes());
    overflowing[first + 48..first + 56].copy_from_slice(&1_u64.to_le_bytes());
    expect_rejected(&overflowing, "virtual range overflow")?;

    let mut out_of_bounds = original.to_vec();
    out_of_bounds[first + 8..first + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    out_of_bounds[first + 48..first + 56].copy_from_slice(&1_u64.to_le_bytes());
    expect_rejected(&out_of_bounds, "file range out of bounds")?;
    Ok(())
}

fn expect_rejected(bytes: &[u8], case: &str) -> Result<(), Box<dyn std::error::Error>> {
    if LoadedElf::load(bytes).is_ok() {
        Err(format!("malformed case loaded: {case}").into())
    } else {
        Ok(())
    }
}

fn dynamic_tag_value(bytes: &[u8], requested: i64) -> Result<u64, Box<dyn std::error::Error>> {
    let entry = dynamic_tag_entry(bytes, requested)?;
    read_u64(bytes, entry + 8)
}

fn dynamic_tag_entry(bytes: &[u8], requested: i64) -> Result<usize, Box<dyn std::error::Error>> {
    let dynamic_header = *load_program_headers(bytes, Some(PT_DYNAMIC))?
        .first()
        .ok_or("missing PT_DYNAMIC")?;
    let offset = usize::try_from(read_u64(bytes, dynamic_header + 8)?)?;
    let size = usize::try_from(read_u64(bytes, dynamic_header + 32)?)?;
    for entry in (offset..offset.checked_add(size).ok_or("dynamic overflow")?).step_by(16) {
        let tag = read_u64(bytes, entry)? as i64;
        if tag == DT_NULL {
            break;
        }
        if tag == requested {
            return Ok(entry);
        }
    }
    Err(format!("dynamic tag {requested:#x} missing").into())
}

fn virtual_to_file(
    bytes: &[u8],
    address: u64,
    size: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    for header in load_program_headers(bytes, Some(PT_LOAD))? {
        let file_offset = read_u64(bytes, header + 8)?;
        let virtual_address = read_u64(bytes, header + 16)?;
        let file_size = read_u64(bytes, header + 32)?;
        let requested_end = address
            .checked_add(u64::try_from(size)?)
            .ok_or("virtual range overflow")?;
        let load_end = virtual_address
            .checked_add(file_size)
            .ok_or("load range overflow")?;
        if address >= virtual_address && requested_end <= load_end {
            return usize::try_from(
                file_offset
                    .checked_add(address - virtual_address)
                    .ok_or("file offset overflow")?,
            )
            .map_err(Into::into);
        }
    }
    Err("virtual address is not file-backed".into())
}

fn load_program_headers(
    bytes: &[u8],
    kind: Option<u32>,
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    let offset = usize::try_from(read_u64(bytes, 32)?)?;
    let entry_size = read_u16(bytes, 54)? as usize;
    let count = read_u16(bytes, 56)? as usize;
    let mut result = Vec::new();
    for index in 0..count {
        let entry = offset
            .checked_add(index.checked_mul(entry_size).ok_or("phdr overflow")?)
            .ok_or("phdr overflow")?;
        if kind.is_none_or(|expected| read_u32(bytes, entry).ok() == Some(expected)) {
            result.push(entry);
        }
    }
    Ok(result)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Box<dyn std::error::Error>> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or("u16 bounds")?
            .try_into()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("u32 bounds")?
            .try_into()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("u64 bounds")?
            .try_into()?,
    ))
}
