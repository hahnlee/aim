#![forbid(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const OK: i32 = 0;
const BAD_VALUE: i32 = -22;
const INVALID_OPERATION: i32 = -38;

type OnCreate = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type OnDestroy = unsafe extern "C" fn(*mut c_void);
type OnTransact = unsafe extern "C" fn(*mut Binder, u32, *const Parcel, *mut Parcel) -> i32;
type OnDump = unsafe extern "C" fn(*mut Binder, c_int, *const *const c_char, u32) -> i32;

#[repr(C)]
pub struct BinderClass {
    descriptor: Box<[u8]>,
    on_create: Option<OnCreate>,
    on_destroy: Option<OnDestroy>,
    on_transact: Option<OnTransact>,
    on_dump: Mutex<Option<OnDump>>,
    sealed: AtomicBool,
}

#[repr(C)]
pub struct Binder {
    class: AtomicUsize,
    user_data: AtomicUsize,
    strong: AtomicUsize,
    destroyed: AtomicBool,
}

#[repr(C)]
pub struct WeakBinder {
    binder: *mut Binder,
}

#[derive(Default)]
struct ParcelState {
    bytes: Vec<u8>,
    position: usize,
    owned_fds: Vec<c_int>,
}

#[repr(C)]
pub struct Parcel {
    state: Mutex<ParcelState>,
}

#[repr(C)]
pub struct Status {
    value: i32,
}

unsafe extern "C" {
    fn aim_bionic_socket_broker_dup(fd: c_int) -> c_int;
    fn aim_bionic_socket_broker_close(fd: c_int) -> c_int;
}

fn new_parcel() -> *mut Parcel {
    Box::into_raw(Box::new(Parcel {
        state: Mutex::new(ParcelState::default()),
    }))
}

fn append(parcel: *mut Parcel, bytes: &[u8]) -> i32 {
    let Some(parcel) = (unsafe { parcel.as_ref() }) else {
        return BAD_VALUE;
    };
    let Ok(mut state) = parcel.state.lock() else {
        return INVALID_OPERATION;
    };
    let position = state.position;
    let end = match position.checked_add(bytes.len()) {
        Some(v) => v,
        None => return BAD_VALUE,
    };
    if end > state.bytes.len() {
        state.bytes.resize(end, 0);
    }
    state.bytes[position..end].copy_from_slice(bytes);
    state.position = end;
    OK
}

