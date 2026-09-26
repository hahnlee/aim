#![forbid(unsafe_op_in_unsafe_fn)]

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_int};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

mod credentials;
mod process_owner;
mod process_snapshot_abi;
mod properties;
mod property_iteration_abi;
mod property_versions;
mod property_wait_abi;
use credentials::Credentials;
pub use credentials::{
    CredentialsOutput, darwin_art_bionic_process_state_read_credential_ids_core,
    darwin_art_bionic_process_state_read_credentials_core,
};
pub use process_owner::{
    darwin_art_bionic_process_state_is_installed, darwin_art_bionic_process_state_process_install,
    darwin_art_bionic_process_state_process_uninstall, install_process_snapshot,
    uninstall_process_snapshot,
};
pub use process_snapshot_abi::darwin_art_bionic_process_state_install_configured;
use properties::PropertyArea;
const ANDROID_ENOENT: i32 = 2;
const ANDROID_E2BIG: i32 = 7;
const ANDROID_EIO: i32 = 5;
const ANDROID_EFAULT: i32 = 14;
const ANDROID_EINVAL: i32 = 22;
const AT_PAGESZ: u64 = 6;
const AT_HWCAP: u64 = 16;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_HWCAP2: u64 = 26;
const SAFE_HWCAP: u64 = 3; // AArch64 FP | ASIMD baseline only.

unsafe extern "C" {
    fn darwin_art_bionic_errno_store(android_errno: i32);
}

static ACTIVE: OnceLock<RwLock<Option<Arc<Snapshot>>>> = OnceLock::new();
static CAPABILITY_FAILURE: AtomicBool = AtomicBool::new(false);
static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

// A process always exposes a valid, NUL-terminated environment vector, even
// before the host installs its capability-filtered Android snapshot. This is
// the same empty environment shape accepted by execve and prevents native
// code from having to special-case a null `environ` pointer.
static mut EMPTY_ENVIRONMENT: [*mut c_char; 1] = [ptr::null_mut()];

#[unsafe(no_mangle)]
pub static mut darwin_art_bionic_environ: *mut *mut c_char =
    (&raw mut EMPTY_ENVIRONMENT).cast::<*mut c_char>();

fn active_slot() -> &'static RwLock<Option<Arc<Snapshot>>> {
    ACTIVE.get_or_init(|| RwLock::new(None))
}

#[derive(Clone, Copy, Debug)]
pub struct AuxSnapshot {
    pub page_size: u64,
    pub hwcap: u64,
    pub hwcap2: u64,
    pub secure: bool,
    pub random: [u8; 16],
}

pub struct Snapshot {
    environment: BTreeMap<Vec<u8>, Box<[u8]>>,
    _environment_strings: Vec<Box<[u8]>>,
    environment_pointers: Vec<usize>,
    properties: PropertyArea,
    auxv: BTreeMap<u64, u64>,
    _random: Box<[u8; 16]>,
    credentials: Option<Credentials>,
}

impl Snapshot {
    pub fn new(
        environment: Vec<(Vec<u8>, Vec<u8>)>,
        properties: Vec<(Vec<u8>, Vec<u8>)>,
        aux: AuxSnapshot,
    ) -> Result<Self, &'static str> {
        Self::new_inner(environment, properties, aux, None)
    }

    pub(crate) fn new_with_credentials(
        environment: Vec<(Vec<u8>, Vec<u8>)>,
        properties: Vec<(Vec<u8>, Vec<u8>)>,
        aux: AuxSnapshot,
        credentials: Credentials,
    ) -> Result<Self, &'static str> {
        Self::new_inner(environment, properties, aux, Some(credentials))
    }

    fn new_inner(
        environment: Vec<(Vec<u8>, Vec<u8>)>,
        properties: Vec<(Vec<u8>, Vec<u8>)>,
        aux: AuxSnapshot,
        credentials: Option<Credentials>,
    ) -> Result<Self, &'static str> {
        if aux.page_size < 4096 || !aux.page_size.is_power_of_two() {
            return Err("invalid Android page size");
        }
        if aux.hwcap & !SAFE_HWCAP != 0 || aux.hwcap2 != 0 {
            return Err("unsupported Android arm64 hardware capability claim");
        }
        let environment = collect_environment(environment)?;
        let environment_strings = environment
            .iter()
            .map(|(name, value)| {
                let mut entry = Vec::with_capacity(name.len() + value.len() + 1);
                entry.extend_from_slice(name);
                entry.push(b'=');
                entry.extend_from_slice(value);
                entry.into_boxed_slice()
            })
            .collect::<Vec<_>>();
        let mut environment_pointers = environment_strings
            .iter()
            .map(|entry| entry.as_ptr() as usize)
            .collect::<Vec<_>>();
        environment_pointers.push(0);
        let properties = PropertyArea::new(properties)?;
        let random = Box::new(aux.random);
        let random_address = (&*random as *const [u8; 16]) as usize as u64;
        let auxv = BTreeMap::from([
            (AT_PAGESZ, aux.page_size),
            (AT_HWCAP, aux.hwcap),
            (AT_SECURE, u64::from(aux.secure)),
            (AT_RANDOM, random_address),
            (AT_HWCAP2, aux.hwcap2),
        ]);
        Ok(Self {
            environment,
            _environment_strings: environment_strings,
            environment_pointers,
            properties,
            auxv,
            _random: random,
            credentials,
        })
    }

    pub fn activate(self: &Arc<Self>) -> Result<Activation, &'static str> {
        let mut active = active_slot()
            .write()
            .map_err(|_| "activation lock poisoned")?;
        if active.is_some() {
            return Err("another process snapshot is active");
        }
        self.properties.resume_waiters()?;
        // SAFETY: activation is serialized by ACTIVE's write lock. The pointer
        // array and its strings are owned by the Arc installed below and remain
        // stable until Activation clears the exported slot before dropping it.
        unsafe {
            darwin_art_bionic_environ = self.environment_pointers.as_ptr().cast_mut().cast();
        }
        *active = Some(Arc::clone(self));
        Ok(Activation { active: true })
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::AcqRel);
    }
}

