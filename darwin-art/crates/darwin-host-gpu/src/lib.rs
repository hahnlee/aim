//! Host-call module [`darwin_hostcall::module::GPU`]: the host side of the
//! guest GLES driver `libGLES_darwin.so` (`docs/gles-driver.md`).
//!
//! The guest driver exports every EGL and GLES entry point of the host's
//! ANGLE (Metal backend). Each one packs its arguments into the callee's
//! register image and makes one host call; [`forward`] loads the registers
//! and calls ANGLE's entry point. Guest pointers are host pointers, so
//! nothing is copied. The table of entry points is generated from the
//! Khronos registry by `tools/gen-gpu-thunks.py`.
//!
//! The guest's `EGLDisplay`s are handles of [`display`]: ANGLE's display,
//! and with it the process's Metal device, is created on first use in the
//! process that uses it. A guest fork child is a fresh process that takes
//! the handles over ([`fork_state`]) and loads ANGLE again when the guest
//! first calls it.
//!
//! Three functions are not plain forwards: [`FN_INIT`] loads ANGLE,
//! [`FN_IMPORT_BUFFER`] turns a mapped graphics buffer into an `EGLImage`
//! over a linear Metal texture ([`metal`]), and [`FN_PRESENT`] copies a
//! window surface's pbuffer into the buffer being queued ([`present`]).

mod display;
mod metal;
mod present;
#[rustfmt::skip]
mod table;

use std::ffi::{CStr, CString, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use darwin_hostcall::gpu::{
    FN_IMPORT_BUFFER, FN_INIT, FN_PRESENT, FN_TABLE_BASE, ImportBuffer, Init, Present, VERSION,
};
use darwin_hostcall::{HostModule, args_mut, errno, module};

pub static MODULE: HostModule = HostModule {
    id: module::GPU,
    name: "gpu",
    version: VERSION,
    call,
};

/// The ANGLE library that exports an entry point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lib {
    Egl,
    Gles,
}

/// A forwarded entry point and the shape of its register image.
pub struct Entry {
    pub name: &'static CStr,
    pub lib: Lib,
    /// x registers, d registers and stack words the callee takes.
    pub nx: u8,
    pub nd: u8,
    pub ns: u8,
}

static LIBRARY_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The directory holding ANGLE's `libEGL.dylib` and `libGLESv2.dylib`
/// (`linux-run --gpu DIR`). Without it the guest driver's `FN_INIT` fails
/// with `ENODEV`.
pub fn set_library_dir(dir: &Path) {
    let _ = LIBRARY_DIR.set(dir.to_owned());
}

const EINVAL: i64 = -(errno::EINVAL as i64);
const ENODEV: i64 = -(errno::ENODEV as i64);
const ENOSYS: i64 = -(errno::ENOSYS as i64);

/// ANGLE's address for each table entry, 0 until [`FN_INIT`] (or when ANGLE
/// lacks it).
static RESOLVED: [AtomicUsize; table::ENTRIES.len()] =
    [const { AtomicUsize::new(0) }; table::ENTRIES.len()];

unsafe extern "C" {
    /// `trampoline` below.
    fn darwin_gpu_forward(f: usize, regs: *const u64, stack: *const u64, words: u64) -> u64;
}

// Load x0-x7 and d0-d7 from `regs` (16 values), copy `words` stack words to
// the bottom of a 16-byte aligned area, and call `f`.
std::arch::global_asm!(
    ".globl _darwin_gpu_forward",
    ".p2align 2",
    "_darwin_gpu_forward:",
    "stp x29, x30, [sp, #-16]!",
    "mov x29, sp",
    "add x9, x3, #1",
    "and x9, x9, #0xfffffffffffffffe",
    "sub sp, sp, x9, lsl #3",
    "mov x10, #0",
    "1:",
    "cmp x10, x3",
    "b.hs 2f",
    "ldr x11, [x2, x10, lsl #3]",
    "str x11, [sp, x10, lsl #3]",
    "add x10, x10, #1",
    "b 1b",
    "2:",
    "mov x16, x0",
    "mov x17, x1",
    "ldp d0, d1, [x17, #64]",
    "ldp d2, d3, [x17, #80]",
    "ldp d4, d5, [x17, #96]",
    "ldp d6, d7, [x17, #112]",
    "ldp x0, x1, [x17]",
    "ldp x2, x3, [x17, #16]",
    "ldp x4, x5, [x17, #32]",
    "ldp x6, x7, [x17, #48]",
    "blr x16",
    "mov sp, x29",
    "ldp x29, x30, [sp], #16",
    "ret",
);

