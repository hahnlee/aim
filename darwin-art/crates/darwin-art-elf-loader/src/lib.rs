use std::collections::{HashMap, HashSet};
use std::error::Error as StdError;
use std::ffi::{c_int, c_void};
use std::fmt;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use sha2::{Digest, Sha256};

mod direct_syscall;
#[cfg(all(test, target_arch = "aarch64", target_os = "macos"))]
mod direct_syscall_execution_tests;
mod direct_tls;
mod ffi;
mod mapping;
mod namespace;
mod packed_relocations;
mod parser;
mod segment_loading;
mod tls;

use parser::{
    parse_dynamic, parse_dynamic_with_policy, parse_image, parse_image_with_appcompat,
    parse_image_with_policy, validate_dynamic_capabilities,
};

use mapping::Mapping;
use tls::{
    MAX_TLS_ALIGNMENT, MAX_TLS_SIZE, TlsDescriptor, TlsDescriptorContext, TlsModule,
    darwin_art_tlsdesc_resolver, register_tls_descriptor, release_current_thread_tls,
    unregister_tls_descriptors,
};

pub use namespace::{
    ClosedElfNamespace, GlobalElfImage, LoadedElfGraph, NamespaceError, NamespaceScopes,
    RetainedElfMapping,
};

mod finalization;
mod initialization;
mod metadata;
mod relocation_symbols;
pub use metadata::{ElfMetadata, inspect_elf_metadata};

#[cfg(not(target_os = "macos"))]
compile_error!("darwin-art-elf-loader supports only macOS hosts");

const EI_NIDENT: usize = 16;
const ELF64_EHDR_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;
const ELF64_DYN_SIZE: usize = 16;
const ELF64_RELA_SIZE: usize = 24;
const ELF64_SYM_SIZE: usize = 24;

const ET_DYN: u16 = 3;
const EM_AARCH64: u16 = 183;
const EV_CURRENT: u32 = 1;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_TLS: u32 = 7;
const PT_GNU_RELRO: u32 = 0x6474_e552;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

const DT_NULL: i64 = 0;
const DT_NEEDED: i64 = 1;
const DT_PLTRELSZ: i64 = 2;
const DT_PLTGOT: i64 = 3;
const DT_HASH: i64 = 4;
const DT_STRTAB: i64 = 5;
const DT_SYMTAB: i64 = 6;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;
const DT_STRSZ: i64 = 10;
const DT_SYMENT: i64 = 11;
const DT_INIT: i64 = 12;
const DT_FINI: i64 = 13;
const DT_SONAME: i64 = 14;
const DT_RPATH: i64 = 15;
const DT_SYMBOLIC: i64 = 16;
const DT_REL: i64 = 17;
const DT_RELSZ: i64 = 18;
const DT_RELENT: i64 = 19;
const DT_PLTREL: i64 = 20;
const DT_DEBUG: i64 = 21;
const DT_TEXTREL: i64 = 22;
const DT_JMPREL: i64 = 23;
const DT_BIND_NOW: i64 = 24;
const DT_INIT_ARRAY: i64 = 25;
const DT_FINI_ARRAY: i64 = 26;
const DT_INIT_ARRAYSZ: i64 = 27;
const DT_FINI_ARRAYSZ: i64 = 28;
const DT_RUNPATH: i64 = 29;
const DT_FLAGS: i64 = 30;
const DT_PREINIT_ARRAY: i64 = 32;
const DT_PREINIT_ARRAYSZ: i64 = 33;
const DT_RELR: i64 = 36;
const DT_RELRSZ: i64 = 35;
const DT_RELRENT: i64 = 37;
const DT_ANDROID_REL: i64 = 0x6000_000f;
const DT_ANDROID_RELSZ: i64 = 0x6000_0010;
const DT_ANDROID_RELA: i64 = 0x6000_0011;
const DT_ANDROID_RELASZ: i64 = 0x6000_0012;
const DT_ANDROID_RELR: i64 = 0x6fff_e000;
const DT_ANDROID_RELRSZ: i64 = 0x6fff_e001;
const DT_ANDROID_RELRENT: i64 = 0x6fff_e003;
const DT_ANDROID_RELRCOUNT: i64 = 0x6fff_e005;
const DT_GNU_HASH: i64 = 0x6fff_fef5;
const DT_RELACOUNT: i64 = 0x6fff_fff9;
const DT_FLAGS_1: i64 = 0x6fff_fffb;
const DT_VERSYM: i64 = 0x6fff_fff0;
const DT_VERDEF: i64 = 0x6fff_fffc;
const DT_VERDEFNUM: i64 = 0x6fff_fffd;
const DT_VERNEED: i64 = 0x6fff_fffe;
const DT_VERNEEDNUM: i64 = 0x6fff_ffff;
// AArch64 ELF uses this zero-valued presence tag to state that PLT entries are
// compatible with Branch Target Identification. It does not request the
// loader to enable a process feature or rewrite the PLT.
const DT_AARCH64_BTI_PLT: i64 = 0x7000_0001;

const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_GLOB_DAT: u32 = 1025;
const R_AARCH64_JUMP_SLOT: u32 = 1026;
const R_AARCH64_RELATIVE: u32 = 1027;
const R_AARCH64_TLSDESC: u32 = 1031;
const R_AARCH64_IRELATIVE: u32 = 1032;
const DF_BIND_NOW: u64 = 0x8;
const DF_SYMBOLIC: u64 = 0x2;
const DF_1_NOW: u64 = 0x1;
const DF_1_GLOBAL: u64 = 0x2;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;
const STT_NOTYPE: u8 = 0;
const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;
const STT_TLS: u8 = 6;
const STV_DEFAULT: u8 = 0;
const STV_PROTECTED: u8 = 3;
const VER_NDX_LOCAL: u16 = 0;
const VER_NDX_GLOBAL: u16 = 1;
const VERSYM_HIDDEN: u16 = 0x8000;
const VER_FLG_BASE: u16 = 0x1;
const VER_FLG_WEAK: u16 = 0x2;

const PROT_NONE: c_int = 0;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const PROT_EXEC: c_int = 4;
const MAP_PRIVATE: c_int = 0x0002;
const MAP_ANON: c_int = 0x1000;
const MAX_IMAGE_SIZE: usize = 1024 * 1024 * 1024;