pub struct Activation {
    active: bool,
}

impl Drop for Activation {
    fn drop(&mut self) {
        if self.active {
            match active_slot().write() {
                Ok(mut active) => {
                    // SAFETY: teardown holds the same exclusive activation lock.
                    unsafe {
                        darwin_art_bionic_environ =
                            (&raw mut EMPTY_ENVIRONMENT).cast::<*mut c_char>()
                    };
                    if let Some(snapshot) = active.take() {
                        snapshot.properties.close_waiters();
                    }
                }
                Err(_) => CAPABILITY_FAILURE.store(true, Ordering::Release),
            }
            self.active = false;
        }
    }
}

fn collect_environment(
    entries: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<BTreeMap<Vec<u8>, Box<[u8]>>, &'static str> {
    let mut result = BTreeMap::new();
    for (name, mut value) in entries {
        if name.is_empty() || name.contains(&0) || name.contains(&b'=') {
            return Err("invalid snapshot name");
        }
        if value.contains(&0) {
            return Err("invalid snapshot value");
        }
        value.push(0);
        if result.insert(name, value.into_boxed_slice()).is_some() {
            return Err("duplicate snapshot name");
        }
    }
    Ok(result)
}

fn set_errno(value: i32) {
    // SAFETY: linked Bionic errno storage is pthread-local and value-only.
    unsafe { darwin_art_bionic_errno_store(value) };
}

fn active_snapshot() -> Option<Arc<Snapshot>> {
    match active_slot().read() {
        Ok(active) => active.clone(),
        Err(_) => {
            CAPABILITY_FAILURE.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            None
        }
    }
}

#[cfg(test)]
pub(crate) fn test_process_owner_guard() -> std::sync::MutexGuard<'static, ()> {
    static TEST_PROCESS_OWNER_LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    TEST_PROCESS_OWNER_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn missing_snapshot() {
    CAPABILITY_FAILURE.store(true, Ordering::Release);
    set_errno(ANDROID_EIO);
}

unsafe fn c_bytes<'a>(value: *const c_char) -> Option<&'a [u8]> {
    if value.is_null() {
        return None;
    }
    // SAFETY: guest ABI requires a readable NUL-terminated string.
    Some(unsafe { CStr::from_ptr(value) }.to_bytes())
}

#[unsafe(no_mangle)]
/// # Safety
/// `name` must point to a readable NUL-terminated environment name.
pub unsafe extern "C" fn darwin_art_bionic_process_getenv_core(name: *const c_char) -> *mut c_char {
    let Some(name) = (unsafe { c_bytes(name) }) else {
        set_errno(ANDROID_EFAULT);
        return ptr::null_mut();
    };
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return ptr::null_mut();
    };
    snapshot
        .environment
        .get(name)
        .map_or(ptr::null_mut(), |value| {
            value.as_ptr().cast::<c_char>().cast_mut()
        })
}

