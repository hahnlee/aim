//! Host-call module [`aim_hostcall::module::VULKAN`]: the host side of the
//! guest Vulkan driver `vulkan.aim.so` (`docs/vulkan-driver.md`).
//!
//! The guest driver hands the original libvulkan a thunk for every Vulkan
//! command of the host's MoltenVK. Each one packs its arguments into the
//! callee's register image and makes one host call; [`forward`] loads the
//! registers and calls MoltenVK's entry point. Guest pointers are host
//! pointers, so structures and their `pNext` chains pass through untouched,
//! and MoltenVK's dispatchable handles already start with the loader word
//! the original loader expects. The table of entry points is generated from
//! the Khronos registry by `tools/gen-vulkan-thunks.py`.
//!
//! Three functions are not plain forwards: [`FN_ATTACH`] makes a mapped
//! graphics buffer the storage of an image, and [`FN_FENCE`] and
//! [`FN_SIGNAL`] connect sync_files to timeline semaphores through their
//! Metal shared events ([`metal`]).

mod metal;
#[rustfmt::skip]
mod table;

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use aim_hostcall::vulkan::{
    Attach, FN_ATTACH, FN_FENCE, FN_INIT, FN_SIGNAL, FN_TABLE_BASE, Init, Timeline, VERSION,
};
use aim_hostcall::{HostModule, args_mut, errno, module};

pub static MODULE: HostModule = HostModule {
    id: module::VULKAN,
    name: "vulkan",
    version: VERSION,
    call,
};

/// A forwarded entry point and the shape of its register image.
pub struct Entry {
    /// MoltenVK's exported name for it.
    pub name: &'static CStr,
    /// x registers, d registers and stack words the callee takes.
    pub nx: u8,
    pub nd: u8,
    pub ns: u8,
    /// Run the call in an autorelease pool: guest threads have none, and
    /// MoltenVK autoreleases Metal objects outside command recording.
    pub pool: bool,
}

static LIBRARY_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The directory holding `libMoltenVK.dylib` (`linux-run --vulkan DIR`).
/// Without it the guest driver's `FN_INIT` fails with `ENODEV`.
pub fn set_library_dir(dir: &Path) {
    let _ = LIBRARY_DIR.set(dir.to_owned());
}

const EBADF: i64 = -(errno::EBADF as i64);
const EINVAL: i64 = -(errno::EINVAL as i64);
const ENODEV: i64 = -(errno::ENODEV as i64);
const ENOSYS: i64 = -(errno::ENOSYS as i64);

/// MoltenVK's address for each table entry, 0 until loaded (or when
/// MoltenVK lacks it).
static RESOLVED: [AtomicUsize; table::ENTRIES.len()] =
    [const { AtomicUsize::new(0) }; table::ENTRIES.len()];

unsafe extern "C" {
    /// `trampoline` below.
    fn darwin_vulkan_forward(f: usize, regs: *const u64, stack: *const u64, words: u64) -> u64;
}

// Load x0-x7 and d0-d7 from `regs` (16 values), copy `words` stack words to
// the bottom of a 16-byte aligned area, and call `f`. The GLES module has
// the same trampoline (crates/aim-host-gpu).
std::arch::global_asm!(
    ".globl _darwin_vulkan_forward",
    ".p2align 2",
    "_darwin_vulkan_forward:",
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

/// Call MoltenVK's entry point `i` with the guest's register image.
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
    if f == 0 {
        // A guest fork child is a fresh process: its driver was initialized
        // in the parent, so MoltenVK is loaded on its first call here.
        if load().is_err() {
            return 0;
        }
        f = RESOLVED[i].load(Ordering::Relaxed);
        if f == 0 {
            return 0;
        }
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
        if e.pool {
            let pool = metal::pool_push();
            let r = darwin_vulkan_forward(f, regs.as_ptr(), stack, ns as u64);
            metal::pool_pop(pool);
            r as i64
        } else {
            darwin_vulkan_forward(f, regs.as_ptr(), stack, ns as u64) as i64
        }
    }
}

/// The host-call function id of a forwarded entry point, by MoltenVK's
/// exported name.
pub fn function(name: &CStr) -> Option<u32> {
    let i = table::ENTRIES.iter().position(|e| e.name == name)?;
    Some(FN_TABLE_BASE + i as u32)
}

/// The generated table's hash and length, which [`FN_INIT`] checks.
pub fn table_identity() -> (u64, u64) {
    (table::TABLE_HASH, table::ENTRIES.len() as u64)
}

/// MoltenVK's own entry points the host uses (not in the table).
struct Private {
    get_mtl_device: usize,
    set_mtl_texture: usize,
    pixel_format: usize,
    bytes_per_block: usize,
    export_objects: usize,
}

static PRIVATE: OnceLock<Private> = OnceLock::new();

