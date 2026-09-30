//! mremap with Linux semantics: the move bionic's CFI shadow update makes
//! (MREMAP_MAYMOVE | MREMAP_FIXED), growing (over the file, for a file
//! mapping), and shrinking.

use aim_linux_abi::{context, diag, patch};

std::arch::global_asm!(
    ".p2align 2",
    ".globl _t_mremap, _t_mremap_end",
    "_t_mremap:", // x0..x5 = arguments, x6 = syscall number
    "mov x8, x6",
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

/// Linux syscall `nr` through the layer.
fn syscall(nr: u64, a: [u64; 6]) -> i64 {
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
    let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
        unsafe { std::mem::transmute(code) };
    f(a[0], a[1], a[2], a[3], a[4], a[5], nr)
}

fn mremap(old: u64, old_len: usize, new_len: usize, flags: u64, new_addr: u64) -> i64 {
    syscall(
        216,
        [old, old_len as u64, new_len as u64, flags, new_addr, 0],
    )
}

/// Tests check where mappings land and what is unmapped; one at a time,
/// so no other test maps into the pages they free.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
    let p = map(3 * PAGE, 3);
    assert_eq!(mremap(p, 3 * PAGE, PAGE, 0, 0) as u64, p);
    assert!(mapped(p));
    assert!(!mapped(p + PAGE as u64));
}

#[test]
fn invalid_requests_fail_with_einval() {
    let _serial = serial();
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

/// A file of `pages` pages, page `i` filled with `i + 1`.
fn paged_file(name: &str, pages: usize) -> std::fs::File {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let data: Vec<u8> = (0..pages * PAGE).map(|i| (i / PAGE + 1) as u8).collect();
    std::fs::write(&path, data).unwrap();
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap()
}

/// The guest's mmap of `f` from its second page, followed by `room` free
/// bytes, or by mapped ones so growing it in place is impossible when
/// `blocked`.
fn map_file(f: &std::fs::File, len: usize, flags: i32, room: usize, blocked: bool) -> u64 {
    use std::os::fd::AsRawFd;
    let at = map(len + room, libc::PROT_READ);
    let prot = (libc::PROT_READ | libc::PROT_WRITE) as u64;
    let fixed = (flags | libc::MAP_FIXED) as u64;
    let r = syscall(
        222,
        [
            at,
            len as u64,
            prot,
            fixed,
            f.as_raw_fd() as u64,
            PAGE as u64,
        ],
    );
    assert_eq!(r as u64, at);
    if !blocked {
        // SAFETY: the page after the mapping, ours.
        assert_eq!(
            unsafe { libc::munmap((at + len as u64) as *mut _, room) },
            0
        );
    }
    at
}

fn file_byte(f: &std::fs::File, off: usize) -> u8 {
    use std::os::unix::fs::FileExt;
    let mut b = [0u8];
    f.read_exact_at(&mut b, off as u64).unwrap();
    b[0]
}

#[test]
fn growing_a_shared_file_mapping_maps_the_following_file_pages() {
    let _serial = serial();
    for blocked in [false, true] {
        let f = paged_file(&format!("mremap-shared-{blocked}"), 4);
        let p = map_file(&f, PAGE, libc::MAP_SHARED, 2 * PAGE, blocked);
        let r = mremap(p, PAGE, 3 * PAGE, MAYMOVE, 0);
        assert!(r > 0, "{r}");
        let r = r as u64;
        assert_eq!(r == p, !blocked, "grown in place unless blocked");
        assert_eq!(byte(r), 2);
        assert_eq!(byte(r + PAGE as u64), 3);
        assert_eq!(byte(r + 2 * PAGE as u64), 4);
        // Writes through the new part reach the file.
        fill(r + 2 * PAGE as u64, 1, 0x77);
        assert_eq!(file_byte(&f, 3 * PAGE), 0x77);
    }
}

#[test]
fn growing_a_private_file_mapping_copies_the_following_file_pages() {
    let _serial = serial();
    for blocked in [false, true] {
        let f = paged_file(&format!("mremap-private-{blocked}"), 3);
        let p = map_file(&f, PAGE, libc::MAP_PRIVATE, PAGE, blocked);
        fill(p, 1, 0x55);
        let r = mremap(p, PAGE, 2 * PAGE, MAYMOVE, 0) as u64;
        assert_eq!(r == p, !blocked, "grown in place unless blocked");
        assert_eq!(byte(r), 0x55, "written page kept");
        assert_eq!(byte(r + PAGE as u64), 3);
        // Written pages stay private.
        fill(r + PAGE as u64, 1, 0x66);
        assert_eq!(file_byte(&f, 2 * PAGE), 3);
    }
}

#[test]
fn a_fixed_move_grows_a_file_mapping_over_the_file() {
    let _serial = serial();
    let f = paged_file("mremap-fixed", 4);
    let p = map_file(&f, PAGE, libc::MAP_SHARED, PAGE, true);
    let dst = map(3 * PAGE, libc::PROT_READ);
    assert_eq!(mremap(p, PAGE, 3 * PAGE, MAYMOVE | FIXED, dst) as u64, dst);
    assert_eq!(byte(dst + 2 * PAGE as u64), 4);
}
