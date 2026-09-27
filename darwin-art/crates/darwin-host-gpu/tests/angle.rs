//! The module against the pinned ANGLE build, driven the way the guest
//! driver drives it (register images through `MODULE.call`). Skipped when
//! the ANGLE build is absent.

use std::ffi::CStr;
use std::path::Path;
use std::time::Instant;

use darwin_host_gpu::{MODULE, function, set_library_dir, table_identity};
use darwin_hostcall::gpu::{FN_IMPORT_BUFFER, FN_INIT, FN_PRESENT, ImportBuffer, Init, Present};

const ANGLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/angle-source/out/DarwinArtRelease"
);

/// Call a forwarded entry point with integer-class arguments.
fn x(name: &CStr, args: &[u64]) -> u64 {
    let id = function(name).unwrap_or_else(|| panic!("{name:?} not forwarded"));
    // SAFETY: the values are this entry point's register image.
    unsafe { (MODULE.call)(id, args.as_ptr() as u64, (args.len() * 8) as u64) as u64 }
}

fn block<T>(func: u32, a: &mut T) -> i64 {
    // SAFETY: the argument block of `func`.
    unsafe { (MODULE.call)(func, a as *mut T as u64, size_of::<T>() as u64) }
}

#[test]
fn renders_into_a_shared_memory_buffer() {
    if !Path::new(ANGLE).join("libGLESv2.dylib").exists() {
        eprintln!("skipped: no ANGLE build at {ANGLE}");
        return;
    }
    set_library_dir(Path::new(ANGLE));
    let (hash, len) = table_identity();
    let mut bits = vec![0u64; (len as usize).div_ceil(64)];
    let mut init = Init {
        table_hash: hash,
        table_len: len,
        resolved: bits.as_mut_ptr() as u64,
        resolved_words: bits.len() as u64,
    };
    assert_eq!(block(FN_INIT, &mut init), 0);
    let resolved: u32 = bits.iter().map(|w| w.count_ones()).sum();
    assert_eq!(resolved as u64, len, "every generated entry point resolves");

    let display_attribs = [0x3203i64, 0x3489, 0x3038];
    let dpy = x(
        c"eglGetPlatformDisplay",
        &[0x3202, 0, display_attribs.as_ptr() as u64],
    );
    assert_ne!(dpy, 0);
    let (mut major, mut minor) = (0i32, 0i32);
    assert_eq!(
        x(
            c"eglInitialize",
            &[
                dpy,
                &mut major as *mut _ as u64,
                &mut minor as *mut _ as u64
            ]
        ),
        1
    );
    let config_attribs = [
        0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3040, 0x40, 0x3033, 1, 0x3038i32,
    ];
    let (mut config, mut n) = (0u64, 0i32);
    x(
        c"eglChooseConfig",
        &[
            dpy,
            config_attribs.as_ptr() as u64,
            &mut config as *mut _ as u64,
            1,
            &mut n as *mut _ as u64,
        ],
    );
    assert_eq!(n, 1);
    let context_attribs = [0x3098, 3, 0x3038i32];
    let ctx = x(
        c"eglCreateContext",
        &[dpy, config, 0, context_attribs.as_ptr() as u64],
    );
    assert_ne!(ctx, 0);
    let (w, h) = (64u64, 64u64);
    let surface_attribs = [0x3057, w as i32, 0x3056, h as i32, 0x3038];
    let surface = x(
        c"eglCreatePbufferSurface",
        &[dpy, config, surface_attribs.as_ptr() as u64],
    );
    assert_eq!(x(c"eglMakeCurrent", &[dpy, surface, surface, ctx]), 1);

    // A buffer as the allocator makes it: shared memory, pixels at 0.
    let stride = w * 4;
    let size = 16384;
    // SAFETY: an anonymous shared mapping we own.
    let mem = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(mem, libc::MAP_FAILED);
    let mut import = ImportBuffer {
        display: dpy,
        address: mem as u64,
        length: size as u64,
        width: w as u32,
        height: h as u32,
        stride_bytes: stride as u32,
        format: 1,
        image: 0,
    };
    assert_eq!(block(FN_IMPORT_BUFFER, &mut import), 0);
    assert_ne!(import.image, 0);

    // Green everywhere, blue in the bottom quarter (GL's y = 0).
    let clear_color = function(c"glClearColor").unwrap();
    let color = |r: f32, g: f32, b: f32| {
        let v = [
            r.to_bits() as u64,
            g.to_bits() as u64,
            b.to_bits() as u64,
            1f32.to_bits() as u64,
        ];
        // SAFETY: four d-register values.
        unsafe { (MODULE.call)(clear_color, v.as_ptr() as u64, 32) };
    };
    color(0.0, 1.0, 0.0);
    x(c"glClear", &[0x4000]);
    x(c"glEnable", &[0x0c11]);
    x(c"glScissor", &[0, 0, w, h / 4]);
    color(0.0, 0.0, 1.0);
    x(c"glClear", &[0x4000]);
    x(c"glDisable", &[0x0c11]);
    let mut present = Present {
        image: import.image,
        src_width: w as u32,
        src_height: h as u32,
        dst_width: w as u32,
        dst_height: h as u32,
    };
    assert_eq!(block(FN_PRESENT, &mut present), 0);
    // SAFETY: the mapping above.
    let px = |row: u64| unsafe { *(mem as *const u32).add((row * stride / 4) as usize) };
    // Row 0 of the buffer is the top of the image.
    assert_eq!(px(0), 0xff00_ff00, "top row green");
    assert_eq!(px(h - 1), 0xffff_0000, "bottom row blue");
    assert_eq!(x(c"glGetError", &[]), 0);

    let n = 1_000_000;
    let start = Instant::now();
    for _ in 0..n {
        color(0.0, 0.0, 0.0);
    }
    eprintln!(
        "host dispatch + ANGLE glClearColor: {:.1} ns/call",
        start.elapsed().as_nanos() as f64 / n as f64
    );
    x(c"eglDestroyImageKHR", &[dpy, import.image]);
    x(c"eglMakeCurrent", &[dpy, 0, 0, 0]);
}