#[unsafe(no_mangle)]
/// # Safety
/// `name` must be readable and NUL-terminated. `value` must be writable for
/// Android `PROP_VALUE_MAX` (92) bytes.
pub unsafe extern "C" fn darwin_art_bionic_process_property_get_core(
    name: *const c_char,
    value: *mut c_char,
) -> c_int {
    if value.is_null() {
        set_errno(ANDROID_EFAULT);
        return 0;
    }
    let Some(name) = (unsafe { c_bytes(name) }) else {
        set_errno(ANDROID_EFAULT);
        // SAFETY: non-null value is writable by ABI contract.
        unsafe { value.write(0) };
        return 0;
    };
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        // SAFETY: non-null value is writable by ABI contract.
        unsafe { value.write(0) };
        return 0;
    };
    let source = match snapshot.properties.get(name) {
        Ok(source) => source,
        Err(_) => {
            CAPABILITY_FAILURE.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            // SAFETY: output is writable by the caller's ABI contract.
            unsafe { value.write(0) };
            return 0;
        }
    };
    let Some(source) = source else {
        // SAFETY: property ABI requires at least PROP_VALUE_MAX writable bytes.
        unsafe { value.write(0) };
        return 0;
    };
    // SAFETY: construction bounds source to at most PROP_VALUE_MAX bytes including NUL.
    unsafe {
        ptr::copy_nonoverlapping(
            source.value.as_ptr().cast::<c_char>(),
            value,
            source.value.len(),
        )
    };
    (source.value.len() - 1) as c_int
}

#[unsafe(no_mangle)]
/// Publishes a value the property service has accepted into this process's
/// property area, as init's area write becomes visible to its setter. The
/// property-service client calls this only after the service's success
/// reply; it is not a native setter. Returns 0, or -1 with errno set.
///
/// # Safety
/// `name` and `value` must point to readable NUL-terminated strings.
pub unsafe extern "C" fn darwin_art_bionic_process_property_apply_service_update_core(
    name: *const c_char,
    value: *const c_char,
) -> c_int {
    let (Some(name), Some(value)) = (unsafe { c_bytes(name) }, unsafe { c_bytes(value) }) else {
        set_errno(ANDROID_EFAULT);
        return -1;
    };
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return -1;
    };
    match snapshot.properties.update(name, value) {
        Ok(()) => 0,
        Err(_) => {
            set_errno(ANDROID_EINVAL);
            -1
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `name` must point to a readable NUL-terminated property name.
pub unsafe extern "C" fn darwin_art_bionic_process_property_find_core(
    name: *const c_char,
) -> *const std::ffi::c_void {
    let Some(name) = (unsafe { c_bytes(name) }) else {
        set_errno(ANDROID_EFAULT);
        return ptr::null();
    };
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return ptr::null();
    };
    match snapshot.properties.find(name) {
        Ok(token) => token,
        Err(_) => {
            CAPABILITY_FAILURE.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            ptr::null()
        }
    }
}

type PropertyReadCallback = unsafe extern "C" fn(
    cookie: *mut std::ffi::c_void,
    name: *const c_char,
    value: *const c_char,
    serial: u32,
);

#[unsafe(no_mangle)]
/// # Safety
/// `property` must be a token returned by the active snapshot's find call and
/// `callback`, when present, must be callable for the duration of this call.
pub unsafe extern "C" fn darwin_art_bionic_process_property_read_callback_core(
    property: *const std::ffi::c_void,
    callback: Option<PropertyReadCallback>,
    cookie: *mut std::ffi::c_void,
) {
    if property.is_null() || callback.is_none() {
        set_errno(ANDROID_EFAULT);
        return;
    }
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return;
    };
    let entry = match snapshot.properties.read(property) {
        Ok(entry) => entry,
        Err(_) => {
            CAPABILITY_FAILURE.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return;
        }
    };
    let Some(entry) = entry else {
        set_errno(ANDROID_EFAULT);
        return;
    };
    // SAFETY: the coherent read shares immutable strings with the area;
    // read-only values remain valid for the area's lifetime. No lock is held
    // during the synchronous callback, which may re-enter property lookup.
    unsafe {
        callback.unwrap_unchecked()(
            cookie,
            entry.name.as_ptr().cast(),
            entry.value.as_ptr().cast(),
            entry.serial,
        )
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_process_getauxval_core(kind: u64) -> u64 {
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return 0;
    };
    match snapshot.auxv.get(&kind) {
        Some(value) => *value,
        None => {
            set_errno(ANDROID_ENOENT);
            0
        }
    }
}

pub fn capability_failed() -> bool {
    CAPABILITY_FAILURE.load(Ordering::Acquire)
}

pub fn drop_count() -> usize {
    DROP_COUNT.load(Ordering::Acquire)
}
