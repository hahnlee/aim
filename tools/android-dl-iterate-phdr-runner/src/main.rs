use aim_elf_loader::{LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver};
use std::env;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};

const ABI_VERSION: u32 = 1;
const PT_LOAD: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct AndroidPhdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C)]
struct AndroidDlPhdrInfo {
    dlpi_addr: u64,
    dlpi_name: *const c_char,
    dlpi_phdr: *const AndroidPhdr,
    dlpi_phnum: u16,
    reserved_padding: [u16; 3],
    dlpi_adds: u64,
    dlpi_subs: u64,
    dlpi_tls_modid: usize,
    dlpi_tls_data: *mut c_void,
}

#[repr(C)]
struct ImageRecord {
    image_id: u64,
    generation: u64,
    load_bias: u64,
    soname: *const c_char,
    phdrs: *const AndroidPhdr,
    phnum: u16,
    reserved16: [u16; 3],
    tls_modid: usize,
    tls_data_for_current_thread: *mut c_void,
}

#[repr(C)]
struct Snapshot {
    abi_version: u32,
    struct_size: u32,
    lease: *mut c_void,
    records: *const ImageRecord,
    record_count: usize,
    load_events: u64,
    unload_events: u64,
}

#[repr(C)]
struct ImageSource {
    abi_version: u32,
    struct_size: u32,
    context: *mut c_void,
    acquire: unsafe extern "C" fn(*mut c_void, *mut Snapshot) -> c_int,
    release: unsafe extern "C" fn(*mut c_void, *mut c_void),
}

type IterateCallback = unsafe extern "C" fn(*mut AndroidDlPhdrInfo, usize, *mut c_void) -> c_int;

unsafe extern "C" {
    fn aim_dl_phdr_bind_source(source: *const ImageSource) -> c_int;
    fn aim_bionic_dl_iterate_phdr(callback: IterateCallback, data: *mut c_void) -> c_int;
    fn aim_dl_phdr_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;
}

struct ImageOwned {
    image_id: u64,
    generation: u64,
    load_bias: u64,
    soname: CString,
    phdrs: Vec<AndroidPhdr>,
}

unsafe impl Send for ImageOwned {}
unsafe impl Sync for ImageOwned {}

struct RegistryInner {
    images: Vec<Arc<ImageOwned>>,
    adds: u64,
    subs: u64,
    acquires: usize,
    releases: usize,
    live_leases: usize,
    max_live_leases: usize,
}

struct Registry {
    inner: Mutex<RegistryInner>,
}

struct Lease {
    registry: Arc<Registry>,
    _images: Vec<Arc<ImageOwned>>,
    records: Vec<ImageRecord>,
}

unsafe impl Send for ImageRecord {}
unsafe impl Sync for ImageRecord {}

impl Registry {
    fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                images: Vec::new(),
                adds: 0,
                subs: 0,
                acquires: 0,
                releases: 0,
                live_leases: 0,
                max_live_leases: 0,
            }),
        }
    }

    fn publish(&self, image: Arc<ImageOwned>) {
        let mut inner = self.inner.lock().unwrap();
        inner.images.push(image);
        inner.adds += 1;
    }

    fn unpublish(&self, image_id: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.images.len();
        inner.images.retain(|image| image.image_id != image_id);
        if inner.images.len() != before {
            inner.subs += 1;
            true
        } else {
            false
        }
    }
}

unsafe extern "C" fn acquire_snapshot(context: *mut c_void, output: *mut Snapshot) -> c_int {
    if context.is_null() || output.is_null() {
        return -1;
    }
    let registry = Arc::from_raw(context.cast::<Registry>());
    let keep_registry = Arc::clone(&registry);
    let _ = Arc::into_raw(registry);
    let (images, adds, subs) = {
        let mut inner = keep_registry.inner.lock().unwrap();
        inner.acquires += 1;
        inner.live_leases += 1;
        inner.max_live_leases = inner.max_live_leases.max(inner.live_leases);
        (inner.images.clone(), inner.adds, inner.subs)
    };
    let records = images
        .iter()
        .map(|image| ImageRecord {
            image_id: image.image_id,
            generation: image.generation,
            load_bias: image.load_bias,
            soname: image.soname.as_ptr(),
            phdrs: image.phdrs.as_ptr(),
            phnum: image.phdrs.len() as u16,
            reserved16: [0; 3],
            tls_modid: 0,
            tls_data_for_current_thread: std::ptr::null_mut(),
        })
        .collect();
    let lease = Box::new(Lease {
        registry: keep_registry,
        _images: images,
        records,
    });
    let lease_pointer = Box::into_raw(lease);
    *output = Snapshot {
        abi_version: ABI_VERSION,
        struct_size: std::mem::size_of::<Snapshot>() as u32,
        lease: lease_pointer.cast(),
        records: (*lease_pointer).records.as_ptr(),
        record_count: (*lease_pointer).records.len(),
        load_events: adds,
        unload_events: subs,
    };
    0
}