/// Call ANGLE's entry point `i` with the guest's register image.
///
/// # Safety
/// `args` must point to `len` readable bytes when `len` is not 0, and the
/// values must be valid arguments for that entry point.
#[inline]
unsafe fn forward(i: usize, args: u64, len: u64) -> i64 {
    let Some(e) = table::ENTRIES.get(i) else {
        return ENOSYS;
    };
    let (nx, nd, ns) = (e.nx as usize, e.nd as usize, e.ns as usize);
    if len != ((nx + nd + ns) * 8) as u64 {
        return EINVAL;
    }
    let mut f = RESOLVED[i].load(Ordering::Relaxed);
    if f == 0 && RELOAD.load(Ordering::Relaxed) && load().is_ok() {
        f = RESOLVED[i].load(Ordering::Relaxed);
    }
    if f == 0 {
        return 0;
    }
    let src = args as *const u64;
    let mut regs = [0u64; 16];
    // SAFETY: `len` covers nx + nd + ns values at `src` (checked above).
    unsafe {
        if nx > 0 {
            std::ptr::copy_nonoverlapping(src, regs.as_mut_ptr(), nx);
        }
        if nd > 0 {
            std::ptr::copy_nonoverlapping(src.add(nx), regs.as_mut_ptr().add(8), nd);
        }
        let stack = if ns > 0 {
            src.add(nx + nd)
        } else {
            regs.as_ptr()
        };
        if e.lib == Lib::Egl {
            if let Some(handle) = display::get(e.name, &regs) {
                return handle as i64;
            }
            // EGL creates Metal objects; guest threads have no
            // autorelease pool of their own.
            let pool = metal::pool_push();
            // A display is always the first argument.
            let handle = regs[0] as usize;
            if nx > 0 {
                regs[0] = display::host(handle) as u64;
            }
            let mut r = darwin_gpu_forward(f, regs.as_ptr(), stack, ns as u64);
            if e.name == c"eglGetCurrentDisplay" {
                r = display::guest(r as usize) as u64;
            } else if e.name == c"eglInitialize" && r as u32 == 1 {
                display::initialized(handle);
            }
            metal::pool_pop(pool);
            r as i64
        } else {
            darwin_gpu_forward(f, regs.as_ptr(), stack, ns as u64) as i64
        }
    }
}

/// The host-call function id of a forwarded entry point.
pub fn function(name: &CStr) -> Option<u32> {
    let i = table::ENTRIES.iter().position(|e| e.name == name)?;
    Some(FN_TABLE_BASE + i as u32)
}

/// The generated table's hash and length, which [`FN_INIT`] checks.
pub fn table_identity() -> (u64, u64) {
    (table::TABLE_HASH, table::ENTRIES.len() as u64)
}

/// ANGLE's address of an entry point of the table, after [`FN_INIT`].
fn resolved(name: &CStr) -> usize {
    table::ENTRIES
        .iter()
        .position(|e| e.name == name)
        .map_or(0, |i| RESOLVED[i].load(Ordering::Relaxed))
}

