use darwin_art_host::{RunOptions, run, run_service_child};
use std::env;
use std::error::Error;
use std::ffi::{CStr, CString, OsString};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

fn main() {
    if let Err(error) = main_result() {
        eprintln!("darwin-art-host: {error}");
        std::process::exit(1);
    }
}

fn main_result() -> Result<(), Box<dyn Error>> {
    // SAFETY: this is the first startup action, before runtime/AppKit threads
    // or other environment consumers are created by this executable.
    unsafe {
        darwin_art_profile::wait_for_process_registration()?;
    }
    // Android blocks its runtime-control signals before creating any process
    // threads, then ART's Signal Catcher consumes them with sigwait(). Do the
    // same at the Mach-O process boundary so AppKit/frame-clock workers cannot
    // inherit an unblocked SIGQUIT and terminate the process first.
    let mut runtime_signals = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    unsafe {
        libc::sigemptyset(&mut runtime_signals);
        libc::sigaddset(&mut runtime_signals, libc::SIGPIPE);
        libc::sigaddset(&mut runtime_signals, libc::SIGQUIT);
        libc::sigaddset(&mut runtime_signals, libc::SIGUSR1);
    }
    let signal_status =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &runtime_signals, std::ptr::null_mut()) };
    if signal_status != 0 {
        return Err(std::io::Error::from_raw_os_error(signal_status).into());
    }
    // darwin-artctl deliberately carries the daemon lease through exec. Make
    // it close-on-exec again immediately so a service child receives its own
    // PID lease instead of extending its parent's registration accidentally.
    if let Ok(value) = env::var("DARWIN_ART_PROFILE_LEASE_FD") {
        let descriptor = value.parse::<i32>()?;
        let flags = unsafe { fcntl(descriptor, F_GETFD) };
        if flags < 0 || unsafe { fcntl(descriptor, F_SETFD, flags | FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    if let Ok(delay) = env::var("DARWIN_ART_DEBUG_ATTACH_DELAY_MS") {
        std::thread::sleep(std::time::Duration::from_millis(delay.parse()?));
    }
    let mut arguments = env::args_os();
    let program = arguments.next().unwrap_or_else(|| "darwin-art-host".into());
    let mut values = arguments.collect::<Vec<_>>();
    if values
        .first()
        .is_some_and(|value| value == "--start-system-service")
    {
        let started = darwin_art_host::system_service_start::start(&values[1..], env::vars_os())?;
        println!(
            "pid={}\nbinder={}\ncompositor={}",
            started.pid,
            started.endpoints.binder.display(),
            started.endpoints.compositor.display()
        );
        return Ok(());
    }
    if values
        .first()
        .is_some_and(|value| value == "--prepare-external-storage")
    {
        if values.len() != 4 {
            return Err(
                "--prepare-external-storage requires storage root, app-data directory and package"
                    .into(),
            );
        }
        let package = values[3].to_str().ok_or("package must be UTF-8")?;
        let files = darwin_art_host::external_storage::prepare(
            std::path::Path::new(&values[1]),
            std::path::Path::new(&values[2]),
            package,
        )?;
        println!("{}", files.display());
        return Ok(());
    }
    if values
        .first()
        .is_some_and(|value| value == "--prepare-system-image")
    {
        if values.len() != 3 {
            return Err("--prepare-system-image requires archive and shared store".into());
        }
        let installed = darwin_art_host::system_image::prepare(
            std::path::Path::new(&values[1]),
            std::path::Path::new(&values[2]),
        )?;
        println!("{}", installed.display());
        return Ok(());
    }
    if values.first().is_some_and(|value| value == "--dex2oat") {
        return run_embedded_art_tool(&values, "--dex2oat", c"darwin_art_run_dex2oat", "dex2oat");
    }
    if values.first().is_some_and(|value| value == "--profman") {
        return run_embedded_art_tool(&values, "--profman", c"darwin_art_run_profman", "profman");
    }
    if values
        .first()
        .is_some_and(|value| value == "--service-child")
    {
        if values.len() != 2 {
            return Err("--service-child requires exactly one control fd".into());
        }
        let control_fd = values[1].to_string_lossy().parse::<i32>()?;
        return run_service_child(control_fd).map_err(Into::into);
    }
    let frame_output = if values.first().is_some_and(|value| value == "--frame-ppm") {
        if values.len() < 2 {
            return Err("--frame-ppm requires a path".into());
        }
        let output = PathBuf::from(&values[1]);
        values.drain(..2);
        Some(output)
    } else {
        None
    };
    let visible_seconds = if values
        .first()
        .is_some_and(|value| value == "--window-seconds")
    {
        if values.len() < 2 {
            return Err("--window-seconds requires a value".into());
        }
        let seconds = values[1].to_string_lossy().parse::<f64>()?;
        values.drain(..2);
        seconds
    } else {
        0.0
    };
    if values.len() != 6 {
        return Err(format!(
            "usage: {} [--frame-ppm PATH] [--window-seconds SECONDS] LIBDARWIN_ART CORE_OJ_JAR \
             CORE_LIBART_JAR FRAMEWORK_JAR CORE_ICU4J_JAR APP_DEX",
            PathBuf::from(program).display()
        )
        .into());
    }
    eprintln!(
        "ART host identity pid={} exe={} app_dex={}",
        std::process::id(),
        std::env::current_exe()
            .map_or_else(|_| "<unknown>".into(), |path| path.display().to_string()),
        values[5].to_string_lossy()
    );
    let options = RunOptions {
        library: PathBuf::from(&values[0]),
        core_oj_jar: PathBuf::from(&values[1]),
        core_libart_jar: PathBuf::from(&values[2]),
        framework_jar: PathBuf::from(&values[3]),
        core_icu4j_jar: PathBuf::from(&values[4]),
        app_dex: PathBuf::from(&values[5]),
        heap_initial_bytes: 64 * 1024 * 1024,
        heap_maximum_bytes: 256 * 1024 * 1024,
        visible_seconds,
        // Android application VMs are process-scoped zygote children. The OS
        // terminates that process instead of calling DestroyJavaVM, which is
        // unsafe for Chromium's still-live native task runners. The pinned
        // ART corpus uses the same process lifetime so tests exercise app
        // execution rather than an Android-inaccurate VM teardown sequence.
        // Android application and dalvikvm test processes are one-shot zygote
        // children. NativeLoader DSOs are explicitly unloaded immediately
        // before this process-style exit so JNI_OnUnload still observes the
        // Android lifecycle without racing an app-owned DestroyJavaVM.
        // Production APK launches follow the Android process boundary and
        // terminate with _exit.  The explicit opt-in is test-only: it keeps
        // the real APK/ViewRoot/HWUI path but exercises the embeddable
        // RuntimeShutdownGuard so worker JNI detach can be observed.
        terminate_android_process: env::var_os("DARWIN_ART_APK_APP_PACKAGE").is_some()
            && env::var_os("DARWIN_ART_FORCE_EMBEDDED_SHUTDOWN").is_none(),
    };
    let outcome = run(&options)?;
    if let Some(path) = frame_output {
        let frame = outcome
            .last_frame
            .as_ref()
            .ok_or("runtime did not produce a frame")?;
        let mut output = BufWriter::new(File::create(path)?);
        write!(output, "P6\n{} {}\n255\n", frame.width, frame.height)?;
        for pixel in &frame.argb_pixels {
            output.write_all(&[
                ((pixel >> 16) & 0xff) as u8,
                ((pixel >> 8) & 0xff) as u8,
                (pixel & 0xff) as u8,
            ])?;
        }
        output.flush()?;
    }
    // Upstream ART run-test owns stdout/stderr byte-for-byte. Native test
    // modules write to the inherited process stream, so keep the host's
    // human-oriented acceptance summary out of that application channel.
    if env::var_os("DARWIN_ART_UPSTREAM_MAIN").is_some() {
        return Ok(());
    }
    println!("ART Darwin Runtime::Create: ok");
    println!("ART Darwin app ClassLoader: PathClassLoader");
    println!(
        "ART Darwin DEX interpreter: Hello.answer()={}",
        outcome.process.hello_answer
    );
    println!(
        "ART Darwin JNI: hostPageSize()={} nativeRoundTrip()={}",
        unsafe { getpagesize() },
        outcome.process.native_round_trip
    );
    println!(
        "ART runtime native: System.arraycopy()={}",
        outcome.process.arraycopy_result
    );
    if let (Ok(package), Ok(activity)) = (
        env::var("DARWIN_ART_APK_APP_PACKAGE"),
        env::var("DARWIN_ART_APK_APP_ACTIVITY"),
    ) {
        let render_scale = match env::var("DARWIN_ART_WINDOW_SCALE").as_deref() {
            Ok("2") => 2,
            Ok("1") | Err(_) => 1,
            Ok(_) => return Err("DARWIN_ART_WINDOW_SCALE must be 1 or 2".into()),
        };
        let expected_width = (360 * render_scale) as u32;
        let expected_height = (640 * render_scale) as u32;
        let is_expected_geometry = |width: u32, height: u32| {
            (width == expected_width && height == expected_height)
                || (width == expected_height && height == expected_width)
        };
        let widget_expected = env::var("DARWIN_ART_APK_APP_EXPECT_WIDGETS").as_deref() == Ok("1");
        let native_loaded = env::var("DARWIN_ART_APK_APP_NATIVE_PATH")
            .map(|path| !path.is_empty())
            .unwrap_or(false);
        let widget = if widget_expected {
            " widgets=framework-owned"
        } else {
            ""
        };
        if let Some(frame) = outcome.last_frame.as_ref() {
            let all_opaque = frame
                .argb_pixels
                .iter()
                .all(|pixel| pixel & 0xff00_0000 == 0xff00_0000);
            if !is_expected_geometry(frame.width, frame.height)
                || outcome.frames_presented == 0
                || !all_opaque
            {
                return Err("APK Activity frame did not match its opaque frame contract".into());
            }
            let pixel_count = frame.width as u64 * frame.height as u64;
            println!(
                "ART Android APK: package={package} launcher={activity} classes.dex=APK native={} pixels={pixel_count}/opaque{widget}",
                if native_loaded { 1 } else { 0 }
            );
        } else {
            // The production graphics path renders directly into the
            // CAMetalLayer drawable. It intentionally has no CPU frame
            // callback or readback, so GPU acceptance validates presentation
            // count and the dimensions recorded by the Android frame probe.
            if outcome.frames_presented == 0
                || !is_expected_geometry(outcome.process.frame_width, outcome.process.frame_height)
            {
                return Err(
                    "APK Activity GPU presentation did not match its frame contract".into(),
                );
            }
            println!(
                "ART Android APK: package={package} launcher={activity} classes.dex=APK native={} gpu=direct drawable={}x{}{widget}",
                if native_loaded { 1 } else { 0 },
                outcome.process.frame_width,
                outcome.process.frame_height,
            );
        }
    } else {
        println!(
            "ART Android framework: ProbeActivity().probeValue()={}",
            outcome.process.activity_probe_result
        );
    }
    println!("ART Android window: Activity.attach()=PhoneWindow+DecorView");
    println!(
        "ART Android view: Activity.setContentView()->DecorView.draw(Canvas)={}x{}",
        outcome.process.frame_width, outcome.process.frame_height
    );
    println!(
        "ART Android lifecycle: Activity.onCreate()={}",
        outcome.process.lifecycle_result
    );
    println!("ART Darwin launcher: main(String[])=ok");
    Ok(())
}

fn run_embedded_art_tool(
    values: &[OsString],
    mode: &str,
    symbol_name: &CStr,
    argv0: &str,
) -> Result<(), Box<dyn Error>> {
    if values.len() < 3 {
        return Err(format!("{mode} requires LIBDARWIN_ART and tool arguments").into());
    }
    let library_path = CString::new(values[1].as_bytes())?;
    // Keep the runtime provider visible to the sibling RTLD_LOCAL OpenJDK
    // owner.  This mirrors the production Darwin engine's libart load and
    // preserves the Android process-wide provider ABI without copying
    // private runtime symbols into each JNI module.
    let handle = unsafe { libc::dlopen(library_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
    if handle.is_null() {
        let message = unsafe { libc::dlerror() };
        return Err(if message.is_null() {
            format!("unable to load {argv0} runtime").into()
        } else {
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
                .into()
        });
    }
    let symbol = unsafe { libc::dlsym(handle, symbol_name.as_ptr()) };
    if symbol.is_null() {
        unsafe { libc::dlclose(handle) };
        return Err(format!("{argv0} runtime entry is missing").into());
    }
    let mut argv = Vec::with_capacity(values.len() - 1);
    argv.push(CString::new(argv0)?);
    for value in &values[2..] {
        argv.push(CString::new(value.as_bytes())?);
    }
    let mut argv_ptrs = argv
        .iter_mut()
        .map(|value| value.as_ptr().cast_mut())
        .collect::<Vec<_>>();
    let argc = argv_ptrs.len() as i32;
    // ART's InitLogging() intentionally reconstructs the command line by
    // walking argv until a null pointer; it does not receive argc. Preserve
    // the process-entry C ABI even though this tool is entered through dlopen.
    argv_ptrs.push(std::ptr::null_mut());
    type ToolMain = unsafe extern "C" fn(i32, *mut *mut libc::c_char) -> i32;
    let entry: ToolMain = unsafe { std::mem::transmute(symbol) };
    let status = unsafe { entry(argc, argv_ptrs.as_mut_ptr()) };
    unsafe { libc::dlclose(handle) };
    if status != 0 {
        return Err(format!("{argv0} failed with status {status}").into());
    }
    Ok(())
}

unsafe extern "C" {
    fn getpagesize() -> i32;
    fn fcntl(descriptor: i32, command: i32, ...) -> i32;
}

const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;