unsafe extern "C" fn release_snapshot(_context: *mut c_void, lease: *mut c_void) {
    if lease.is_null() {
        return;
    }
    let lease = Box::from_raw(lease.cast::<Lease>());
    {
        let mut inner = lease.registry.inner.lock().unwrap();
        inner.releases += 1;
        inner.live_leases -= 1;
    }
    drop(lease);
}

struct PhdrResolver;

impl SymbolResolver for PhdrResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.needed_libraries != ["libdl.so"] || request.symbol != "dl_iterate_phdr" {
            return Err(ResolveError::Rejected(request.symbol.to_owned()));
        }
        let version = request
            .version
            .ok_or_else(|| ResolveError::VersionMismatch {
                soname: "libdl.so".to_owned(),
                symbol: request.symbol.to_owned(),
                requested: "<none>".to_owned(),
            })?;
        if version.soname != "libdl.so"
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
        let soname = CString::new("libdl.so").unwrap();
        let symbol = CString::new("dl_iterate_phdr").unwrap();
        let version = CString::new("LIBC").unwrap();
        let address =
            unsafe { aim_dl_phdr_resolve(soname.as_ptr(), symbol.as_ptr(), version.as_ptr()) }
                as usize;
        let address = NonZeroUsize::new(address)
            .ok_or_else(|| ResolveError::Rejected(request.symbol.to_owned()))?;
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

fn u16le(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or("u16 bounds")?
            .try_into()
            .unwrap(),
    ))
}
fn u32le(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("u32 bounds")?
            .try_into()
            .unwrap(),
    ))
}
fn u64le(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("u64 bounds")?
            .try_into()
            .unwrap(),
    ))
}

fn parse_phdrs(bytes: &[u8]) -> Result<Vec<AndroidPhdr>, Box<dyn std::error::Error>> {
    if bytes.get(0..6) != Some(&[0x7f, b'E', b'L', b'F', 2, 1]) {
        return Err("not ELF64 little-endian".into());
    }
    let offset = usize::try_from(u64le(bytes, 32)?)?;
    let entry_size = usize::from(u16le(bytes, 54)?);
    let count = usize::from(u16le(bytes, 56)?);
    if entry_size != 56 || count == 0 || count > u16::MAX as usize {
        return Err("invalid ELF program header table".into());
    }
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let base = offset
            .checked_add(index * entry_size)
            .ok_or("phdr overflow")?;
        result.push(AndroidPhdr {
            p_type: u32le(bytes, base)?,
            p_flags: u32le(bytes, base + 4)?,
            p_offset: u64le(bytes, base + 8)?,
            p_vaddr: u64le(bytes, base + 16)?,
            p_paddr: u64le(bytes, base + 24)?,
            p_filesz: u64le(bytes, base + 32)?,
            p_memsz: u64le(bytes, base + 40)?,
            p_align: u64le(bytes, base + 48)?,
        });
    }
    if !result.iter().any(|header| header.p_type == PT_LOAD) {
        return Err("ELF has no PT_LOAD".into());
    }
    Ok(result)
}

fn parse_hex(text: &str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(usize::from_str_radix(text.trim_start_matches("0x"), 16)?)
}

struct BarrierState {
    entered: bool,
    release: bool,
    stable: bool,
}

struct Barrier {
    mutex: Mutex<BarrierState>,
    condition: Condvar,
}

