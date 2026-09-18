use darwin_art_host::{ExecutionLifetime, HostOutcome, OwnedFrame, RunOptions, run};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt::Write as _;
use std::mem::size_of;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;
const FRAME_PIXEL_COUNT: usize = 230_400;
const FRAME_FNV1A64: u64 = 0x44d1_22c2_96a3_e065;
const SHUTDOWN_ALREADY_COMPLETED: i32 = 69;

const EXPECTED_COLORS: [(u32, usize); 6] = [
    (0xff10_2a20, 2_988),
    (0xff11_1827, 52_864),
    (0xff25_63eb, 12_672),
    (0xff3d_dc84, 38_372),
    (0xffe2_e8f0, 54_656),
    (0xfff8_fafc, 68_848),
];

const REQUIRED_REAL_SYMBOLS: [&str; 3] = [
    "register_android_graphics_Canvas",
    "register_android_graphics_Paint",
    "register_android_view_RenderNode",
];

const FORBIDDEN_FAKE_SYMBOLS: [&str; 4] = [
    "__ZN12_GLOBAL__N_19PaintInitEv",
    "__ZN12_GLOBAL__N_113PaintSetColorExi",
    "__ZN12_GLOBAL__N_116RenderNodeCreateE",
    "__ZN12_GLOBAL__N_131RenderNodeSetLeftTopRightBottomE",
];

fn main() {
    if let Err(error) = main_result() {
        eprintln!("real-graphics-acceptance: {error}");
        std::process::exit(1);
    }
}

fn main_result() -> Result<(), Box<dyn Error>> {
    #[cfg(not(target_os = "macos"))]
    return Err("real graphics acceptance requires macOS".into());

    #[cfg(target_os = "macos")]
    {
        let options = parse_options()?;
        audit_backend_symbols(&options.library)?;

        // run() owns the callback mailbox, performs the hidden NSWindow upload,
        // destroys the surface, and shuts ART down before returning.
        let outcome = run(&options)?;
        audit_process(&outcome)?;
        let frame = outcome
            .last_frame
            .as_ref()
            .ok_or("real backend returned without a frame callback")?;
        let metrics = FrameMetrics::from_frame(frame)?;
        audit_frame(&metrics)?;

        // ABI v1 makes shutdown intentionally non-idempotent. Seeing 69 here
        // proves that the Rust host already performed its one required shutdown
        // after the callback and presentation paths completed.
        let duplicate_shutdown = shutdown_status(&options.library)?;
        if duplicate_shutdown != SHUTDOWN_ALREADY_COMPLETED {
            return Err(format!(
                "shutdown lifecycle mismatch: duplicate status={duplicate_shutdown}, expected={SHUTDOWN_ALREADY_COMPLETED}"
            )
            .into());
        }

        println!(
            "real-graphics-acceptance: backend=android-graphics frame={}x{} pixels={} colors={} opaque={} hash={:016x} shutdown=complete fake-backend=absent",
            metrics.width,
            metrics.height,
            metrics.pixel_count,
            metrics.colors.len(),
            metrics.opaque_pixels,
            metrics.fnv1a64,
        );
        Ok(())
    }
}

fn parse_options() -> Result<RunOptions, Box<dyn Error>> {
    let mut arguments = env::args_os();
    let program = arguments
        .next()
        .unwrap_or_else(|| "real-graphics-acceptance".into());
    let values = arguments.collect::<Vec<_>>();
    if values.len() != 6 {
        return Err(format!(
            "usage: {} LIBDARWIN_ART CORE_OJ_JAR CORE_LIBART_JAR FRAMEWORK_JAR CORE_ICU4J_JAR APP_DEX",
            PathBuf::from(program).display()
        )
        .into());
    }
    Ok(RunOptions {
        library: PathBuf::from(&values[0]),
        core_oj_jar: PathBuf::from(&values[1]),
        core_libart_jar: PathBuf::from(&values[2]),
        framework_jar: PathBuf::from(&values[3]),
        core_icu4j_jar: PathBuf::from(&values[4]),
        app_dex: PathBuf::from(&values[5]),
        heap_initial_bytes: 64 * 1024 * 1024,
        heap_maximum_bytes: 64 * 1024 * 1024,
        visible_seconds: 0.0,
        execution_lifetime: ExecutionLifetime::ReusableSession,
    })
}