fn dlerror() -> String {
    // SAFETY: dlerror returns a string or null.
    let err = unsafe { libc::dlerror() };
    if err.is_null() {
        String::new()
    } else {
        // SAFETY: a NUL-terminated string from dlerror.
        unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Load MoltenVK once per process and resolve every entry point.
fn load() -> Result<(), i64> {
    static LOADED: OnceLock<Result<(), i64>> = OnceLock::new();
    *LOADED.get_or_init(|| {
        let dir = LIBRARY_DIR.get().ok_or(ENODEV)?;
        let path = dir.join("libMoltenVK.dylib");
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| ENODEV)?;
        // SAFETY: a NUL-terminated path.
        let lib = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            eprintln!("[vulkan] cannot load {}: {}", path.display(), dlerror());
            return Err(ENODEV);
        }
        let sym = |name: &CStr| {
            // SAFETY: a library handle and a NUL-terminated name.
            unsafe { libc::dlsym(lib, name.as_ptr()) as usize }
        };
        for (e, slot) in table::ENTRIES.iter().zip(&RESOLVED) {
            slot.store(sym(e.name), Ordering::Relaxed);
        }
        let private = Private {
            get_mtl_device: sym(c"vkGetMTLDeviceMVK"),
            set_mtl_texture: sym(c"vkSetMTLTextureMVK"),
            pixel_format: sym(c"mvkMTLPixelFormatFromVkFormat"),
            bytes_per_block: sym(c"mvkMTLPixelFormatBytesPerBlock"),
            export_objects: sym(c"vkExportMetalObjectsEXT"),
        };
        if private.get_mtl_device == 0
            || private.set_mtl_texture == 0
            || private.pixel_format == 0
            || private.bytes_per_block == 0
            || private.export_objects == 0
        {
            eprintln!(
                "[vulkan] {} lacks MoltenVK's Metal entry points",
                path.display()
            );
            return Err(ENODEV);
        }
        let _ = PRIVATE.set(private);
        Ok(())
    })
}

fn init(a: &Init) -> i64 {
    if a.table_hash != table::TABLE_HASH || a.table_len != table::ENTRIES.len() as u64 {
        eprintln!(
            "[vulkan] guest driver table {:#x} ({} entries) differs from the host's {:#x}",
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

fn attach(a: &mut Attach) -> i64 {
    match PRIVATE.get() {
        Some(p) => metal::attach(p, a),
        None => ENODEV,
    }
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
        FN_ATTACH => match unsafe { args_mut::<Attach>(args, len) } {
            Ok(a) => attach(a),
            Err(e) => e,
        },
        FN_FENCE => match (unsafe { args_mut::<Timeline>(args, len) }, PRIVATE.get()) {
            (Ok(a), Some(p)) => {
                metal::fence(p, a).map_or_else(|e| e, |f| aim_sync_file::give_to_guest(f).map(|fd|fd as i64).unwrap_or_else(|error|-(error as i64)))
            }
            (Err(e), _) => e,
            (_, None) => ENODEV,
        },
        FN_SIGNAL => match (unsafe { args_mut::<Timeline>(args, len) }, PRIVATE.get()) {
            (Ok(a), Some(p)) => metal::signal(p, a),
            (Err(e), _) => e,
            (_, None) => ENODEV,
        },
        _ => ENOSYS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn many(
        a: u64,
        b: u32,
        c: u32,
        d: u32,
        e: u32,
        f: u64,
        g: u32,
        h: u64,
        i: u32,
        j: u64,
        x: f32,
    ) -> u64 {
        assert_eq!((a, b, c, d, e, f, g, h), (1, 2, 3, 4, 5, 6, 7, 8));
        assert_eq!((i, j, x), (9, 10, 0.5));
        99
    }

    #[test]
    fn trampoline_follows_the_apple_arm64_convention() {
        // vkCmdPipelineBarrier's shape: eight x registers, then a u32 at
        // stack offset 0 and a pointer at offset 8.
        let mut regs = [0u64; 16];
        regs[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        regs[8] = 0.5f32.to_bits() as u64;
        let stack = [9u64, 10];
        // SAFETY: the register image matches `many`'s signature.
        let r = unsafe { darwin_vulkan_forward(many as usize, regs.as_ptr(), stack.as_ptr(), 2) };
        assert_eq!(r, 99);
    }

    #[test]
    fn calls_are_checked() {
        // SAFETY: rejected before any access.
        unsafe {
            assert_eq!(call(FN_TABLE_BASE + 1_000_000, 0, 0), ENOSYS);
            let i = function(c"vkCmdDraw").unwrap();
            // vkCmdDraw takes five registers; a wrong length is refused.
            assert_eq!(call(i, 0, 16), EINVAL);
        }
        let bad = Init {
            table_hash: 1,
            ..Init::default()
        };
        assert_eq!(init(&bad), EINVAL);
        let mut attach_args = Attach::default();
        assert_eq!(attach(&mut attach_args), ENODEV);
        let mut timeline = Timeline::default();
        // SAFETY: a valid argument block, refused before MoltenVK loads.
        let r = unsafe {
            call(
                FN_FENCE,
                (&raw mut timeline) as u64,
                size_of::<Timeline>() as u64,
            )
        };
        assert_eq!(r, ENODEV);
    }
}
