//! mremap with Linux semantics: the move bionic's CFI shadow update makes
//! (MREMAP_MAYMOVE | MREMAP_FIXED), growing, and shrinking.

use darwin_linux_abi::{context, diag, patch};

std::arch::global_asm!(
    ".p2align 2",
    ".globl _t_mremap, _t_mremap_end",
    "_t_mremap:", // x0..x4 = mremap arguments
    "mov x8, #216",
    "svc #0",
    "ret",
    "_t_mremap_end:",
);

unsafe extern "C" {
    static t_mremap: u8;
    static t_mremap_end: u8;
}

const PAGE: usize = 16384;
const MAYMOVE: u64 = 1;
const FIXED: u64 = 2;

fn mremap(old: u64, old_len: usize, new_len: usize, flags: u64, new_addr: u64) -> i64 {
    context::init_thread();
    diag::install_signal_handlers();
    static CODE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let code = *CODE.get_or_init(|| {
        // SAFETY: symbols from global_asm above; fresh RW page, copied,
        // rewritten, then protected RX.
        unsafe {
            let (s, e) = (&t_mremap as *const u8, &t_mremap_end as *const u8);
            let p = map(PAGE, libc::PROT_READ | libc::PROT_WRITE);
            std::ptr::copy_nonoverlapping(s, p as *mut u8, e as usize - s as usize);
            patch::rewrite_region_with(p, (e as usize - s as usize) as u64, true);
            assert_eq!(
                libc::mprotect(p as *mut _, PAGE, libc::PROT_READ | libc::PROT_EXEC),
                0
            );
            p
        }
    });
    // SAFETY: the snippet follows the C ABI.
    let f: extern "C" fn(u64, u64, u64, u64, u64) -> i64 = unsafe { std::mem::transmute(code) };
    f(old, old_len as u64, new_len as u64, flags, new_addr)
}

fn map(len: usize, prot: i32) -> u64 {
    // SAFETY: fresh anonymous mapping.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            prot,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(p, libc::MAP_FAILED);
    p as u64
}

fn mapped(addr: u64) -> bool {
    patch::vm::region(addr).is_some_and(|(start, _, _, _)| start <= addr)
}

fn fill(addr: u64, len: usize, v: u8) {
    // SAFETY: writable mapping of at least `len` bytes.
    unsafe { std::ptr::write_bytes(addr as *mut u8, v, len) };
}

fn byte(addr: u64) -> u8 {
    // SAFETY: readable mapping.
    unsafe { (addr as *const u8).read() }
}

#[test]
fn fixed_move_replaces_the_target_and_unmaps_the_source() {
    let (src, dst) = (map(2 * PAGE, 3), map(2 * PAGE, libc::PROT_READ));
    fill(src, 2 * PAGE, 0x5a);
    let r = mremap(src, 2 * PAGE, 2 * PAGE, MAYMOVE | FIXED, dst);
    assert_eq!(r as u64, dst);
    assert_eq!(byte(dst + PAGE as u64 + 7), 0x5a);
    assert!(!mapped(src), "source unmapped");
    fill(dst, 1, 1); // still writable: protection moved with the pages
}

#[test]
fn growing_keeps_contents_and_zero_fills_the_tail() {
    let p = map(PAGE, 3);
    fill(p, PAGE, 0x33);
    let r = mremap(p, PAGE, 4 * PAGE, MAYMOVE, 0);
    assert!(r > 0);
    let r = r as u64;
    assert_eq!(byte(r), 0x33);
    assert_eq!(byte(r + 3 * PAGE as u64), 0);
    fill(r, 4 * PAGE, 1);
}

#[test]
fn shrinking_unmaps_the_tail() {
    let p = map(3 * PAGE, 3);
    assert_eq!(mremap(p, 3 * PAGE, PAGE, 0, 0) as u64, p);
    assert!(mapped(p));
    assert!(!mapped(p + PAGE as u64));
}

#[test]
fn invalid_requests_fail_with_einval() {
    let p = map(PAGE, 3);
    let einval = -(libc::EINVAL as i64);
    assert_eq!(mremap(p + 1, PAGE, PAGE, MAYMOVE, 0), einval, "unaligned");
    assert_eq!(
        mremap(p, PAGE, PAGE, FIXED, p + 0x10000),
        einval,
        "FIXED needs MAYMOVE"
    );
    assert_eq!(
        mremap(p, PAGE, 2 * PAGE, MAYMOVE | FIXED, p),
        einval,
        "overlap"
    );
}