unsafe extern "C" fn blocking_callback(
    info: *mut AndroidDlPhdrInfo,
    size: usize,
    data: *mut c_void,
) -> c_int {
    if info.is_null() || size != std::mem::size_of::<AndroidDlPhdrInfo>() {
        return 81;
    }
    let name = CStr::from_ptr((*info).dlpi_name).to_bytes();
    if name != b"libdl-phdr-helper.so" {
        return 0;
    }
    let barrier = &*data.cast::<Barrier>();
    let before = (*info).dlpi_phdr;
    let phnum = (*info).dlpi_phnum;
    let mut state = barrier.mutex.lock().unwrap();
    state.entered = true;
    barrier.condition.notify_all();
    while !state.release {
        state = barrier.condition.wait(state).unwrap();
    }
    state.stable = before == (*info).dlpi_phdr
        && phnum == (*info).dlpi_phnum
        && (0..usize::from(phnum)).any(|index| (*before.add(index)).p_type == PT_LOAD);
    0
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() != 5 {
        return Err(
            "usage: runner TARGET.so HELPER.so TARGET_SYMBOL_VALUE HELPER_SYMBOL_VALUE".into(),
        );
    }
    let resolve =
        |soname: &str, symbol: &str, version: &str| -> Result<usize, Box<dyn std::error::Error>> {
            let soname = CString::new(soname)?;
            let symbol = CString::new(symbol)?;
            let version = CString::new(version)?;
            Ok(
                unsafe { aim_dl_phdr_resolve(soname.as_ptr(), symbol.as_ptr(), version.as_ptr()) }
                    as usize,
            )
        };
    if resolve("libdl.so", "dl_iterate_phdr", "LIBC")? == 0
        || resolve("libc.so", "dl_iterate_phdr", "LIBC")? != 0
        || resolve("libdl.so", "dlopen", "LIBC")? != 0
        || resolve("libdl.so", "dl_iterate_phdr", "GLIBC_2.2.5")? != 0
    {
        return Err("closed libdl.so@LIBC resolver policy failed".into());
    }
    let target_bytes = fs::read(&arguments[1])?;
    let helper_bytes = fs::read(&arguments[2])?;
    let mut resolver = PhdrResolver;
    let mut target_image = LoadedElf::load_with_resolver(&target_bytes, &mut resolver)?;
    let mut helper_image = LoadedElf::load(&helper_bytes)?;
    target_image.run_initializers()?;
    helper_image.run_initializers()?;
    if target_image.soname() != Some("libdl-phdr-target.so")
        || helper_image.soname() != Some("libdl-phdr-helper.so")
    {
        return Err("fixture SONAME mismatch".into());
    }
    let target_bias = target_image
        .lookup_exported("phdr_fixture_run")?
        .checked_sub(parse_hex(&arguments[3])?)
        .ok_or("target load bias underflow")?;
    let helper_bias = helper_image
        .lookup_exported("phdr_helper_marker")?
        .checked_sub(parse_hex(&arguments[4])?)
        .ok_or("helper load bias underflow")?;

    let registry = Arc::new(Registry::new());
    registry.publish(Arc::new(ImageOwned {
        image_id: 1,
        generation: 1,
        load_bias: target_bias as u64,
        soname: CString::new(target_image.soname().unwrap())?,
        phdrs: parse_phdrs(&target_bytes)?,
    }));
    registry.publish(Arc::new(ImageOwned {
        image_id: 2,
        generation: 1,
        load_bias: helper_bias as u64,
        soname: CString::new(helper_image.soname().unwrap())?,
        phdrs: parse_phdrs(&helper_bytes)?,
    }));
    let source_context = Arc::into_raw(Arc::clone(&registry)) as *mut c_void;
    let source = ImageSource {
        abi_version: ABI_VERSION,
        struct_size: std::mem::size_of::<ImageSource>() as u32,
        context: source_context,
        acquire: acquire_snapshot,
        release: release_snapshot,
    };
    if unsafe { aim_dl_phdr_bind_source(&source) } != 0 {
        return Err("snapshot source bind failed".into());
    }
    if target_image.call_exported_i32("phdr_fixture_run")? != 0 {
        return Err("Android callback/reentrant iteration failed".into());
    }
    if target_image.call_exported_i32("phdr_fixture_early_stop")? != 0 {
        return Err("Android early-stop propagation failed".into());
    }

    let barrier = Arc::new(Barrier {
        mutex: Mutex::new(BarrierState {
            entered: false,
            release: false,
            stable: false,
        }),
        condition: Condvar::new(),
    });
    let thread_barrier = Arc::clone(&barrier);
    let iteration = std::thread::spawn(move || unsafe {
        aim_bionic_dl_iterate_phdr(
            blocking_callback,
            Arc::as_ptr(&thread_barrier) as *mut c_void,
        )
    });
    {
        let mut state = barrier.mutex.lock().unwrap();
        while !state.entered {
            state = barrier.condition.wait(state).unwrap();
        }
    }
    if !registry.unpublish(2) {
        return Err("concurrent helper unpublish failed".into());
    }
    {
        let mut state = barrier.mutex.lock().unwrap();
        state.release = true;
        barrier.condition.notify_all();
    }
    if iteration.join().map_err(|_| "iteration thread panicked")? != 0
        || !barrier.mutex.lock().unwrap().stable
    {
        return Err("snapshot lease did not survive concurrent unpublish".into());
    }
    if target_image.call_exported_i32("phdr_fixture_after_unload")? != 0 {
        return Err("post-unload snapshot counters/content failed".into());
    }
    let inner = registry.inner.lock().unwrap();
    if inner.acquires != inner.releases || inner.live_leases != 0 || inner.max_live_leases < 2 {
        return Err("snapshot lease accounting/reentrancy mismatch".into());
    }
    println!("android-dl-iterate-phdr: ELF-callback=PT_LOAD+SONAME records=2->1");
    println!(
        "snapshot=leased concurrent-unpublish=stable reentrant=max{} early-stop=37",
        inner.max_live_leases
    );
    println!("resolver=libdl.so@LIBC-only negatives=3 dyld-fallback=0 adds=2 subs=1 info-size=64");
    drop(inner);
    unsafe {
        drop(Arc::from_raw(source_context.cast::<Registry>()));
    }
    drop(helper_image);
    drop(target_image);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("android-dl-iterate-phdr: {error}");
            ExitCode::from(2)
        }
    }
}