fn audit_process(outcome: &HostOutcome) -> Result<(), Box<dyn Error>> {
    let result = outcome.process;
    if result.hello_answer != 42
        || result.native_round_trip != 42
        || result.arraycopy_result != 42
        || result.activity_probe_result != 42
        || result.lifecycle_result != 43
        || result.frame_width != FRAME_WIDTH
        || result.frame_height != FRAME_HEIGHT
    {
        return Err(format!("unexpected runtime result: {result:?}").into());
    }
    if outcome.frames_presented != 1 {
        return Err(format!(
            "Rust host presentation count={}, expected=1",
            outcome.frames_presented
        )
        .into());
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct FrameMetrics {
    width: u32,
    height: u32,
    pixel_count: usize,
    opaque_pixels: usize,
    colors: BTreeMap<u32, usize>,
    fnv1a64: u64,
}

impl FrameMetrics {
    fn from_frame(frame: &OwnedFrame) -> Result<Self, Box<dyn Error>> {
        let expected_len = (frame.width as usize)
            .checked_mul(frame.height as usize)
            .ok_or("frame dimensions overflow usize")?;
        if frame.argb_pixels.len() != expected_len {
            return Err(format!(
                "packed callback length={}, dimensions require={expected_len}",
                frame.argb_pixels.len()
            )
            .into());
        }

        let mut colors = BTreeMap::new();
        let mut opaque_pixels = 0;
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for &pixel in &frame.argb_pixels {
            *colors.entry(pixel).or_insert(0) += 1;
            opaque_pixels += usize::from(pixel >> 24 == 0xff);
            // Canonical little-endian bytes make the digest independent of the
            // host representation while matching Android ARM64 pixel storage.
            for byte in pixel.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        Ok(Self {
            width: frame.width,
            height: frame.height,
            pixel_count: frame.argb_pixels.len(),
            opaque_pixels,
            colors,
            fnv1a64: hash,
        })
    }
}

fn audit_frame(metrics: &FrameMetrics) -> Result<(), Box<dyn Error>> {
    let expected_colors = EXPECTED_COLORS.into_iter().collect::<BTreeMap<_, _>>();
    if metrics.width != FRAME_WIDTH
        || metrics.height != FRAME_HEIGHT
        || metrics.pixel_count != FRAME_PIXEL_COUNT
        || metrics.opaque_pixels != FRAME_PIXEL_COUNT
        || metrics.colors != expected_colors
        || metrics.fnv1a64 != FRAME_FNV1A64
    {
        let mut actual_colors = String::new();
        for (color, count) in &metrics.colors {
            let _ = write!(&mut actual_colors, " {color:08x}={count}");
        }
        return Err(format!(
            "real frame mismatch: dimensions={}x{} pixels={} opaque={} hash={:016x} colors:{}",
            metrics.width,
            metrics.height,
            metrics.pixel_count,
            metrics.opaque_pixels,
            metrics.fnv1a64,
            actual_colors
        )
        .into());
    }
    Ok(())
}

fn audit_backend_symbols(library: &Path) -> Result<(), Box<dyn Error>> {
    let nm = Command::new("/usr/bin/nm")
        .args(["-aU"])
        .arg(library)
        .output()?;
    if !nm.status.success() {
        return Err(format!(
            "nm failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&nm.stderr).trim()
        )
        .into());
    }
    // Keep this on raw Mach-O names. Demangling the full ART image is both
    // unnecessary and expensive; upstream registrar names remain visible in
    // their mangled spellings, while these four fake names are exact outputs
    // of the locked Apple clang toolchain.
    let symbols = String::from_utf8(nm.stdout)?;
    for required in REQUIRED_REAL_SYMBOLS {
        if !symbols.contains(required) {
            return Err(format!("real graphics symbol missing: {required}").into());
        }
    }
    for forbidden in FORBIDDEN_FAKE_SYMBOLS {
        if symbols.contains(forbidden) {
            return Err(format!("fake graphics backend symbol is linked: {forbidden}").into());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn shutdown_status(library: &Path) -> Result<i32, Box<dyn Error>> {
    type ShutdownFn = unsafe extern "C" fn() -> i32;
    let path = CString::new(library.as_os_str().as_bytes())?;
    // SAFETY: The path is NUL-terminated and these are Darwin loader flags.
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
    if handle.is_null() {
        return Err(loader_error().into());
    }
    // SAFETY: The exported symbol's signature is fixed by darwin_art.h ABI v1.
    unsafe { dlerror() };
    let address = unsafe { dlsym(handle, c"darwin_art_shutdown_process".as_ptr()) };
    let symbol_error = unsafe { dlerror() };
    if !symbol_error.is_null() || address.is_null() {
        let message = if symbol_error.is_null() {
            "darwin_art_shutdown_process resolved to null".to_owned()
        } else {
            // SAFETY: A non-null dlerror result is a NUL-terminated string.
            unsafe { CStr::from_ptr(symbol_error) }
                .to_string_lossy()
                .into_owned()
        };
        // SAFETY: handle was returned by dlopen in this function.
        unsafe { dlclose(handle) };
        return Err(message.into());
    }
    if size_of::<ShutdownFn>() != size_of::<*mut c_void>() {
        // SAFETY: handle was returned by dlopen in this function.
        unsafe { dlclose(handle) };
        return Err("shutdown function pointer has an unexpected size".into());
    }
    // SAFETY: address is the ABI v1 shutdown function and pointer sizes match.
    let shutdown = unsafe { std::ptr::read((&address as *const *mut c_void).cast::<ShutdownFn>()) };
    let status = unsafe { shutdown() };
    // run() deliberately retains its original process-scoped image reference;
    // releasing only this audit reference cannot unload the dylib.
    let close_status = unsafe { dlclose(handle) };
    if close_status != 0 {
        return Err(loader_error().into());
    }
    Ok(status)
}

#[cfg(target_os = "macos")]
fn loader_error() -> String {
    // SAFETY: dlerror returns null or a process-owned NUL-terminated string.
    let error = unsafe { dlerror() };
    if error.is_null() {
        "unknown dynamic-loader error".to_owned()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "macos")]
const RTLD_LOCAL: i32 = 0x4;
#[cfg(target_os = "macos")]
const RTLD_NOW: i32 = 0x2;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const c_char;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_frame() -> OwnedFrame {
        let mut pixels = vec![0_u32; FRAME_PIXEL_COUNT];
        let mut fill = |left: usize, top: usize, width: usize, height: usize, color: u32| {
            for y in top..top + height {
                pixels[y * FRAME_WIDTH as usize + left..y * FRAME_WIDTH as usize + left + width]
                    .fill(color);
            }
        };
        fill(0, 0, 640, 360, 0xff11_1827);
        fill(28, 28, 584, 304, 0xfff8_fafc);
        fill(28, 28, 584, 70, 0xff3d_dc84);
        fill(76, 144, 488, 112, 0xffe2_e8f0);
        fill(188, 278, 264, 48, 0xff25_63eb);
        for (left, top, width, height) in [
            (76, 76, 12, 30),
            (88, 64, 12, 12),
            (100, 76, 12, 30),
            (83, 85, 24, 9),
            (128, 64, 12, 42),
            (140, 64, 24, 9),
            (152, 73, 12, 15),
            (140, 85, 18, 9),
            (152, 94, 12, 12),
            (180, 64, 48, 9),
            (198, 73, 12, 33),
        ] {
            fill(left, top, width, height, 0xff10_2a20);
        }
        OwnedFrame {
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            argb_pixels: pixels,
        }
    }

    #[test]
    fn exact_probe_frame_contract_matches_constants() {
        let metrics = FrameMetrics::from_frame(&expected_frame()).unwrap();
        audit_frame(&metrics).unwrap();
    }

    #[test]
    fn one_changed_pixel_fails_exact_frame_contract() {
        let mut frame = expected_frame();
        frame.argb_pixels[FRAME_WIDTH as usize + 1] ^= 1;
        let metrics = FrameMetrics::from_frame(&frame).unwrap();
        assert!(audit_frame(&metrics).is_err());
    }
}