fn take<const N: usize>(parcel: *const Parcel) -> Result<[u8; N], i32> {
    let Some(parcel) = (unsafe { parcel.as_ref() }) else {
        return Err(BAD_VALUE);
    };
    let Ok(mut state) = parcel.state.lock() else {
        return Err(INVALID_OPERATION);
    };
    let end = state.position.checked_add(N).ok_or(BAD_VALUE)?;
    if end > state.bytes.len() {
        return Err(BAD_VALUE);
    }
    let mut result = [0; N];
    result.copy_from_slice(&state.bytes[state.position..end]);
    state.position = end;
    Ok(result)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_Class_define(
    descriptor: *const c_char,
    on_create: Option<OnCreate>,
    on_destroy: Option<OnDestroy>,
    on_transact: Option<OnTransact>,
) -> *mut BinderClass {
    if descriptor.is_null() || on_transact.is_none() {
        return ptr::null_mut();
    }
    let descriptor = unsafe { CStr::from_ptr(descriptor) }
        .to_bytes_with_nul()
        .to_vec()
        .into_boxed_slice();
    Box::into_raw(Box::new(BinderClass {
        descriptor,
        on_create,
        on_destroy,
        on_transact,
        on_dump: Mutex::new(None),
        sealed: AtomicBool::new(false),
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_Class_setOnDump(
    class: *mut BinderClass,
    callback: Option<OnDump>,
) {
    if let Some(class) = unsafe { class.as_ref() } {
        if !class.sealed.load(Ordering::Acquire) {
            if let Ok(mut slot) = class.on_dump.lock() {
                *slot = callback;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_Class_setTransactionCodeToFunctionNameMap(
    _class: *mut BinderClass,
    _map: Option<unsafe extern "C" fn(u32) -> *const c_char>,
) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_new(class: *const BinderClass, args: *mut c_void) -> *mut Binder {
    let Some(class_ref) = (unsafe { class.as_ref() }) else {
        return ptr::null_mut();
    };
    class_ref.sealed.store(true, Ordering::Release);
    let user_data = match class_ref.on_create {
        Some(f) => unsafe { f(args) },
        None => args,
    };
    Box::into_raw(Box::new(Binder {
        class: AtomicUsize::new(class as usize),
        user_data: AtomicUsize::new(user_data as usize),
        strong: AtomicUsize::new(1),
        destroyed: AtomicBool::new(false),
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_incStrong(b: *mut Binder) {
    if let Some(v) = unsafe { b.as_ref() } {
        v.strong.fetch_add(1, Ordering::Relaxed);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_decStrong(b: *mut Binder) {
    if let Some(v) = unsafe { b.as_ref() } {
        if v.strong.fetch_sub(1, Ordering::AcqRel) == 1 && !v.destroyed.swap(true, Ordering::AcqRel)
        {
            let c = v.class.load(Ordering::Acquire) as *const BinderClass;
            if let Some(f) = unsafe { c.as_ref() }.and_then(|x| x.on_destroy) {
                unsafe { f(v.user_data.load(Ordering::Acquire) as *mut c_void) }
            }
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_getUserData(b: *mut Binder) -> *mut c_void {
    unsafe { b.as_ref() }.map_or(ptr::null_mut(), |v| {
        v.user_data.load(Ordering::Acquire) as *mut c_void
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_isRemote(_: *mut Binder) -> bool {
    false
}
#[unsafe(no_mangle)]
pub extern "C" fn AIBinder_getCallingPid() -> i32 {
    std::process::id() as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_associateClass(b: *mut Binder, c: *const BinderClass) -> bool {
    let Some(v) = (unsafe { b.as_ref() }) else {
        return false;
    };
    let old = v.class.load(Ordering::Acquire);
    old == c as usize
        || (old == 0
            && v.class
                .compare_exchange(0, c as usize, Ordering::AcqRel, Ordering::Acquire)
                .is_ok())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_Weak_new(b: *mut Binder) -> *mut WeakBinder {
    if b.is_null() {
        ptr::null_mut()
    } else {
        Box::into_raw(Box::new(WeakBinder { binder: b }))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_Weak_delete(w: *mut WeakBinder) {
    if !w.is_null() {
        drop(unsafe { Box::from_raw(w) })
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_Weak_promote(w: *mut WeakBinder) -> *mut Binder {
    let Some(w) = (unsafe { w.as_ref() }) else {
        return ptr::null_mut();
    };
    let Some(b) = (unsafe { w.binder.as_ref() }) else {
        return ptr::null_mut();
    };
    if b.destroyed.load(Ordering::Acquire) {
        return ptr::null_mut();
    }
    b.strong.fetch_add(1, Ordering::Relaxed);
    w.binder
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_prepareTransaction(
    b: *mut Binder,
    input: *mut *mut Parcel,
) -> i32 {
    if b.is_null() || input.is_null() {
        return BAD_VALUE;
    }
    unsafe { *input = new_parcel() };
    OK
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_transact(
    b: *mut Binder,
    code: u32,
    input: *mut *mut Parcel,
    out: *mut *mut Parcel,
    _flags: u32,
) -> i32 {
    if b.is_null() || input.is_null() || out.is_null() {
        return BAD_VALUE;
    }
    let incoming = unsafe { *input };
    unsafe { *input = ptr::null_mut() };
    let output = new_parcel();
    unsafe { *out = output };
    let class = unsafe { b.as_ref() }.unwrap().class.load(Ordering::Acquire) as *const BinderClass;
    let result = unsafe { class.as_ref() }
        .and_then(|c| c.on_transact)
        .map_or(INVALID_OPERATION, |f| unsafe {
            f(b, code, incoming, output)
        });
    unsafe { AParcel_delete(incoming) };
    result
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIBinder_dump(
    b: *mut Binder,
    fd: c_int,
    args: *const *const c_char,
    n: u32,
) -> i32 {
    let Some(v) = (unsafe { b.as_ref() }) else {
        return BAD_VALUE;
    };
    let c = v.class.load(Ordering::Acquire) as *const BinderClass;
    let callback = unsafe { c.as_ref() }.and_then(|x| x.on_dump.lock().ok().and_then(|g| *g));
    callback.map_or(OK, |f| unsafe { f(b, fd, args, n) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_delete(p: *mut Parcel) {
    if p.is_null() {
        return;
    }
    let p = unsafe { Box::from_raw(p) };
    if let Ok(state) = p.state.lock() {
        for fd in &state.owned_fds {
            unsafe { aim_bionic_socket_broker_close(*fd) };
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_getDataPosition(p: *const Parcel) -> i32 {
    unsafe { p.as_ref() }
        .and_then(|p| p.state.lock().ok().map(|s| s.position as i32))
        .unwrap_or(-1)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_setDataPosition(p: *mut Parcel, pos: i32) -> i32 {
    let Some(p) = (unsafe { p.as_ref() }) else {
        return BAD_VALUE;
    };
    let Ok(mut s) = p.state.lock() else {
        return INVALID_OPERATION;
    };
    if pos < 0 || pos as usize > s.bytes.len() {
        return BAD_VALUE;
    }
    s.position = pos as usize;
    OK
}

macro_rules! primitive {
    ($write:ident,$read:ident,$ty:ty) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $write(p: *mut Parcel, v: $ty) -> i32 {
            append(p, &v.to_le_bytes())
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $read(p: *const Parcel, v: *mut $ty) -> i32 {
            if v.is_null() {
                return BAD_VALUE;
            }
            match take::<{ std::mem::size_of::<$ty>() }>(p) {
                Ok(b) => {
                    unsafe { *v = <$ty>::from_le_bytes(b) };
                    OK
                }
                Err(e) => e,
            }
        }
    };
}
primitive!(AParcel_writeInt32, AParcel_readInt32, i32);
primitive!(AParcel_writeInt64, AParcel_readInt64, i64);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_writeBool(p: *mut Parcel, v: bool) -> i32 {
    unsafe { AParcel_writeInt32(p, v as i32) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readBool(p: *const Parcel, v: *mut bool) -> i32 {
    let mut n = 0;
    let r = unsafe { AParcel_readInt32(p, &mut n) };
    if r == OK && !v.is_null() {
        unsafe { *v = n != 0 }
    }
    r
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_writeString(p: *mut Parcel, s: *const c_char, len: i32) -> i32 {
    if len < 0 {
        return unsafe { AParcel_writeInt32(p, -1) };
    };
    if s.is_null() {
        return BAD_VALUE;
    }
    let r = unsafe { AParcel_writeInt32(p, len) };
    if r != OK {
        return r;
    }
    append(p, unsafe {
        std::slice::from_raw_parts(s.cast::<u8>(), len as usize)
    })
}
type StringAllocator = unsafe extern "C" fn(*mut c_void, i32) -> *mut c_char;
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readString(
    p: *const Parcel,
    data: *mut c_void,
    alloc: Option<StringAllocator>,
) -> i32 {
    let mut len = 0;
    let r = unsafe { AParcel_readInt32(p, &mut len) };
    if r != OK {
        return r;
    }
    let Some(a) = alloc else { return BAD_VALUE };
    let out = unsafe { a(data, if len < 0 { -1 } else { len + 1 }) };
    if len < 0 {
        return OK;
    }
    if out.is_null() {
        return BAD_VALUE;
    }
    let Ok(bytes) = take_dynamic(p, len as usize) else {
        return BAD_VALUE;
    };
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.len());
        *out.add(bytes.len()) = 0
    };
    OK
}
fn take_dynamic(p: *const Parcel, n: usize) -> Result<Vec<u8>, i32> {
    let Some(p) = (unsafe { p.as_ref() }) else {
        return Err(BAD_VALUE);
    };
    let Ok(mut s) = p.state.lock() else {
        return Err(INVALID_OPERATION);
    };
    let end = s.position.checked_add(n).ok_or(BAD_VALUE)?;
    if end > s.bytes.len() {
        return Err(BAD_VALUE);
    }
    let v = s.bytes[s.position..end].to_vec();
    s.position = end;
    Ok(v)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_writeParcelFileDescriptor(p: *mut Parcel, fd: c_int) -> i32 {
    let copy = unsafe { aim_bionic_socket_broker_dup(fd) };
    if copy < 0 {
        return BAD_VALUE;
    }
    let Some(parcel) = (unsafe { p.as_ref() }) else {
        unsafe { aim_bionic_socket_broker_close(copy) };
        return BAD_VALUE;
    };
    if let Ok(mut s) = parcel.state.lock() {
        s.owned_fds.push(copy);
    }
    unsafe { AParcel_writeInt32(p, copy) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readParcelFileDescriptor(p: *const Parcel, fd: *mut c_int) -> i32 {
    let mut stored = 0;
    let r = unsafe { AParcel_readInt32(p, &mut stored) };
    if r != OK || fd.is_null() {
        return if r == OK { BAD_VALUE } else { r };
    }
    let copy = unsafe { aim_bionic_socket_broker_dup(stored) };
    if copy < 0 {
        return BAD_VALUE;
    }
    unsafe { *fd = copy };
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_writeStrongBinder(p: *mut Parcel, b: *mut Binder) -> i32 {
    if !b.is_null() {
        unsafe { AIBinder_incStrong(b) }
    }
    append(p, &(b as usize as u64).to_le_bytes())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readStrongBinder(p: *const Parcel, b: *mut *mut Binder) -> i32 {
    if b.is_null() {
        return BAD_VALUE;
    }
    match take::<8>(p) {
        Ok(v) => {
            let x = u64::from_le_bytes(v) as usize as *mut Binder;
            if !x.is_null() {
                unsafe { AIBinder_incStrong(x) }
            }
            unsafe { *b = x };
            OK
        }
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn AStatus_newOk() -> *mut Status {
    Box::into_raw(Box::new(Status { value: OK }))
}
#[unsafe(no_mangle)]
pub extern "C" fn AStatus_fromStatus(v: i32) -> *mut Status {
    Box::into_raw(Box::new(Status { value: v }))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AStatus_isOk(s: *const Status) -> bool {
    unsafe { s.as_ref() }.is_some_and(|s| s.value == OK)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AStatus_delete(s: *mut Status) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) })
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_writeStatusHeader(p: *mut Parcel, s: *const Status) -> i32 {
    unsafe { AParcel_writeInt32(p, s.as_ref().map_or(BAD_VALUE, |s| s.value)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readStatusHeader(p: *const Parcel, s: *mut *mut Status) -> i32 {
    if s.is_null() {
        return BAD_VALUE;
    }
    let mut v = 0;
    let r = unsafe { AParcel_readInt32(p, &mut v) };
    if r == OK {
        unsafe { *s = AStatus_fromStatus(v) }
    }
    r
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readStringArray(
    _: *const Parcel,
    _: *mut c_void,
    _: Option<unsafe extern "C" fn(*mut c_void, i32) -> bool>,
    _: Option<unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_char>,
) -> i32 {
    INVALID_OPERATION
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AParcel_readParcelableArray(
    _: *const Parcel,
    _: *mut c_void,
    _: Option<unsafe extern "C" fn(*mut c_void, i32) -> bool>,
    _: Option<unsafe extern "C" fn(*const Parcel, *mut c_void, i32) -> i32>,
) -> i32 {
    INVALID_OPERATION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aim_android_binder_ndk_resolve(
    soname: *const c_char,
    symbol: *const c_char,
    version: *const c_char,
) -> *mut c_void {
    if soname.is_null() || symbol.is_null() || version.is_null() {
        return ptr::null_mut();
    }
    if unsafe { CStr::from_ptr(soname) } != c"libbinder_ndk.so"
        || unsafe { CStr::from_ptr(version) } != c"LIBBINDER_NDK"
    {
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(symbol) }.to_bytes();
    macro_rules! r {
        ($n:ident) => {
            if name == stringify!($n).as_bytes() {
                return $n as *mut c_void;
            }
        };
    }
    r!(AIBinder_Class_define);
    r!(AIBinder_Class_setOnDump);
    r!(AIBinder_Class_setTransactionCodeToFunctionNameMap);
    r!(AIBinder_Weak_delete);
    r!(AIBinder_Weak_new);
    r!(AIBinder_Weak_promote);
    r!(AIBinder_associateClass);
    r!(AIBinder_decStrong);
    r!(AIBinder_dump);
    r!(AIBinder_getCallingPid);
    r!(AIBinder_getUserData);
    r!(AIBinder_incStrong);
    r!(AIBinder_isRemote);
    r!(AIBinder_new);
    r!(AIBinder_prepareTransaction);
    r!(AIBinder_transact);
    r!(AParcel_delete);
    r!(AParcel_getDataPosition);
    r!(AParcel_readBool);
    r!(AParcel_readInt32);
    r!(AParcel_readInt64);
    r!(AParcel_readParcelFileDescriptor);
    r!(AParcel_readParcelableArray);
    r!(AParcel_readStatusHeader);
    r!(AParcel_readString);
    r!(AParcel_readStringArray);
    r!(AParcel_readStrongBinder);
    r!(AParcel_setDataPosition);
    r!(AParcel_writeBool);
    r!(AParcel_writeInt32);
    r!(AParcel_writeInt64);
    r!(AParcel_writeParcelFileDescriptor);
    r!(AParcel_writeStatusHeader);
    r!(AParcel_writeString);
    r!(AParcel_writeStrongBinder);
    r!(AStatus_delete);
    r!(AStatus_fromStatus);
    r!(AStatus_isOk);
    r!(AStatus_newOk);
    ptr::null_mut()
}