fn dlopen(dir: &Path, name: &str) -> *mut c_void {
    let path = CString::new(dir.join(name).as_os_str().as_bytes()).unwrap();
    // SAFETY: a NUL-terminated path.
    let h = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if h.is_null() {
        // SAFETY: dlerror returns a string or null.
        let err = unsafe { libc::dlerror() };
        let err = if err.is_null() {
            String::new()
        } else {
            // SAFETY: a NUL-terminated string from dlerror.
            unsafe { CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned()
        };
        eprintln!("[gpu] cannot load {}: {err}", dir.join(name).display());
    }
    h
}

/// Set in a fork child whose parent had loaded ANGLE: the guest's driver
/// is initialized, so the first forwarded call loads it here.
static RELOAD: AtomicBool = AtomicBool::new(false);

/// The state a guest fork child takes over: whether ANGLE is loaded, and
/// the display handles.
pub fn fork_state() -> Vec<u8> {
    let loaded = RESOLVED.iter().any(|f| f.load(Ordering::Relaxed) != 0);
    let mut v = vec![loaded as u8];
    v.extend(display::fork_state());
    v
}

/// Take over a parent's [`fork_state`].
pub fn restore_fork_state(b: &[u8]) {
    let Some((&loaded, displays)) = b.split_first() else {
        return;
    };
    RELOAD.store(loaded != 0, Ordering::Relaxed);
    display::restore_fork_state(displays);
}

/// Load ANGLE once per process and resolve every entry point.
fn load() -> Result<(), i64> {
    static LOADED: OnceLock<Result<(), i64>> = OnceLock::new();
    *LOADED.get_or_init(|| {
        let dir = LIBRARY_DIR.get().ok_or(ENODEV)?;
        let (egl, gles) = (dlopen(dir, "libEGL.dylib"), dlopen(dir, "libGLESv2.dylib"));
        if egl.is_null() || gles.is_null() {
            return Err(ENODEV);
        }
        for (e, slot) in table::ENTRIES.iter().zip(&RESOLVED) {
            let lib = if e.lib == Lib::Egl { egl } else { gles };
            // SAFETY: a library handle and a NUL-terminated name.
            let f = unsafe { libc::dlsym(lib, e.name.as_ptr()) };
            slot.store(f as usize, Ordering::Relaxed);
        }
        Ok(())
    })
}

fn init(a: &Init) -> i64 {
    if a.table_hash != table::TABLE_HASH || a.table_len != table::ENTRIES.len() as u64 {
        eprintln!(
            "[gpu] guest driver table {:#x} ({} entries) differs from the host's {:#x}",
            a.table_hash,
            a.table_len,
            table::TABLE_HASH
        );
        return EINVAL;
    }
    let words = table::ENTRIES.len().div_ceil(64);
    if a.resolved == 0 || a.resolved_words != words as u64 {
        return EINVAL;
    }
    if let Err(e) = load() {
        return e;
    }
    // SAFETY: the guest's bitmap of `words` u64s (checked non-null above).
    let bits = unsafe { std::slice::from_raw_parts_mut(a.resolved as *mut u64, words) };
    bits.fill(0);
    for (i, slot) in RESOLVED.iter().enumerate() {
        if slot.load(Ordering::Relaxed) != 0 {
            bits[i / 64] |= 1 << (i % 64);
        }
    }
    0
}

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    if func >= FN_TABLE_BASE {
        // SAFETY: the guest's argument block for this entry point.
        return unsafe { forward((func - FN_TABLE_BASE) as usize, args, len) };
    }
    // SAFETY (all arms): the registry passes the guest's argument block.
    match func {
        FN_INIT => match unsafe { args_mut::<Init>(args, len) } {
            Ok(a) => init(a),
            Err(e) => e,
        },
        FN_IMPORT_BUFFER => match unsafe { args_mut::<ImportBuffer>(args, len) } {
            Ok(a) => metal::import_buffer(a),
            Err(e) => e,
        },
        FN_PRESENT => match unsafe { args_mut::<Present>(args, len) } {
            Ok(a) => present::present(a),
            Err(e) => e,
        },
        _ => ENOSYS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn many(
        a: u32,
        b: i64,
        c: u8,
        d: i16,
        e: u32,
        f: u64,
        g: i32,
        h: u32,
        i: u32,
        j: u64,
        x: f32,
        y: f32,
    ) -> u64 {
        assert_eq!((a, b, c, d, e, f, g, h), (1, -2, 3, -4, 5, 6, -7, 8));
        assert_eq!((i, j, x, y), (9, 10, 0.5, -1.5));
        99
    }

    #[test]
    fn trampoline_follows_the_apple_arm64_convention() {
        // Eight x registers, then i (4 bytes at offset 0) and j (8 bytes at
        // offset 8) on the stack, and two d registers.
        let mut regs = [0u64; 16];
        regs[..8].copy_from_slice(&[1, -2i64 as u64, 3, -4i64 as u64, 5, 6, -7i64 as u64, 8]);
        regs[8] = 0.5f32.to_bits() as u64;
        regs[9] = (-1.5f32).to_bits() as u64;
        let stack = [9u64, 10];
        // SAFETY: the register image matches `many`'s signature.
        let r = unsafe { darwin_gpu_forward(many as usize, regs.as_ptr(), stack.as_ptr(), 2) };
        assert_eq!(r, 99);
    }

    #[test]
    fn calls_are_checked() {
        // SAFETY: rejected before any access.
        unsafe {
            assert_eq!(call(FN_TABLE_BASE + 1_000_000, 0, 0), ENOSYS);
            // glClear takes one register; a wrong length is refused.
            let i = table::ENTRIES
                .iter()
                .position(|e| e.name == c"glClear")
                .unwrap();
            assert_eq!(call(FN_TABLE_BASE + i as u32, 0, 16), EINVAL);
            // Unresolved (ANGLE not loaded in this test): a no-op.
            let v = [0x4000u64];
            assert_eq!(call(FN_TABLE_BASE + i as u32, v.as_ptr() as u64, 8), 0);
        }
        let bad = Init {
            table_hash: 1,
            ..Init::default()
        };
        assert_eq!(init(&bad), EINVAL);
    }
}