unsafe extern "C" {
    fn getpagesize() -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        file_descriptor: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn mprotect(address: *mut c_void, length: usize, protection: c_int) -> c_int;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
    fn sys_icache_invalidate(start: *mut c_void, length: usize);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Capability {
    HostArchitecture,
    DirectSyscall,
    DirectThreadPointer,
    Tls,
    Relro,
    LazyBinding,
    RelRelocations,
    RelrRelocations,
    UnsupportedRelocation { relocation_type: u32, symbol: u32 },
    DynamicInitializer,
    Finalizers,
    PreinitArray,
    TextRelocations,
    Rpath,
    UnknownDynamicTag(i64),
    MissingSysvHash,
    SymbolicLookup,
    VersionDefinitions,
    AbsoluteSymbolDefinition,
    DynamicFlags { tag: i64, value: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveError {
    UnknownSoname(String),
    VersionMismatch {
        soname: String,
        symbol: String,
        requested: String,
    },
    Rejected(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSoname(soname) => write!(formatter, "unknown dependency SONAME: {soname}"),
            Self::VersionMismatch {
                soname,
                symbol,
                requested,
            } => write!(
                formatter,
                "version mismatch for {symbol}@{requested} from {soname}"
            ),
            Self::Rejected(message) => write!(formatter, "resolver rejected symbol: {message}"),
        }
    }
}

impl StdError for ResolveError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedSymbol {
    address: NonZeroUsize,
}

impl ResolvedSymbol {
    /// Constructs a provider result from a raw host address.
    ///
    /// # Safety
    ///
    /// The address must have the ABI, object/function kind, alignment, and readable/executable
    /// lifetime required by every relocation that names it. It must remain valid until every
    /// `LoadedElf` created with the resolver has been dropped.
    pub unsafe fn new(address: NonZeroUsize) -> Self {
        Self { address }
    }

    pub fn address(self) -> usize {
        self.address.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionRequirement<'a> {
    pub soname: &'a str,
    pub name: &'a str,
    pub hidden: bool,
    pub flags: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct SymbolRequest<'a> {
    pub symbol: &'a str,
    /// Standalone relocation supplies DT_NEEDED. Ordered graph lookup supplies
    /// exactly the selected native image here; a miss must return Ok(None) so
    /// later local-group images can be searched. Do not scan unrelated images.
    pub needed_libraries: &'a [String],
    pub version: Option<VersionRequirement<'a>>,
    /// True for an ELF `STB_WEAK` undefined symbol. A resolver may use this
    /// to decline an optional platform capability without turning absence
    /// into a hard resolver error.
    pub is_weak: bool,
}

/// A closed symbol namespace supplied by the caller.
///
/// The loader never consults `dlsym`, dyld's global namespace, or the host process. Returning
/// `Ok(None)` means the symbol is absent. An absent weak undefined symbol resolves to zero; an
/// absent global undefined symbol fails the load.
pub trait SymbolResolver {
    /// Preempt a default-visible definition within the caller's load group.
    /// Raw ELF bytes are preserved; an explicit version matches that name.
    /// Standalone external import providers do not preempt definitions by
    /// default. A graph resolver must search its complete ordered local group.
    fn resolve_defined(
        &mut self,
        _name: &[u8],
        _version: Option<&str>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        Ok(None)
    }
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError>;
}

#[derive(Debug)]
pub enum LoadError {
    Format(&'static str),
    Bounds(&'static str),
    Capability(Capability),
    Protection(&'static str),
    Integrity(&'static str),
    Resolver {
        symbol: String,
        source: ResolveError,
    },
    UnresolvedSymbol {
        symbol: String,
        soname: Option<String>,
        version: Option<String>,
    },
    SymbolNotFound(String),
    InvalidSymbol(String),
    InitializersAlreadyRun,
    InitializersNotRun,
    TlsInUse {
        active_threads: usize,
    },
    System {
        operation: &'static str,
        code: i32,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(message) => write!(formatter, "invalid ELF: {message}"),
            Self::Bounds(message) => write!(formatter, "ELF bounds error: {message}"),
            Self::Capability(capability) => {
                write!(formatter, "unsupported ELF capability: {capability:?}")
            }
            Self::Protection(message) => write!(formatter, "ELF protection error: {message}"),
            Self::Integrity(message) => write!(formatter, "ELF integrity error: {message}"),
            Self::Resolver { symbol, source } => {
                write!(formatter, "resolver failed for {symbol}: {source}")
            }
            Self::UnresolvedSymbol {
                symbol,
                soname,
                version,
            } => {
                write!(formatter, "unresolved strong symbol: {symbol}")?;
                if let Some(version) = version {
                    write!(formatter, "@{version}")?;
                }
                if let Some(soname) = soname {
                    write!(formatter, " from {soname}")?;
                }
                Ok(())
            }
            Self::SymbolNotFound(name) => write!(formatter, "dynamic symbol not found: {name}"),
            Self::InvalidSymbol(name) => write!(formatter, "invalid dynamic symbol: {name}"),
            Self::InitializersAlreadyRun => write!(formatter, "DT_INIT_ARRAY already executed"),
            Self::InitializersNotRun => write!(formatter, "DT_INIT_ARRAY has not executed"),
            Self::TlsInUse { active_threads } => write!(
                formatter,
                "ELF TLS module still has {active_threads} live thread allocation(s)"
            ),
            Self::System { operation, code } => {
                write!(formatter, "{operation} failed with errno {code}")
            }
        }
    }
}

impl StdError for LoadError {}

fn boringssl_integrity_hmac(text: &[u8], rodata: &[u8]) -> [u8; 32] {
    let inner_pad = [0x36_u8; 64];
    let outer_pad = [0x5c_u8; 64];
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update((text.len() as u64).to_le_bytes());
    inner.update(text);
    inner.update((rodata.len() as u64).to_le_bytes());
    inner.update(rodata);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

#[derive(Clone, Copy, Debug)]
struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: u64,
    virtual_address: u64,
    file_size: u64,
    memory_size: u64,
    alignment: u64,
}

#[derive(Default, Debug)]
struct DynamicInfo {
    needed_offsets: Vec<u64>,
    soname_offset: Option<u64>,
    runpath_offset: Option<u64>,
    hash: Option<u64>,
    gnu_hash: Option<u64>,
    string_table: Option<u64>,
    string_size: Option<u64>,
    symbol_table: Option<u64>,
    symbol_entry_size: Option<u64>,
    rela: Option<u64>,
    rela_size: Option<u64>,
    rela_entry_size: Option<u64>,
    relative_relocation_count: Option<u64>,
    android_rela: Option<u64>,
    android_rela_size: Option<u64>,
    relr: Option<u64>,
    relr_size: Option<u64>,
    relr_entry_size: Option<u64>,
    relr_count: Option<u64>,
    plt_rela: Option<u64>,
    plt_rela_size: Option<u64>,
    plt_relocation_kind: Option<u64>,
    bind_now_tag: bool,
    flags: Option<u64>,
    flags_1: Option<u64>,
    versym: Option<u64>,
    verneed: Option<u64>,
    verneed_count: Option<u64>,
    verdef: Option<u64>,
    verdef_count: Option<u64>,
    init_array: Option<u64>,
    init_array_size: Option<u64>,
    fini: Option<u64>,
    fini_array: Option<u64>,
    fini_array_size: Option<u64>,
    aarch64_bti_plt: Option<u64>,
}

#[derive(Debug)]
struct ParsedImage {
    loads: Vec<ProgramHeader>,
    dynamic: ProgramHeader,
    tls: Option<ProgramHeader>,
    minimum_page: u64,
    image_size: usize,
    reservation_size: usize,
    image_offset: usize,
    compat_layout: bool,
    stack_guard_offset: usize,
    direct_syscall_shim_offset: usize,
    direct_syscall_shim_size: usize,
    page_size: usize,
    page_protections: Vec<c_int>,
}

pub struct LoadedElf {
    reservation: NonNull<u8>,
    reservation_size: usize,
    mapping: NonNull<u8>,
    mapping_size: usize,
    stack_guard: NonNull<u64>,
    stack_guard_rewrites: Vec<(u64, u32)>,
    direct_syscall_shim: NonNull<u8>,
    direct_syscall_shim_size: usize,
    direct_syscall_rewrites: Vec<(u64, u32)>,
    minimum_page: u64,
    loads: Vec<ProgramHeader>,
    dynamic: DynamicInfo,
    needed_libraries: Vec<String>,
    soname: Option<String>,
    initialization: initialization::Initialization,
    finalization: finalization::Finalization,
    finalizers: Vec<usize>,
    dso_lifecycle: Option<Arc<dyn DsoLifecycle>>,
    tls_header: Option<ProgramHeader>,
    tls_module: Option<Arc<TlsModule>>,
    tls_descriptor_tokens: Vec<u64>,
    pending_irelative: Vec<(u64, usize)>,
}

/// Per-image lifecycle boundary for Bionic `__cxa_atexit` registrations.
///
/// A graph publishes the complete, unique mmap reservation before running any
/// constructor. Teardown is called while that reservation is still executable,
/// before ELF fini entries and `munmap`. Implementations must synchronously
/// drain callbacks and stop admitting registrations before returning.
pub trait DsoLifecycle: Send + Sync {
    fn publish_image(&self, range: Range<usize>) -> Result<(), String>;
    fn finalize_image(&self, range: Range<usize>) -> Result<(), String>;
}

struct StagedElf {
    image: LoadedElf,
    page_size: usize,
    page_protections: Vec<c_int>,
}

// SAFETY: LoadedElf exclusively owns an mmap reservation. Guest TLS allocations retain their
// synchronized module owner and teardown aborts rather than unmapping code with live threads.
// Resolver callbacks are not retained and mutation is restricted to &mut self. C consumers
// additionally serialize the value behind a Mutex.
unsafe impl Send for LoadedElf {}

#[derive(Default)]
struct RejectAllResolver;

impl SymbolResolver for RejectAllResolver {
    fn resolve(
        &mut self,
        _request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        Ok(None)
    }
}

impl LoadedElf {
    pub fn load(bytes: &[u8]) -> Result<Self, LoadError> {
        Self::load_with_resolver(bytes, &mut RejectAllResolver)
    }

    pub fn load_with_resolver(
        bytes: &[u8],
        resolver: &mut dyn SymbolResolver,
    ) -> Result<Self, LoadError> {
        let mut staged = Self::stage(bytes)?;
        staged
            .image
            .finish_load(resolver, staged.page_size, &staged.page_protections)?;
        Ok(staged.image)
    }

    fn stage(bytes: &[u8]) -> Result<StagedElf, LoadError> {
        Self::stage_with_appcompat(bytes, None)
    }

    pub(crate) fn stage_with_appcompat(
        bytes: &[u8],
        appcompat_16kb: Option<bool>,
    ) -> Result<StagedElf, LoadError> {
        if !cfg!(target_arch = "aarch64") {
            return Err(LoadError::Capability(Capability::HostArchitecture));
        }
        let parsed = match appcompat_16kb {
            None => parse_image(bytes)?,
            Some(enabled) => parse_image_with_appcompat(bytes, Some(enabled))?,
        };
        let mapping = Mapping::reserve(parsed.reservation_size)?;

        segment_loading::populate(&mapping, &parsed, bytes)?;

        let dynamic = parse_dynamic(bytes, &parsed.dynamic)?;
        validate_dynamic_capabilities(&dynamic)?;
        let tls_module = parsed
            .tls
            .map(|header| TlsModule::new(header, bytes))
            .transpose()?;

        let reservation = mapping.pointer();
        let logical_mapping =
            NonNull::new(unsafe { reservation.as_ptr().add(parsed.image_offset) })
                .expect("offset into a non-null reservation remains non-null");
        let stack_guard = NonNull::new(
            unsafe { reservation.as_ptr().add(parsed.stack_guard_offset + 0x28) }.cast::<u64>(),
        )
        .expect("offset into a non-null reservation remains non-null");
        let direct_syscall_shim =
            NonNull::new(unsafe { reservation.as_ptr().add(parsed.direct_syscall_shim_offset) })
                .expect("offset into a non-null reservation remains non-null");
        let mut guard_hasher = RandomState::new().build_hasher();
        guard_hasher.write_usize(stack_guard.as_ptr() as usize);
        let guard = guard_hasher.finish() | 1;
        // SAFETY: the dedicated compatibility page is writable during staging and aligned.
        unsafe { stack_guard.as_ptr().write(guard) };
        let mut loaded = Self {
            reservation,
            reservation_size: mapping.length(),
            mapping: logical_mapping,
            mapping_size: parsed.image_size,
            stack_guard,
            stack_guard_rewrites: Vec::new(),
            direct_syscall_shim,
            direct_syscall_shim_size: parsed.direct_syscall_shim_size,
            direct_syscall_rewrites: Vec::new(),
            minimum_page: parsed.minimum_page,
            loads: parsed.loads,
            dynamic,
            needed_libraries: Vec::new(),
            soname: None,
            initialization: initialization::Initialization::default(),
            finalization: finalization::Finalization::default(),
            finalizers: Vec::new(),
            dso_lifecycle: None,
            tls_header: parsed.tls,
            tls_module,
            tls_descriptor_tokens: Vec::new(),
            pending_irelative: Vec::new(),
        };
        std::mem::forget(mapping);

        loaded.materialize_dynamic_names()?;
        Ok(StagedElf {
            image: loaded,
            page_size: parsed.page_size,
            page_protections: parsed.page_protections,
        })
    }

    fn finish_load(
        &mut self,
        resolver: &mut dyn SymbolResolver,
        page_size: usize,
        page_protections: &[c_int],
    ) -> Result<(), LoadError> {
        self.rewrite_android_stack_guard_tls()?;
        self.rewrite_direct_android_syscalls(resolver)?;
        if self.soname() == Some("libcrypto.so")
            && std::env::var_os("DARWIN_ART_ELF_PREFLIGHT_BORINGSSL_BOUNDS").is_some()
        {
            let destination = self.loaded_pointer(0xd35b4, 8)?;
            // Diagnostic-only: replace the integrity function with a known-returning leaf.
            unsafe {
                ptr::write_unaligned(destination.cast::<u32>(), 0x5280_0020);
                ptr::write_unaligned(destination.add(4).cast::<u32>(), 0xd65f_03c0);
            }
        }
        self.validate_symbols_and_apply_relocations(resolver)?;
        self.apply_irelative_relocations(page_size, page_protections)?;
        self.refresh_boringssl_integrity_after_rewrite()?;
        self.refresh_tls_template()?;
        self.validate_initializer_entries()?;
        self.prepare_finalizers()?;
        self.apply_final_protections(page_size, page_protections)
    }

    pub fn needed_libraries(&self) -> &[String] {
        &self.needed_libraries
    }

    fn refresh_boringssl_integrity_after_rewrite(&mut self) -> Result<(), LoadError> {
        let has_fips_hash = self
            .exported_symbols()?
            .iter()
            .any(|symbol| symbol.name == b"FIPS_module_hash");
        if !has_fips_hash {
            return Ok(());
        }

        let text_start = self.exported_address("BORINGSSL_bcm_text_start")?;
        let text_end = self.exported_address("BORINGSSL_bcm_text_end")?;
        let rodata_start = self.exported_address("BORINGSSL_bcm_rodata_start")?;
        let rodata_end = self.exported_address("BORINGSSL_bcm_rodata_end")?;
        let text = self.host_slice(text_start, text_end, Some(PF_X), "BoringSSL text span")?;
        let rodata = self.host_slice(
            rodata_start,
            rodata_end,
            Some(PF_R),
            "BoringSSL rodata span",
        )?;

        let hash_function = self.resolve_function("FIPS_module_hash")?;
        self.host_span_to_virtual(
            hash_function,
            12,
            Some(PF_X),
            "BoringSSL FIPS hash accessor",
        )?;
        let accessor = unsafe { std::slice::from_raw_parts(hash_function as *const u8, 12) };
        let adrp = u32::from_le_bytes(accessor[0..4].try_into().unwrap());
        let ldr = u32::from_le_bytes(accessor[4..8].try_into().unwrap());
        let ret = u32::from_le_bytes(accessor[8..12].try_into().unwrap());
        if adrp & 0x9f00_001f != 0x9000_0000
            || ldr & 0xffc0_03ff != 0xf940_0000
            || ret != 0xd65f_03c0
        {
            return Err(LoadError::Integrity(
                "BoringSSL FIPS hash accessor has an unsupported shape",
            ));
        }
        let immediate = u64::from((adrp >> 29) & 0x3) | (u64::from((adrp >> 5) & 0x7ffff) << 2);
        let signed_pages = if immediate & (1 << 20) != 0 {
            i64::try_from(immediate).unwrap() - (1 << 21)
        } else {
            i64::try_from(immediate).unwrap()
        };
        let function_page = hash_function & !0xfff;
        let got_page = (function_page as i128)
            .checked_add(i128::from(signed_pages) << 12)
            .and_then(|address| usize::try_from(address).ok())
            .ok_or(LoadError::Bounds("BoringSSL FIPS hash GOT page"))?;
        let got_address = got_page
            .checked_add(usize::try_from((ldr >> 10) & 0xfff).unwrap() * 8)
            .ok_or(LoadError::Bounds("BoringSSL FIPS hash GOT slot"))?;
        self.host_span_to_virtual(got_address, 8, Some(PF_R), "BoringSSL FIPS hash GOT slot")?;
        let expected_pointer = unsafe { ptr::read_unaligned(got_address as *const *mut u8) };
        if expected_pointer.is_null() {
            return Err(LoadError::Integrity("BoringSSL FIPS hash pointer is null"));
        }
        let expected_address = expected_pointer as usize;
        self.host_span_to_virtual(expected_address, 32, Some(PF_R), "BoringSSL FIPS hash")?;
        let expected = unsafe { std::slice::from_raw_parts(expected_pointer, 32) };

        let mut original_text = text.to_vec();
        for &(virtual_address, instruction) in self
            .stack_guard_rewrites
            .iter()
            .chain(self.direct_syscall_rewrites.iter())
        {
            let host = self.loaded_pointer(virtual_address, 4)? as usize;
            if host < text_start || host.checked_add(4).is_none_or(|end| end > text_end) {
                continue;
            }
            let offset = host - text_start;
            original_text[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
        }
        let original_digest = boringssl_integrity_hmac(&original_text, rodata);
        if expected != original_digest {
            return Err(LoadError::Integrity(
                "BoringSSL FIPS hash did not authenticate the original Android image",
            ));
        }

        if !self.stack_guard_rewrites.is_empty() || !self.direct_syscall_rewrites.is_empty() {
            let transformed_digest = boringssl_integrity_hmac(text, rodata);
            // SAFETY: staging keeps every populated PT_LOAD page writable until final
            // protections are applied, and the validated hash span is 32 bytes long.
            unsafe {
                ptr::copy_nonoverlapping(
                    transformed_digest.as_ptr(),
                    expected_pointer,
                    transformed_digest.len(),
                )
            };
        }
        Ok(())
    }

    fn exported_address(&self, name: &str) -> Result<usize, LoadError> {
        self.exported_address_bytes(name.as_bytes())
    }

    fn exported_address_bytes(&self, name: &[u8]) -> Result<usize, LoadError> {
        let symbols = self.exported_symbols()?;
        find_exported_symbol(&symbols, name)
            .map(|symbol| symbol.address)
            .ok_or_else(|| LoadError::SymbolNotFound(String::from_utf8_lossy(name).into_owned()))
    }

    fn host_span_to_virtual(
        &self,
        start: usize,
        size: usize,
        required_flag: Option<u32>,
        what: &'static str,
    ) -> Result<u64, LoadError> {
        let offset = start
            .checked_sub(self.mapping.as_ptr() as usize)
            .ok_or(LoadError::Bounds(what))?;
        let virtual_address = self
            .minimum_page
            .checked_add(u64::try_from(offset).map_err(|_| LoadError::Bounds(what))?)
            .ok_or(LoadError::Bounds(what))?;
        self.require_loaded_range(
            virtual_address,
            u64::try_from(size).map_err(|_| LoadError::Bounds(what))?,
            required_flag,
            what,
        )?;
        Ok(virtual_address)
    }

    fn host_slice(
        &self,
        start: usize,
        end: usize,
        required_flag: Option<u32>,
        what: &'static str,
    ) -> Result<&[u8], LoadError> {
        let size = end.checked_sub(start).ok_or(LoadError::Bounds(what))?;
        self.host_span_to_virtual(start, size, required_flag, what)?;
        Ok(unsafe { std::slice::from_raw_parts(start as *const u8, size) })
    }

    pub fn soname(&self) -> Option<&str> {
        self.soname.as_deref()
    }

    /// Returns the mapped address of a defined dynamic export.
    ///
    /// The address remains valid only while this `LoadedElf` is alive. Calling it is unsafe in
    /// the general case: the caller must know the exact ABI and must run required initializers
    /// first. This method performs symbol visibility/type and executable-range validation only.
    pub fn lookup_exported(&self, name: &str) -> Result<usize, LoadError> {
        self.lookup_exported_bytes(name.as_bytes())
    }

    pub(crate) fn lookup_exported_bytes(&self, name: &[u8]) -> Result<usize, LoadError> {
        self.resolve_function_bytes(name)
    }

    pub(crate) fn lookup_any_exported_bytes(&self, name: &[u8]) -> Result<usize, LoadError> {
        self.exported_address_bytes(name)
    }

    /// Returns the image's parsed GNU version definitions for Android's
    /// dlsym/dlvsym matching policy. `None` means the image has no DT_VERSYM;
    /// an empty map means DT_VERSYM is present but no named definitions were
    /// supplied, so an unknown request still falls back to VER_NDX_GLOBAL.
    pub(crate) fn android_version_definitions(
        &self,
    ) -> Result<Option<HashMap<u16, String>>, LoadError> {
        if self.dynamic.versym.is_none() {
            return Ok(None);
        }
        let definitions = self.parse_version_definitions(self.symbol_count()?)?;
        Ok(Some(
            definitions
                .into_iter()
                .map(|(index, definition)| (index, definition.name))
                .collect(),
        ))
    }

    pub(crate) fn debug_mapped_pointer(&self, virtual_address: u64) -> Result<usize, LoadError> {
        Ok(self.loaded_pointer(virtual_address, 1)? as usize)
    }

    pub(crate) fn debug_read_mapped_u64(&self, virtual_address: u64) -> Result<u64, LoadError> {
        self.read_loaded_u64(virtual_address)
    }

    pub(crate) fn debug_stack_guard(&self) -> (usize, u64) {
        // SAFETY: the loader-owned guard page remains mapped for the image lifetime.
        (self.stack_guard.as_ptr() as usize, unsafe {
            self.stack_guard.as_ptr().read()
        })
    }

    fn mapping_range(&self) -> Range<usize> {
        let start = self.mapping.as_ptr() as usize;
        start..start + self.mapping_size
    }

    fn publish_dso_lifecycle(&mut self, lifecycle: Arc<dyn DsoLifecycle>) -> Result<(), String> {
        lifecycle.publish_image(self.mapping_range())?;
        self.dso_lifecycle = Some(lifecycle);
        Ok(())
    }

    /// Consumes this image, running its finalizers (when initialized) before unmapping it.
    ///
    /// Dropping the image has the same lifecycle semantics. The consuming API exists so callers
    /// can make an intentional unload point explicit without introducing a second owner state.
    pub fn close(self) {}

    fn validate_initializer_entries(&self) -> Result<(), LoadError> {
        let address = self.dynamic.init_array.unwrap_or(0);
        let size = self.dynamic.init_array_size.unwrap_or(0);
        if (address == 0) != (size == 0) || size % 8 != 0 {
            return Err(LoadError::Format("invalid DT_INIT_ARRAY pair"));
        }
        if size == 0 {
            return Ok(());
        }
        self.require_loaded_range(address, size, None, "DT_INIT_ARRAY")?;
        for index in 0..(size / 8) {
            let entry_address = address
                .checked_add(index * 8)
                .ok_or(LoadError::Bounds("DT_INIT_ARRAY entry overflow"))?;
            let pointer = self.read_loaded_u64(entry_address)?;
            if pointer != 0 && pointer != u64::MAX {
                self.require_host_executable(pointer, "DT_INIT_ARRAY function")?;
            }
        }
        Ok(())
    }

    fn prepare_finalizers(&mut self) -> Result<(), LoadError> {
        let address = self.dynamic.fini_array.unwrap_or(0);
        let size = self.dynamic.fini_array_size.unwrap_or(0);
        if (address == 0) != (size == 0) || size % 8 != 0 {
            return Err(LoadError::Format("invalid DT_FINI_ARRAY pair"));
        }

        let mut finalizers = Vec::new();
        if size != 0 {
            self.require_loaded_range(address, size, None, "DT_FINI_ARRAY")?;
            for index in (0..(size / 8)).rev() {
                let entry_address = address
                    .checked_add(index * 8)
                    .ok_or(LoadError::Bounds("DT_FINI_ARRAY entry overflow"))?;
                let pointer = self.read_loaded_u64(entry_address)?;
                if pointer == 0 || pointer == u64::MAX {
                    continue;
                }
                self.require_host_executable(pointer, "DT_FINI_ARRAY function")?;
                finalizers.push(
                    usize::try_from(pointer).map_err(|_| LoadError::Bounds("finalizer pointer"))?,
                );
            }
        }

        if let Some(address) = self.dynamic.fini.filter(|address| *address != 0) {
            self.require_loaded_range(address, 1, Some(PF_X), "DT_FINI function")?;
            finalizers.push(self.loaded_pointer(address, 1)? as usize);
        }
        self.finalizers = finalizers;
        Ok(())
    }

    pub fn call_exported_i32(&self, name: &str) -> Result<i32, LoadError> {
        if !self.initialization.complete() {
            return Err(LoadError::InitializersNotRun);
        }
        self.call_exported_i32_before_initializers(name)
    }

    pub(crate) fn call_exported_i32_before_initializers(
        &self,
        name: &str,
    ) -> Result<i32, LoadError> {
        let pointer = self.resolve_function(name)?;
        // SAFETY: resolve_function validates a defined function symbol inside executable
        // mapped memory. This API deliberately supports only the no-argument/i32 ABI slice.
        let function: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(pointer) };
        Ok(unsafe { function() })
    }

    fn materialize_dynamic_names(&mut self) -> Result<(), LoadError> {
        self.needed_libraries = self
            .dynamic
            .needed_offsets
            .iter()
            .map(|offset| {
                u32::try_from(*offset)
                    .map_err(|_| LoadError::Bounds("DT_NEEDED string offset"))
                    .and_then(|offset| self.dynamic_string(offset))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.soname = self
            .dynamic
            .soname_offset
            .map(|offset| {
                u32::try_from(offset)
                    .map_err(|_| LoadError::Bounds("DT_SONAME string offset"))
                    .and_then(|offset| self.dynamic_string(offset))
            })
            .transpose()?;
        if self.needed_libraries.iter().any(String::is_empty) {
            return Err(LoadError::Format("empty DT_NEEDED string"));
        }
        if self.soname.as_ref().is_some_and(String::is_empty) {
            return Err(LoadError::Format("empty DT_SONAME string"));
        }
        Ok(())
    }

    fn validate_symbols_and_apply_relocations(
        &mut self,
        resolver: &mut dyn SymbolResolver,
    ) -> Result<(), LoadError> {
        let symbol_count = self.symbol_count()?;
        for index in 1..symbol_count {
            let symbol = self.dynamic_symbol(index)?;
            if symbol.kind == STT_TLS {
                if symbol.section_index == SHN_UNDEF {
                    // Cross-module TLS needs graph-wide module indexing. This bounded slice owns
                    // only definitions in this image and deliberately fails imported TLS closed.
                    return Err(LoadError::Capability(Capability::Tls));
                }
                self.validate_defined_tls_symbol(&symbol)?;
            }
        }

        // Validate provider-side GNU version metadata even when this image is loaded outside a
        // graph and therefore does not need an export catalog.
        self.parse_version_definitions(symbol_count)?;
        let versions = self.parse_version_requirements(symbol_count)?;
        let mut resolved = HashMap::new();
        let relr_address = self.dynamic.relr.unwrap_or(0);
        let relr_size = self.dynamic.relr_size.unwrap_or(0);
        if relr_size != 0 {
            self.apply_relr_table(relr_address, relr_size)?;
        }

        let android_rela_address = self.dynamic.android_rela.unwrap_or(0);
        let android_rela_size = self.dynamic.android_rela_size.unwrap_or(0);
        if android_rela_size != 0 {
            self.apply_android_packed_relocations(
                android_rela_address,
                android_rela_size,
                symbol_count,
                &versions,
                &mut resolved,
                resolver,
            )?;
        }
        let rela_address = self.dynamic.rela.unwrap_or(0);
        let rela_size = self.dynamic.rela_size.unwrap_or(0);
        if (rela_address == 0) != (rela_size == 0) {
            return Err(LoadError::Format("incomplete DT_RELA metadata"));
        }
        if rela_size != 0 {
            self.apply_relocation_table(
                rela_address,
                rela_size,
                false,
                self.dynamic.relative_relocation_count.unwrap_or(0),
                symbol_count,
                &versions,
                &mut resolved,
                resolver,
            )?;
        }

        let plt_address = self.dynamic.plt_rela.unwrap_or(0);
        let plt_size = self.dynamic.plt_rela_size.unwrap_or(0);
        if (plt_address == 0) != (plt_size == 0) {
            return Err(LoadError::Format("incomplete PLT relocation metadata"));
        }
        if plt_size != 0 {
            let rela_end = rela_address
                .checked_add(rela_size)
                .ok_or(LoadError::Bounds("DT_RELA range overflow"))?;
            let plt_end = plt_address
                .checked_add(plt_size)
                .ok_or(LoadError::Bounds("DT_JMPREL range overflow"))?;
            if rela_size != 0 && rela_address < plt_end && plt_address < rela_end {
                return Err(LoadError::Format("DT_RELA and DT_JMPREL overlap"));
            }
            self.apply_relocation_table(
                plt_address,
                plt_size,
                true,
                0,
                symbol_count,
                &versions,
                &mut resolved,
                resolver,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_relocation_table(
        &mut self,
        table_address: u64,
        table_size: u64,
        plt_only: bool,
        relative_prefix_count: u64,
        symbol_count: u32,
        versions: &HashMap<u16, OwnedVersionRequirement>,
        resolved: &mut HashMap<u32, usize>,
        resolver: &mut dyn SymbolResolver,
    ) -> Result<(), LoadError> {
        let entry_size = self
            .dynamic
            .rela_entry_size
            .unwrap_or(ELF64_RELA_SIZE as u64);
        if entry_size != ELF64_RELA_SIZE as u64 || table_size % entry_size != 0 {
            return Err(LoadError::Format(
                "invalid DT_RELAENT/relocation table size",
            ));
        }
        let count = table_size / entry_size;
        if relative_prefix_count > count {
            return Err(LoadError::Format("DT_RELACOUNT exceeds DT_RELASZ"));
        }
        self.require_loaded_range(table_address, table_size, Some(PF_R), "RELA table")?;
        for index in 0..count {
            let address = table_address
                .checked_add(index * entry_size)
                .ok_or(LoadError::Bounds("DT_RELA entry overflow"))?;
            let offset = self.read_loaded_u64(address)?;
            let info = self.read_loaded_u64(address + 8)?;
            let addend = self.read_loaded_i64(address + 16)?;
            let relocation_type = info as u32;
            let symbol_index = (info >> 32) as u32;
            if index < relative_prefix_count
                && (relocation_type != R_AARCH64_RELATIVE || symbol_index != 0)
            {
                return Err(LoadError::Format(
                    "DT_RELACOUNT prefix contains non-relative relocation",
                ));
            }
            self.apply_one_relocation(
                offset,
                info,
                addend,
                plt_only,
                symbol_count,
                versions,
                resolved,
                resolver,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_android_packed_relocations(
        &mut self,
        table_address: u64,
        table_size: u64,
        symbol_count: u32,
        versions: &HashMap<u16, OwnedVersionRequirement>,
        resolved: &mut HashMap<u32, usize>,
        resolver: &mut dyn SymbolResolver,
    ) -> Result<(), LoadError> {
        self.require_loaded_range(
            table_address,
            table_size,
            Some(PF_R),
            "Android packed RELA table",
        )?;
        let bytes = self
            .loaded_slice(table_address, to_usize(table_size, "DT_ANDROID_RELASZ")?)?
            .to_vec();
        let relocations =
            packed_relocations::decode_aps2_rela(&bytes).map_err(LoadError::Format)?;
        for relocation in relocations {
            self.apply_one_relocation(
                relocation.offset,
                relocation.info,
                relocation.addend,
                false,
                symbol_count,
                versions,
                resolved,
                resolver,
            )?;
        }
        Ok(())
    }

    fn apply_relr_table(&mut self, table_address: u64, table_size: u64) -> Result<(), LoadError> {
        self.require_loaded_range(table_address, table_size, Some(PF_R), "RELR table")?;
        let count = table_size / 8;
        let mut entries = Vec::with_capacity(
            usize::try_from(count).map_err(|_| LoadError::Bounds("RELR entry count"))?,
        );
        for index in 0..count {
            entries.push(
                self.read_loaded_u64(
                    table_address
                        .checked_add(index * 8)
                        .ok_or(LoadError::Bounds("RELR entry overflow"))?,
                )?,
            );
        }
        let mut cursor = None;
        let mut applied = 0_u64;
        for entry in entries {
            if entry & 1 == 0 {
                self.apply_relr_target(entry)?;
                applied = applied
                    .checked_add(1)
                    .ok_or(LoadError::Bounds("RELR relocation count"))?;
                cursor = Some(
                    entry
                        .checked_add(8)
                        .ok_or(LoadError::Bounds("RELR cursor overflow"))?,
                );
                continue;
            }
            let base = cursor.ok_or(LoadError::Format("RELR bitmap precedes address"))?;
            let bitmap = entry >> 1;
            for bit in 0..63_u64 {
                if bitmap & (1_u64 << bit) == 0 {
                    continue;
                }
                let target = base
                    .checked_add(bit * 8)
                    .ok_or(LoadError::Bounds("RELR bitmap target overflow"))?;
                self.apply_relr_target(target)?;
                applied = applied
                    .checked_add(1)
                    .ok_or(LoadError::Bounds("RELR relocation count"))?;
            }
            cursor = Some(
                base.checked_add(63 * 8)
                    .ok_or(LoadError::Bounds("RELR bitmap cursor overflow"))?,
            );
        }
        if self
            .dynamic
            .relr_count
            .is_some_and(|expected| expected != applied)
        {
            return Err(LoadError::Format("DT_ANDROID_RELRCOUNT mismatch"));
        }
        Ok(())
    }

    fn apply_relr_target(&mut self, target: u64) -> Result<(), LoadError> {
        if target % 8 != 0 {
            return Err(LoadError::Format("unaligned RELR target"));
        }
        self.require_loaded_range(target, 8, Some(PF_W), "RELR target")?;
        let addend = self.read_loaded_u64(target)?;
        let bias = self.mapping.as_ptr() as i128 - self.minimum_page as i128;
        let value = i128::from(addend)
            .checked_add(bias)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(LoadError::Bounds("RELR value overflow"))?;
        let destination = self.loaded_pointer(target, 8)?;
        // SAFETY: the target is an eight-byte writable mapped-image slot.
        unsafe { ptr::write_unaligned(destination.cast::<usize>(), value) };
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_one_relocation(
        &mut self,
        offset: u64,
        info: u64,
        addend: i64,
        plt_only: bool,
        symbol_count: u32,
        versions: &HashMap<u16, OwnedVersionRequirement>,
        resolved: &mut HashMap<u32, usize>,
        resolver: &mut dyn SymbolResolver,
    ) -> Result<(), LoadError> {
        let relocation_type = info as u32;
        let symbol_index = (info >> 32) as u32;
        if plt_only && !matches!(relocation_type, R_AARCH64_JUMP_SLOT | R_AARCH64_IRELATIVE) {
            return Err(LoadError::Capability(Capability::UnsupportedRelocation {
                relocation_type,
                symbol: symbol_index,
            }));
        }
        if relocation_type == R_AARCH64_TLSDESC {
            if plt_only {
                return Err(LoadError::Capability(Capability::UnsupportedRelocation {
                    relocation_type,
                    symbol: symbol_index,
                }));
            }
            return self.apply_tlsdesc_relocation(offset, symbol_index, symbol_count, addend);
        }
        if relocation_type == R_AARCH64_IRELATIVE {
            if symbol_index != 0 {
                return Err(LoadError::Format(
                    "R_AARCH64_IRELATIVE must be a symbol-free relocation",
                ));
            }
            self.require_loaded_range(offset, 8, Some(PF_W), "R_AARCH64_IRELATIVE target")?;
            let resolver = self.load_bias_add(addend)?;
            self.require_host_executable(
                u64::try_from(resolver)
                    .map_err(|_| LoadError::Bounds("R_AARCH64_IRELATIVE resolver"))?,
                "R_AARCH64_IRELATIVE resolver",
            )?;
            self.pending_irelative.push((offset, resolver));
            return Ok(());
        }
        self.require_loaded_range(offset, 8, Some(PF_W), "RELA target")?;
        let value = match relocation_type {
            R_AARCH64_RELATIVE if symbol_index == 0 => self.load_bias_add(addend)?,
            R_AARCH64_ABS64 | R_AARCH64_GLOB_DAT | R_AARCH64_JUMP_SLOT => {
                if symbol_index == 0 && relocation_type != R_AARCH64_ABS64 {
                    return Err(LoadError::Format(
                        "GLOB_DAT/JUMP_SLOT relocation has STN_UNDEF symbol index",
                    ));
                }
                let symbol_address = self.resolve_relocation_symbol(
                    symbol_index,
                    symbol_count,
                    versions,
                    resolved,
                    resolver,
                )?;
                checked_signed_add(symbol_address, addend, "symbol relocation value")?
            }
            _ => {
                return Err(LoadError::Capability(Capability::UnsupportedRelocation {
                    relocation_type,
                    symbol: symbol_index,
                }));
            }
        };
        let destination = self.loaded_pointer(offset, 8)?;
        // SAFETY: require_loaded_range proves an eight-byte writable destination.
        unsafe { ptr::write_unaligned(destination.cast::<usize>(), value) };
        Ok(())
    }

    fn apply_irelative_relocations(
        &mut self,
        page_size: usize,
        page_protections: &[c_int],
    ) -> Result<(), LoadError> {
        if self.pending_irelative.is_empty() {
            return Ok(());
        }

        // Android resolves IRELATIVE after ordinary relocations. Staging pages are RW, so make
        // only executable runs RX before calling guest resolvers; GOT/data stays writable until
        // the normal final-protection pass.
        let mut start = 0;
        while start < page_protections.len() {
            if page_protections[start] & PROT_EXEC == 0 {
                start += 1;
                continue;
            }
            let mut end = start + 1;
            while end < page_protections.len() && page_protections[end] & PROT_EXEC != 0 {
                end += 1;
            }
            let address = unsafe { self.reservation.as_ptr().add(start * page_size) };
            let length = (end - start) * page_size;
            unsafe { sys_icache_invalidate(address.cast(), length) };
            if unsafe { mprotect(address.cast(), length, PROT_READ | PROT_EXEC) } != 0 {
                return Err(system_error("mprotect(IRELATIVE resolver)"));
            }
            start = end;
        }

        let relocations = std::mem::take(&mut self.pending_irelative);
        for (target, resolver) in relocations {
            // SAFETY: the address is in this image's executable PT_LOAD, ordinary relocations
            // are complete, and ELF specifies the resolver ABI as no arguments -> address.
            let function: unsafe extern "C" fn() -> usize =
                unsafe { std::mem::transmute(resolver) };
            let implementation = unsafe { function() };
            if implementation == 0 {
                return Err(LoadError::Format(
                    "R_AARCH64_IRELATIVE resolver returned null",
                ));
            }
            let destination = self.loaded_pointer(target, 8)?;
            unsafe { ptr::write_unaligned(destination.cast::<usize>(), implementation) };
        }
        Ok(())
    }

    fn validate_defined_tls_symbol(&self, symbol: &DynamicSymbol) -> Result<(), LoadError> {
        let module = self
            .tls_module
            .as_ref()
            .ok_or(LoadError::Format("STT_TLS definition without PT_TLS"))?;
        let start = usize::try_from(symbol.value)
            .map_err(|_| LoadError::Bounds("STT_TLS symbol offset"))?;
        let size = usize::try_from(symbol.size.max(1))
            .map_err(|_| LoadError::Bounds("STT_TLS symbol size"))?;
        if start
            .checked_add(size)
            .is_none_or(|end| end > module.memory_size)
        {
            return Err(LoadError::Bounds("STT_TLS symbol outside PT_TLS"));
        }
        Ok(())
    }

    fn apply_tlsdesc_relocation(
        &mut self,
        target: u64,
        symbol_index: u32,
        symbol_count: u32,
        addend: i64,
    ) -> Result<(), LoadError> {
        if target % 8 != 0 {
            return Err(LoadError::Format("unaligned R_AARCH64_TLSDESC target"));
        }
        self.require_loaded_range(target, 16, Some(PF_W), "R_AARCH64_TLSDESC target")?;
        let module = self
            .tls_module
            .as_ref()
            .cloned()
            .ok_or(LoadError::Format("R_AARCH64_TLSDESC without PT_TLS"))?;
        let base = if symbol_index == 0 {
            0_i128
        } else {
            if symbol_index >= symbol_count {
                return Err(LoadError::Bounds("TLS relocation symbol index"));
            }
            let symbol = self.dynamic_symbol(symbol_index)?;
            if symbol.kind != STT_TLS
                || symbol.section_index == SHN_UNDEF
                || symbol.section_index == SHN_ABS
            {
                return Err(LoadError::InvalidSymbol(format!(
                    "dynsym[{symbol_index}] is not a local TLS definition"
                )));
            }
            self.validate_defined_tls_symbol(&symbol)?;
            i128::from(symbol.value)
        };
        let offset = base
            .checked_add(i128::from(addend))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(LoadError::Bounds("R_AARCH64_TLSDESC offset overflow"))?;
        if offset >= module.memory_size {
            return Err(LoadError::Bounds("R_AARCH64_TLSDESC outside PT_TLS"));
        }
        let destination = self.loaded_pointer(target, 16)?.cast::<TlsDescriptor>();
        let token = register_tls_descriptor(TlsDescriptorContext {
            module,
            offset,
            descriptor_address: destination as usize,
            thread_pointer_base: (self.stack_guard.as_ptr() as usize) & !0xfff,
        })?;
        let descriptor = TlsDescriptor {
            resolver: darwin_art_tlsdesc_resolver as usize,
            token,
        };
        // SAFETY: target is a validated writable pair of 8-byte GOT entries. The second word is
        // an opaque integer; Rust resolves it only through the checked process registry.
        unsafe { ptr::write_unaligned(destination, descriptor) };
        self.tls_descriptor_tokens.push(token);
        Ok(())
    }

    fn refresh_tls_template(&self) -> Result<(), LoadError> {
        let (Some(header), Some(module)) = (self.tls_header, self.tls_module.as_ref()) else {
            return Ok(());
        };
        if header.file_size == 0 {
            return Ok(());
        }
        let template = self.loaded_slice(
            header.virtual_address,
            to_usize(header.file_size, "PT_TLS file size")?,
        )?;
        let mut owned = module
            .template
            .lock()
            .map_err(|_| LoadError::Protection("poisoned TLS template"))?;
        owned.clear();
        owned.extend_from_slice(template);
        Ok(())
    }

    fn parse_version_requirements(
        &self,
        symbol_count: u32,
    ) -> Result<HashMap<u16, OwnedVersionRequirement>, LoadError> {
        let mut result = HashMap::new();
        let (Some(mut current), Some(count)) = (self.dynamic.verneed, self.dynamic.verneed_count)
        else {
            if self.dynamic.verneed.is_some() || self.dynamic.verneed_count.is_some() {
                return Err(LoadError::Format("incomplete DT_VERNEED metadata"));
            }
            if let Some(versym) = self.dynamic.versym {
                self.require_loaded_range(
                    versym,
                    u64::from(symbol_count) * 2,
                    Some(PF_R),
                    "DT_VERSYM",
                )?;
            }
            return Ok(result);
        };
        let versym = self
            .dynamic
            .versym
            .ok_or(LoadError::Format("DT_VERNEED requires DT_VERSYM"))?;
        if count == 0 {
            return Err(LoadError::Format("DT_VERNEEDNUM is zero"));
        }
        self.require_loaded_range(versym, u64::from(symbol_count) * 2, Some(PF_R), "DT_VERSYM")?;
        let mut visited = HashSet::new();
        for requirement_index in 0..count {
            if !visited.insert(current) {
                return Err(LoadError::Format("cyclic DT_VERNEED list"));
            }
            self.require_loaded_range(current, 16, Some(PF_R), "Elf64_Verneed")?;
            let version = self.read_loaded_u16(current)?;
            let auxiliary_count = self.read_loaded_u16(current + 2)?;
            let file_offset = self.read_loaded_u32(current + 4)?;
            let auxiliary_offset = self.read_loaded_u32(current + 8)?;
            let next_offset = self.read_loaded_u32(current + 12)?;
            if version != 1 || auxiliary_count == 0 || auxiliary_offset == 0 {
                return Err(LoadError::Format("invalid Elf64_Verneed header"));
            }
            let soname = self.dynamic_string(file_offset)?;
            if !self.needed_libraries.iter().any(|needed| needed == &soname) {
                return Err(LoadError::Format(
                    "Elf64_Verneed file is not present in DT_NEEDED",
                ));
            }
            let mut auxiliary = current
                .checked_add(u64::from(auxiliary_offset))
                .ok_or(LoadError::Bounds("Elf64_Vernaux address overflow"))?;
            for auxiliary_index in 0..auxiliary_count {
                self.require_loaded_range(auxiliary, 16, Some(PF_R), "Elf64_Vernaux")?;
                let expected_hash = self.read_loaded_u32(auxiliary)?;
                let flags = self.read_loaded_u16(auxiliary + 4)?;
                let raw_other = self.read_loaded_u16(auxiliary + 6)?;
                let name_offset = self.read_loaded_u32(auxiliary + 8)?;
                let next = self.read_loaded_u32(auxiliary + 12)?;
                let index = raw_other & !VERSYM_HIDDEN;
                if index <= VER_NDX_GLOBAL {
                    return Err(LoadError::Format("invalid Elf64_Vernaux version index"));
                }
                if flags & !VER_FLG_WEAK != 0 {
                    return Err(LoadError::Format("unsupported Elf64_Vernaux flags"));
                }
                let name = self.dynamic_string(name_offset)?;
                if elf_hash(name.as_bytes()) != expected_hash {
                    return Err(LoadError::Format("Elf64_Vernaux hash mismatch"));
                }
                let requirement = OwnedVersionRequirement {
                    soname: soname.clone(),
                    name,
                    flags,
                };
                if result.insert(index, requirement).is_some() {
                    return Err(LoadError::Format("duplicate version requirement index"));
                }
                if auxiliary_index + 1 == auxiliary_count {
                    if next != 0 {
                        return Err(LoadError::Format("final Elf64_Vernaux has nonzero next"));
                    }
                } else if next == 0 {
                    return Err(LoadError::Format("truncated Elf64_Vernaux list"));
                } else {
                    auxiliary = auxiliary
                        .checked_add(u64::from(next))
                        .ok_or(LoadError::Bounds("Elf64_Vernaux next overflow"))?;
                }
            }
            if requirement_index + 1 == count {
                if next_offset != 0 {
                    return Err(LoadError::Format("final Elf64_Verneed has nonzero next"));
                }
            } else if next_offset == 0 {
                return Err(LoadError::Format("truncated Elf64_Verneed list"));
            } else {
                current = current
                    .checked_add(u64::from(next_offset))
                    .ok_or(LoadError::Bounds("Elf64_Verneed next overflow"))?;
            }
        }
        Ok(result)
    }

    fn version_for_symbol<'a>(
        &self,
        symbol_index: u32,
        requirements: &'a HashMap<u16, OwnedVersionRequirement>,
    ) -> Result<Option<(&'a OwnedVersionRequirement, bool)>, LoadError> {
        let Some(table) = self.dynamic.versym else {
            return Ok(None);
        };
        let address = table
            .checked_add(u64::from(symbol_index) * 2)
            .ok_or(LoadError::Bounds("DT_VERSYM entry overflow"))?;
        let raw = self.read_loaded_u16(address)?;
        let index = raw & !VERSYM_HIDDEN;
        if matches!(index, VER_NDX_LOCAL | VER_NDX_GLOBAL) {
            return Ok(None);
        }
        let requirement = requirements
            .get(&index)
            .ok_or(LoadError::Format("DT_VERSYM index has no DT_VERNEED entry"))?;
        Ok(Some((requirement, raw & VERSYM_HIDDEN != 0)))
    }

    fn load_bias_add(&self, addend: i64) -> Result<usize, LoadError> {
        (self.mapping.as_ptr() as i128)
            .checked_add(addend as i128 - self.minimum_page as i128)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(LoadError::Bounds("R_AARCH64_RELATIVE value overflow"))
    }

    fn resolve_function(&self, requested: &str) -> Result<usize, LoadError> {
        self.resolve_function_bytes(requested.as_bytes())
    }

    fn resolve_function_bytes(&self, requested: &[u8]) -> Result<usize, LoadError> {
        for index in 1..self.symbol_count()? {
            let symbol = self.dynamic_symbol(index)?;
            if self.dynamic_string_bytes(symbol.name_offset)? != requested {
                continue;
            }
            if symbol.section_index == SHN_UNDEF
                || !matches!(symbol.kind, STT_FUNC | STT_NOTYPE)
                || !matches!(symbol.binding, STB_GLOBAL | STB_WEAK)
                || !matches!(symbol.visibility, STV_DEFAULT | STV_PROTECTED)
                || symbol.value == 0
            {
                return Err(LoadError::InvalidSymbol(
                    String::from_utf8_lossy(requested).into_owned(),
                ));
            }
            if symbol.section_index == SHN_ABS {
                return Err(LoadError::Capability(Capability::AbsoluteSymbolDefinition));
            }
            self.require_loaded_range(
                symbol.value,
                symbol.size.max(1),
                Some(PF_X),
                "function symbol",
            )?;
            let pointer = self.loaded_pointer(symbol.value, 1)? as usize;
            return Ok(pointer);
        }
        Err(LoadError::SymbolNotFound(
            String::from_utf8_lossy(requested).into_owned(),
        ))
    }

    fn exported_symbols(&self) -> Result<Vec<ExportedSymbol>, LoadError> {
        let count = self.symbol_count()?;
        let versions = self.parse_version_definitions(count)?;
        let mut result = Vec::new();
        for index in 1..count {
            let symbol = self.dynamic_symbol(index)?;
            if symbol.section_index == SHN_UNDEF
                || !matches!(symbol.kind, STT_NOTYPE | STT_OBJECT | STT_FUNC)
                || !matches!(symbol.binding, STB_GLOBAL | STB_WEAK)
                || !matches!(symbol.visibility, STV_DEFAULT | STV_PROTECTED)
                || symbol.value == 0
            {
                continue;
            }
            let symbol_name = self.dynamic_string_bytes(symbol.name_offset)?;
            if symbol.section_index == SHN_ABS
                && !self.is_image_relative_absolute_marker(&symbol, symbol_name)
            {
                return Err(LoadError::Capability(Capability::AbsoluteSymbolDefinition));
            }
            // GNU/LLVM linkers commonly emit the zero-sized `end` marker at
            // the first address after the final PT_LOAD.  It is a valid
            // dlsym-visible sentinel, but treating it as a one-byte object
            // would reject an otherwise loadable Android image.
            let is_load_end_marker = symbol.size == 0
                && self.loads.iter().any(|load| {
                    load.virtual_address
                        .checked_add(load.memory_size)
                        .is_some_and(|end| end == symbol.value)
                });
            let range_error = if is_load_end_marker {
                None
            } else {
                self.require_loaded_range(symbol.value, symbol.size.max(1), None, "exported symbol")
                    .err()
            };
            if let Some(error) = range_error {
                return Err(error);
            }
            let name = self.dynamic_string_bytes(symbol.name_offset)?.to_vec();
            if name.is_empty() {
                continue;
            }
            let (version, hidden) = self.definition_for_symbol(index, &versions)?;
            let version_index = self.version_index_for_symbol(index)?;
            let pointer_size = usize::try_from(symbol.size)
                .map_err(|_| LoadError::Bounds("exported symbol size"))?;
            result.push(ExportedSymbol {
                name,
                address: self.loaded_pointer(symbol.value, pointer_size)? as usize,
                version: version.map(|item| item.name.clone()),
                version_hidden: hidden,
                version_index,
            });
        }
        Ok(result)
    }

    fn parse_version_definitions(
        &self,
        symbol_count: u32,
    ) -> Result<HashMap<u16, OwnedVersionDefinition>, LoadError> {
        let Some(mut current) = self.dynamic.verdef else {
            if self.dynamic.verdef_count.is_some() {
                return Err(LoadError::Format("incomplete DT_VERDEF metadata"));
            }
            return Ok(HashMap::new());
        };
        let count = self
            .dynamic
            .verdef_count
            .ok_or(LoadError::Format("incomplete DT_VERDEF metadata"))?;
        let versym = self
            .dynamic
            .versym
            .ok_or(LoadError::Format("DT_VERDEF requires DT_VERSYM"))?;
        if count == 0 {
            return Err(LoadError::Format("DT_VERDEFNUM is zero"));
        }
        self.require_loaded_range(versym, u64::from(symbol_count) * 2, Some(PF_R), "DT_VERSYM")?;
        let mut result = HashMap::new();
        let mut visited = HashSet::new();
        for definition_index in 0..count {
            if !visited.insert(current) {
                return Err(LoadError::Format("cyclic DT_VERDEF list"));
            }
            self.require_loaded_range(current, 20, Some(PF_R), "Elf64_Verdef")?;
            let version = self.read_loaded_u16(current)?;
            let flags = self.read_loaded_u16(current + 2)?;
            let index = self.read_loaded_u16(current + 4)? & !VERSYM_HIDDEN;
            let auxiliary_count = self.read_loaded_u16(current + 6)?;
            let expected_hash = self.read_loaded_u32(current + 8)?;
            let auxiliary_offset = self.read_loaded_u32(current + 12)?;
            let next_offset = self.read_loaded_u32(current + 16)?;
            if version != 1
                || index == VER_NDX_LOCAL
                || (index == VER_NDX_GLOBAL && flags & VER_FLG_BASE == 0)
                || auxiliary_count == 0
                || auxiliary_offset == 0
            {
                return Err(LoadError::Format("invalid Elf64_Verdef header"));
            }
            if flags & !(VER_FLG_BASE | VER_FLG_WEAK) != 0 {
                return Err(LoadError::Format("unsupported Elf64_Verdef flags"));
            }
            let mut auxiliary = current
                .checked_add(u64::from(auxiliary_offset))
                .ok_or(LoadError::Bounds("Elf64_Verdaux address overflow"))?;
            let mut definition_name = None;
            for auxiliary_index in 0..auxiliary_count {
                self.require_loaded_range(auxiliary, 8, Some(PF_R), "Elf64_Verdaux")?;
                let name = self.dynamic_string(self.read_loaded_u32(auxiliary)?)?;
                let next = self.read_loaded_u32(auxiliary + 4)?;
                if auxiliary_index == 0 {
                    definition_name = Some(name);
                }
                if auxiliary_index + 1 == auxiliary_count {
                    if next != 0 {
                        return Err(LoadError::Format("final Elf64_Verdaux has nonzero next"));
                    }
                } else if next == 0 {
                    return Err(LoadError::Format("truncated Elf64_Verdaux list"));
                } else {
                    auxiliary = auxiliary
                        .checked_add(u64::from(next))
                        .ok_or(LoadError::Bounds("Elf64_Verdaux next overflow"))?;
                }
            }
            let name = definition_name.expect("nonzero Verdaux count");
            if elf_hash(name.as_bytes()) != expected_hash {
                return Err(LoadError::Format("Elf64_Verdef hash mismatch"));
            }
            if result
                .insert(index, OwnedVersionDefinition { name })
                .is_some()
            {
                return Err(LoadError::Format("duplicate version definition index"));
            }
            if definition_index + 1 == count {
                if next_offset != 0 {
                    return Err(LoadError::Format("final Elf64_Verdef has nonzero next"));
                }
            } else if next_offset == 0 {
                return Err(LoadError::Format("truncated Elf64_Verdef list"));
            } else {
                current = current
                    .checked_add(u64::from(next_offset))
                    .ok_or(LoadError::Bounds("Elf64_Verdef next overflow"))?;
            }
        }
        Ok(result)
    }

    fn definition_for_symbol<'a>(
        &self,
        symbol_index: u32,
        definitions: &'a HashMap<u16, OwnedVersionDefinition>,
    ) -> Result<(Option<&'a OwnedVersionDefinition>, bool), LoadError> {
        let Some(table) = self.dynamic.versym else {
            return Ok((None, false));
        };
        let address = table
            .checked_add(u64::from(symbol_index) * 2)
            .ok_or(LoadError::Bounds("DT_VERSYM entry overflow"))?;
        let raw = self.read_loaded_u16(address)?;
        let index = raw & !VERSYM_HIDDEN;
        if matches!(index, VER_NDX_LOCAL | VER_NDX_GLOBAL) {
            return Ok((None, raw & VERSYM_HIDDEN != 0));
        }
        let definition = definitions
            .get(&index)
            .ok_or(LoadError::Format("DT_VERSYM index has no DT_VERDEF entry"))?;
        Ok((Some(definition), raw & VERSYM_HIDDEN != 0))
    }

    fn version_index_for_symbol(&self, symbol_index: u32) -> Result<Option<u16>, LoadError> {
        let Some(table) = self.dynamic.versym else {
            return Ok(None);
        };
        let address = table
            .checked_add(u64::from(symbol_index) * 2)
            .ok_or(LoadError::Bounds("DT_VERSYM entry overflow"))?;
        Ok(Some(self.read_loaded_u16(address)?))
    }

    fn symbol_count(&self) -> Result<u32, LoadError> {
        if let Some(hash) = self.dynamic.hash {
            return self.sysv_hash_symbol_count(hash);
        }
        if let Some(hash) = self.dynamic.gnu_hash {
            return self.gnu_hash_symbol_count(hash);
        }
        Err(LoadError::Capability(Capability::MissingSysvHash))
    }

    fn sysv_hash_symbol_count(&self, hash: u64) -> Result<u32, LoadError> {
        self.require_loaded_range(hash, 8, Some(PF_R), "DT_HASH header")?;
        let bucket_count = self.read_loaded_u32(hash)? as u64;
        let chain_count = self.read_loaded_u32(hash + 4)?;
        let words = 2_u64
            .checked_add(bucket_count)
            .and_then(|value| value.checked_add(chain_count as u64))
            .ok_or(LoadError::Bounds("DT_HASH size overflow"))?;
        self.require_loaded_range(hash, words * 4, Some(PF_R), "DT_HASH table")?;
        Ok(chain_count)
    }

    fn gnu_hash_symbol_count(&self, hash: u64) -> Result<u32, LoadError> {
        self.require_loaded_range(hash, 16, Some(PF_R), "DT_GNU_HASH header")?;
        let bucket_count = self.read_loaded_u32(hash)?;
        let symbol_offset = self.read_loaded_u32(hash + 4)?;
        let bloom_count = self.read_loaded_u32(hash + 8)?;
        if bucket_count == 0 || bloom_count == 0 || !bloom_count.is_power_of_two() {
            return Err(LoadError::Format("invalid DT_GNU_HASH header"));
        }
        let buckets = hash
            .checked_add(16)
            .and_then(|value| value.checked_add(u64::from(bloom_count) * 8))
            .ok_or(LoadError::Bounds("DT_GNU_HASH bloom overflow"))?;
        let chains = buckets
            .checked_add(u64::from(bucket_count) * 4)
            .ok_or(LoadError::Bounds("DT_GNU_HASH buckets overflow"))?;
        self.require_loaded_range(
            hash,
            chains
                .checked_sub(hash)
                .ok_or(LoadError::Bounds("DT_GNU_HASH prefix"))?,
            Some(PF_R),
            "DT_GNU_HASH prefix",
        )?;

        let symbol_table = self
            .dynamic
            .symbol_table
            .ok_or(LoadError::Format("missing DT_SYMTAB"))?;
        let entry_size = self
            .dynamic
            .symbol_entry_size
            .unwrap_or(ELF64_SYM_SIZE as u64);
        if entry_size != ELF64_SYM_SIZE as u64 {
            return Err(LoadError::Format("invalid DT_SYMENT"));
        }
        let load = self
            .loads
            .iter()
            .find(|load| {
                load.virtual_address <= symbol_table
                    && load
                        .virtual_address
                        .checked_add(load.memory_size)
                        .is_some_and(|end| symbol_table < end)
            })
            .ok_or(LoadError::Bounds("DT_SYMTAB outside PT_LOAD"))?;
        let load_end = load
            .virtual_address
            .checked_add(load.memory_size)
            .ok_or(LoadError::Bounds("DT_SYMTAB load end"))?;
        let symbol_limit = (load_end - symbol_table) / entry_size;
        if u64::from(symbol_offset) > symbol_limit {
            return Err(LoadError::Bounds("DT_GNU_HASH symbol offset"));
        }

        let mut highest = None;
        for index in 0..bucket_count {
            let symbol = self.read_loaded_u32(buckets + u64::from(index) * 4)?;
            if symbol == 0 {
                continue;
            }
            if symbol < symbol_offset || u64::from(symbol) >= symbol_limit {
                return Err(LoadError::Bounds("DT_GNU_HASH bucket symbol"));
            }
            highest = Some(highest.map_or(symbol, |current: u32| current.max(symbol)));
        }
        let Some(mut symbol) = highest else {
            return Ok(symbol_offset);
        };
        loop {
            if u64::from(symbol) >= symbol_limit {
                return Err(LoadError::Bounds("DT_GNU_HASH chain symbol"));
            }
            let chain_index = u64::from(symbol - symbol_offset);
            let value = self.read_loaded_u32(
                chains
                    .checked_add(chain_index * 4)
                    .ok_or(LoadError::Bounds("DT_GNU_HASH chain overflow"))?,
            )?;
            symbol = symbol
                .checked_add(1)
                .ok_or(LoadError::Bounds("DT_GNU_HASH symbol count"))?;
            if value & 1 != 0 {
                return Ok(symbol);
            }
        }
    }

    fn dynamic_symbol(&self, index: u32) -> Result<DynamicSymbol, LoadError> {
        let table = self
            .dynamic
            .symbol_table
            .ok_or(LoadError::Format("missing DT_SYMTAB"))?;
        let entry_size = self
            .dynamic
            .symbol_entry_size
            .unwrap_or(ELF64_SYM_SIZE as u64);
        if entry_size != ELF64_SYM_SIZE as u64 {
            return Err(LoadError::Format("invalid DT_SYMENT"));
        }
        let address = table
            .checked_add(index as u64 * entry_size)
            .ok_or(LoadError::Bounds("dynamic symbol address overflow"))?;
        self.require_loaded_range(address, entry_size, Some(PF_R), "dynamic symbol")?;
        let bytes = self.loaded_slice(address, ELF64_SYM_SIZE)?;
        Ok(DynamicSymbol {
            name_offset: read_u32(bytes, 0)?,
            binding: bytes[4] >> 4,
            kind: bytes[4] & 0x0f,
            visibility: bytes[5] & 0x03,
            section_index: read_u16(bytes, 6)?,
            value: read_u64(bytes, 8)?,
            size: read_u64(bytes, 16)?,
        })
    }

    fn dynamic_string_bytes(&self, offset: u32) -> Result<&[u8], LoadError> {
        let table = self
            .dynamic
            .string_table
            .ok_or(LoadError::Format("missing DT_STRTAB"))?;
        let size = self
            .dynamic
            .string_size
            .ok_or(LoadError::Format("missing DT_STRSZ"))?;
        if offset as u64 >= size {
            return Err(LoadError::Bounds("dynamic string offset"));
        }
        self.require_loaded_range(table, size, Some(PF_R), "dynamic string table")?;
        let bytes = self.loaded_slice(table + offset as u64, (size - offset as u64) as usize)?;
        terminated_dynamic_string(bytes)
    }

    fn dynamic_string(&self, offset: u32) -> Result<String, LoadError> {
        let value = self.dynamic_string_bytes(offset)?;
        let value = std::str::from_utf8(value)
            .map_err(|_| LoadError::Format("non-UTF-8 dynamic metadata string"))?;
        Ok(value.to_owned())
    }

    fn apply_final_protections(
        &self,
        page_size: usize,
        page_protections: &[c_int],
    ) -> Result<(), LoadError> {
        let mut start = 0;
        while start < page_protections.len() {
            let protection = page_protections[start];
            let mut end = start + 1;
            while end < page_protections.len() && page_protections[end] == protection {
                end += 1;
            }
            let address = unsafe { self.reservation.as_ptr().add(start * page_size) };
            let length = (end - start) * page_size;
            if protection & PROT_EXEC != 0 {
                // SAFETY: this is a live portion of the loader-owned reservation. Apple
                // requires explicit instruction-cache invalidation for freshly copied code.
                unsafe { sys_icache_invalidate(address.cast(), length) };
            }
            if unsafe { mprotect(address.cast(), length, protection) } != 0 {
                return Err(system_error("mprotect(final)"));
            }
            start = end;
        }
        Ok(())
    }

    fn rewrite_android_stack_guard_tls(&mut self) -> Result<(), LoadError> {
        let executable_ranges = self
            .loads
            .iter()
            .filter(|load| load.flags & PF_X != 0)
            .map(|load| (load.virtual_address, load.file_size))
            .collect::<Vec<_>>();
        for (virtual_address, file_size) in executable_ranges {
            let size = to_usize(file_size, "executable PT_LOAD file size")?;
            let pointer = self.loaded_pointer(virtual_address, size)?;
            // SAFETY: staging made every populated PT_LOAD page writable. This unique `&mut self`
            // owns the image, and final executable protections/cache invalidation happen later.
            let code = unsafe { std::slice::from_raw_parts_mut(pointer, size) };
            let rewrites = direct_tls::rewrite_android_stack_guard_tls(
                code,
                self.stack_guard.as_ptr() as usize,
            )?;
            for rewrite in rewrites {
                let offset = u64::try_from(rewrite.instruction_index)
                    .ok()
                    .and_then(|index| index.checked_mul(4))
                    .and_then(|offset| virtual_address.checked_add(offset))
                    .ok_or(LoadError::Bounds("stack-guard rewrite address"))?;
                self.stack_guard_rewrites.push((offset, rewrite.original));
            }
        }
        Ok(())
    }

    fn rewrite_direct_android_syscalls(
        &mut self,
        resolver: &mut dyn SymbolResolver,
    ) -> Result<(), LoadError> {
        let executable_ranges = self
            .loads
            .iter()
            .filter(|load| load.flags & PF_X != 0)
            .map(|load| (load.virtual_address, load.file_size))
            .collect::<Vec<_>>();
        let has_direct_syscall = executable_ranges.iter().try_fold(false, |found, range| {
            let size = to_usize(range.1, "executable PT_LOAD file size")?;
            let code = self.loaded_slice(range.0, size)?;
            Ok::<_, LoadError>(found || direct_syscall::contains_linux_svc(code))
        })?;
        if !has_direct_syscall {
            return Ok(());
        }

        let target = resolver
            .resolve(SymbolRequest {
                symbol: "syscall",
                needed_libraries: &self.needed_libraries,
                version: Some(VersionRequirement {
                    soname: "libc.so",
                    name: "LIBC",
                    hidden: false,
                    flags: 0,
                }),
                is_weak: false,
            })
            .map_err(|source| LoadError::Resolver {
                symbol: "syscall".to_owned(),
                source,
            })?
            .ok_or(LoadError::Capability(Capability::DirectSyscall))?
            .address();

        let errno_target = resolver
            .resolve(SymbolRequest {
                symbol: "__errno",
                needed_libraries: &self.needed_libraries,
                version: Some(VersionRequirement {
                    soname: "libc.so",
                    name: "LIBC",
                    hidden: false,
                    flags: 0,
                }),
                is_weak: false,
            })
            .map_err(|source| LoadError::Resolver {
                symbol: "__errno".to_owned(),
                source,
            })?
            .ok_or(LoadError::Capability(Capability::DirectSyscall))?
            .address();

        let mut veneer_cursor = 0usize;
        for (virtual_address, file_size) in executable_ranges {
            let size = to_usize(file_size, "executable PT_LOAD file size")?;
            let pointer = self.loaded_pointer(virtual_address, size)?;
            let rewrites = {
                // SAFETY: staging made every executable PT_LOAD page writable and this unique
                // mutable image owner serializes all instruction rewrites.
                let code = unsafe { std::slice::from_raw_parts_mut(pointer, size) };
                direct_syscall::rewrite_linux_svc(
                    code,
                    self.direct_syscall_shim.as_ptr() as usize,
                    &mut veneer_cursor,
                    self.direct_syscall_shim_size,
                )?
            };
            for rewrite in rewrites {
                // SAFETY: the parser reserved and staged the complete direct-SVC veneer area;
                // each cursor slot is unique and remains writable until final protections.
                unsafe {
                    direct_syscall::write_veneer(
                        self.direct_syscall_shim.as_ptr().add(rewrite.veneer_offset),
                        target,
                        errno_target,
                        rewrite.return_address,
                    )?;
                }
                let offset = u64::try_from(rewrite.instruction_index)
                    .ok()
                    .and_then(|index| index.checked_mul(4))
                    .and_then(|offset| virtual_address.checked_add(offset))
                    .ok_or(LoadError::Bounds("direct-syscall rewrite address"))?;
                self.direct_syscall_rewrites
                    .push((offset, rewrite.original));
            }
        }
        eprintln!(
            "DARWIN ELF syscall: rewrote {} direct Linux svc instruction(s)",
            self.direct_syscall_rewrites.len()
        );
        Ok(())
    }

    fn require_host_executable(&self, pointer: u64, what: &'static str) -> Result<(), LoadError> {
        let offset = pointer
            .checked_sub(self.mapping.as_ptr() as u64)
            .ok_or(LoadError::Bounds("host pointer below mapping"))?;
        let virtual_address = self
            .minimum_page
            .checked_add(offset)
            .ok_or(LoadError::Bounds("host pointer translation overflow"))?;
        self.require_loaded_range(virtual_address, 1, Some(PF_X), what)
    }

    fn require_loaded_range(
        &self,
        address: u64,
        size: u64,
        required_flag: Option<u32>,
        what: &'static str,
    ) -> Result<(), LoadError> {
        let end = address.checked_add(size).ok_or(LoadError::Bounds(what))?;
        let found = self.loads.iter().any(|load| {
            let Some(load_end) = load.virtual_address.checked_add(load.memory_size) else {
                return false;
            };
            address >= load.virtual_address
                && end <= load_end
                && required_flag.is_none_or(|flag| load.flags & flag != 0)
        });
        if found {
            Ok(())
        } else {
            Err(LoadError::Bounds(what))
        }
    }

    fn is_image_relative_absolute_marker(&self, symbol: &DynamicSymbol, name: &[u8]) -> bool {
        // Android DSOs produced by GNU/LLVM linkers commonly publish
        // zero-sized linker markers such as __bss_start, _edata, and _end as
        // SHN_ABS even though their st_value is an image virtual address.
        // Bionic exposes these as load-bias-relative dlsym values. Accept only
        // the tightly bounded form whose address lies in (or exactly at the
        // end of) a PT_LOAD; arbitrary absolute constants remain rejected.
        const LINKER_MARKERS: [&[u8]; 8] = [
            b"__bss_start",
            b"__bss_end__",
            b"__bss_start__",
            b"_edata",
            b"_bss_end__",
            b"_end",
            b"__end__",
            b"end",
        ];
        let section_marker = name
            .strip_prefix(b"__start_")
            .or_else(|| name.strip_prefix(b"__stop_"))
            .is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix.iter().enumerate().all(|(index, byte)| {
                        if index == 0 {
                            byte.is_ascii_alphabetic() || *byte == b'_'
                        } else {
                            byte.is_ascii_alphanumeric() || *byte == b'_'
                        }
                    })
            });
        symbol.section_index == SHN_ABS
            && symbol.size == 0
            && (LINKER_MARKERS.contains(&name) || section_marker)
            && self
                .require_loaded_range(symbol.value, 0, None, "image-relative absolute marker")
                .is_ok()
    }

    fn loaded_pointer(&self, address: u64, size: usize) -> Result<*mut u8, LoadError> {
        let offset = difference_to_usize(address, self.minimum_page)?;
        let end = offset
            .checked_add(size)
            .ok_or(LoadError::Bounds("mapped pointer overflow"))?;
        if end > self.mapping_size {
            return Err(LoadError::Bounds("mapped pointer outside reservation"));
        }
        Ok(unsafe { self.mapping.as_ptr().add(offset) })
    }

    fn loaded_slice(&self, address: u64, size: usize) -> Result<&[u8], LoadError> {
        let pointer = self.loaded_pointer(address, size)?;
        Ok(unsafe { std::slice::from_raw_parts(pointer, size) })
    }

    fn read_loaded_u32(&self, address: u64) -> Result<u32, LoadError> {
        read_u32(self.loaded_slice(address, 4)?, 0)
    }

    fn read_loaded_u16(&self, address: u64) -> Result<u16, LoadError> {
        read_u16(self.loaded_slice(address, 2)?, 0)
    }

    fn read_loaded_u64(&self, address: u64) -> Result<u64, LoadError> {
        read_u64(self.loaded_slice(address, 8)?, 0)
    }

    fn read_loaded_i64(&self, address: u64) -> Result<i64, LoadError> {
        Ok(self.read_loaded_u64(address)? as i64)
    }
}

impl Drop for LoadedElf {
    fn drop(&mut self) {
        self.finalize_once();
        unregister_tls_descriptors(&self.tls_descriptor_tokens);
        if let Some(module) = &self.tls_module {
            // Guest finalizers may touch this image's TLS. Release the unloading thread's block
            // synchronously, then seal allocation. Any other live thread makes unmapping unsafe.
            release_current_thread_tls(module.id);
            if module.seal_for_unload().is_err() {
                std::process::abort();
            }
        }
        // SAFETY: LoadedElf exclusively owns this complete mmap reservation.
        let result = unsafe { munmap(self.reservation.as_ptr().cast(), self.reservation_size) };
        debug_assert_eq!(result, 0, "munmap failed while dropping LoadedElf");
    }
}

#[derive(Debug)]
struct DynamicSymbol {
    name_offset: u32,
    binding: u8,
    kind: u8,
    visibility: u8,
    section_index: u16,
    value: u64,
    size: u64,
}

#[derive(Clone, Debug)]
struct OwnedVersionRequirement {
    soname: String,
    name: String,
    flags: u16,
}

#[derive(Clone, Debug)]
struct OwnedVersionDefinition {
    name: String,
}

#[derive(Clone, Debug)]
struct ExportedSymbol {
    /// Raw bytes from the ELF string table. ELF symbol names are byte strings,
    /// not Rust text; keeping them raw prevents invalid names from colliding
    /// through replacement-character or lossy conversions.
    name: Vec<u8>,
    address: usize,
    version: Option<String>,
    version_hidden: bool,
    /// Raw DT_VERSYM value, including VERSYM_HIDDEN. None means DT_VERSYM is
    /// absent from this image, which is distinct from a LOCAL/GLOBAL index.
    version_index: Option<u16>,
}

fn terminated_dynamic_string(bytes: &[u8]) -> Result<&[u8], LoadError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(LoadError::Format("unterminated dynamic string"))?;
    Ok(&bytes[..end])
}

fn find_exported_symbol<'a>(
    symbols: &'a [ExportedSymbol],
    name: &[u8],
) -> Option<&'a ExportedSymbol> {
    symbols.iter().find(|symbol| symbol.name == name)
}

fn elf_flags_to_protection(flags: u32) -> c_int {
    let mut protection = PROT_NONE;
    if flags & PF_R != 0 {
        protection |= PROT_READ;
    }
    if flags & PF_W != 0 {
        protection |= PROT_WRITE;
    }
    if flags & PF_X != 0 {
        protection |= PROT_EXEC;
    }
    protection
}

fn set_once(slot: &mut Option<u64>, value: u64, duplicate: &'static str) -> Result<(), LoadError> {
    if slot.replace(value).is_some() {
        Err(LoadError::Format(duplicate))
    } else {
        Ok(())
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, LoadError> {
    let data: [u8; 2] = checked_slice(bytes, offset, 2, "u16 read")?
        .try_into()
        .unwrap();
    Ok(u16::from_le_bytes(data))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, LoadError> {
    let data: [u8; 4] = checked_slice(bytes, offset, 4, "u32 read")?
        .try_into()
        .unwrap();
    Ok(u32::from_le_bytes(data))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, LoadError> {
    let data: [u8; 8] = checked_slice(bytes, offset, 8, "u64 read")?
        .try_into()
        .unwrap();
    Ok(u64::from_le_bytes(data))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, LoadError> {
    Ok(read_u64(bytes, offset)? as i64)
}

fn checked_slice<'a>(
    bytes: &'a [u8],
    offset: usize,
    size: usize,
    what: &'static str,
) -> Result<&'a [u8], LoadError> {
    let end = offset.checked_add(size).ok_or(LoadError::Bounds(what))?;
    bytes.get(offset..end).ok_or(LoadError::Bounds(what))
}

fn to_usize(value: u64, what: &'static str) -> Result<usize, LoadError> {
    usize::try_from(value).map_err(|_| LoadError::Bounds(what))
}

fn difference_to_usize(value: u64, base: u64) -> Result<usize, LoadError> {
    let difference = value
        .checked_sub(base)
        .ok_or(LoadError::Bounds("address below image base"))?;
    to_usize(difference, "address difference")
}

fn checked_signed_add(base: usize, addend: i64, what: &'static str) -> Result<usize, LoadError> {
    (base as i128)
        .checked_add(addend as i128)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(LoadError::Bounds(what))
}

fn elf_hash(name: &[u8]) -> u32 {
    let mut hash = 0_u32;
    for byte in name {
        hash = hash.wrapping_shl(4).wrapping_add(u32::from(*byte));
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    hash
}

fn system_error(operation: &'static str) -> LoadError {
    LoadError::System {
        operation,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_header() {
        assert!(matches!(
            LoadedElf::load(&[0; 8]),
            Err(LoadError::Format(_))
        ));
    }

    #[test]
    fn checked_slice_rejects_overflow() {
        assert!(checked_slice(&[0; 4], usize::MAX, 2, "test").is_err());
    }

    #[test]
    fn rejects_non_elf() {
        assert!(matches!(
            LoadedElf::load(&[0; ELF64_EHDR_SIZE]),
            Err(LoadError::Format("bad magic"))
        ));
    }

    #[test]
    fn raw_defined_symbol_names_are_retained_and_looked_up_exactly() {
        let symbols = vec![
            ExportedSymbol {
                name: b"ascii_export".to_vec(),
                address: 1,
                version: None,
                version_hidden: false,
                version_index: None,
            },
            ExportedSymbol {
                name: b"raw\x80".to_vec(),
                address: 2,
                version: None,
                version_hidden: false,
                version_index: None,
            },
            ExportedSymbol {
                name: b"raw\x81".to_vec(),
                address: 3,
                version: None,
                version_hidden: false,
                version_index: None,
            },
            ExportedSymbol {
                name: b"raw\xef\xbf\xbd".to_vec(),
                address: 4,
                version: None,
                version_hidden: false,
                version_index: None,
            },
        ];
        assert_eq!(
            find_exported_symbol(&symbols, b"ascii_export")
                .unwrap()
                .address,
            1
        );
        assert_eq!(
            find_exported_symbol(&symbols, b"raw\x80").unwrap().address,
            2
        );
        assert_eq!(
            find_exported_symbol(&symbols, b"raw\x81").unwrap().address,
            3
        );
        assert_eq!(
            find_exported_symbol(&symbols, b"raw\xef\xbf\xbd")
                .unwrap()
                .address,
            4
        );
        assert!(find_exported_symbol(&symbols, b"raw\x82").is_none());
    }

    #[test]
    fn dynamic_string_decoder_preserves_invalid_bytes_and_requires_nul() {
        let bytes = b"defined\x80\0following";
        assert_eq!(terminated_dynamic_string(bytes).unwrap(), b"defined\x80");
        assert!(matches!(
            terminated_dynamic_string(b"unterminated\x80"),
            Err(LoadError::Format("unterminated dynamic string"))
        ));
    }
}
