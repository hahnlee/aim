//! `cargo aim bench`: boots a fresh data directory the way `cargo aim boot`
//! does (aim-display, guest-init `--exclude bootanim`) and measures boot,
//! process creation, binder, app starts, Chrome and memory
//! (docs/perf-baseline.md has the method). Each run is one boot; the
//! table of medians goes to stdout and the runs to
//! `target/aim/bench/<timestamp>.json`. `--compare A.json B.json` prints
//! the change of every median.
//!
//! Guest commands run as root through another linux-run on the boot's path
//! map and binder host, as docs/boot-status.md describes.

mod parse;
mod report;

use crate::boot;
use crate::graph::Ctx;
use report::Run;
use serde_json::{Map, Value, json};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CHROME: &str = "org.chromium.chrome";
const CHROME_MAIN: &str = "org.chromium.chrome/com.google.android.apps.chrome.Main";
const CALCULATOR: &str = "com.android.calculator2";
const CALCULATOR_MAIN: &str = "com.android.calculator2/.Calculator";
const SETTINGS: &str = "com.android.settings";
const SETTINGS_MAIN: &str = "com.android.settings/.homepage.SettingsHomepageActivity";
const PAGES: &[(&str, &str)] = &[
    ("example", "https://example.com/"),
    (
        "wikipedia",
        "https://en.wikipedia.org/wiki/Android_(operating_system)",
    ),
];
/// Launches per app and state, page loads per page.
const REPEAT: usize = 3;
/// The guest is settled after boot below this CPU use (host cores), at the
/// latest this long after boot completion.
const SETTLED_CORES: f64 = 1.5;
const SETTLE_MAX: Duration = Duration::from_secs(180);
/// The screen is settled once it has not changed for this long.
const SETTLE: Duration = Duration::from_secs(3);

pub struct Options {
    pub runs: usize,
    pub keep: bool,
}

pub fn run(ctx: &Ctx, options: &Options) -> Result<ExitCode, String> {
    let dir = aim_paths::out().join("bench");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let fixtures = Fixtures::build(&dir.join("bin"))?;
    let started = unix_now();
    let mut runs = Vec::new();
    let mut failed = 0;
    for n in 0..options.runs {
        eprintln!("bench: run {} of {}", n + 1, options.runs);
        let mut run = Run::new();
        let result = one_run(ctx, &dir, n, &fixtures, options.keep, &mut run);
        if let Err(error) = &result {
            eprintln!("bench: run {}: {error}", n + 1);
            failed += 1;
        }
        runs.push(run);
        if let Err(error) = result
            && error.contains("SIGKILL")
        {
            // The host security agent (#232) kills every later linux-run too.
            break;
        }
    }
    let mut meta = Map::new();
    meta.insert("version".into(), json!(1));
    meta.insert("date".into(), json!(started));
    meta.insert("failed_runs".into(), json!(failed));
    meta.insert("sha".into(), json!(git(&["rev-parse", "HEAD"])));
    meta.insert(
        "dirty".into(),
        json!(!git(&["status", "--porcelain", "--untracked-files=no"]).is_empty()),
    );
    meta.insert("machine".into(), machine());
    meta.insert(
        "method".into(),
        json!({
            "boot": "guest-init --run --exclude bootanim --gpu ANGLE --display aim-display (1080x1920), fresh data directory per run",
            "repeat": REPEAT,
            "settle_s": SETTLE.as_secs_f64(),
        }),
    );
    let report = report::to_json(meta, &runs);
    let path = dir.join(format!("{}.json", report::timestamp(started)));
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    print!("{}", report::table(&runs));
    println!("\n{} run(s); written to {}", runs.len(), path.display());
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn compare(a: &str, b: &str) -> Result<ExitCode, String> {
    let load = |path: &str| -> Result<_, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let value: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
        let sha = value["sha"].as_str().unwrap_or("?").to_string();
        Ok((
            sha,
            report::medians(&value).map_err(|e| format!("{path}: {e}"))?,
        ))
    };
    let ((sha_a, a), (sha_b, b)) = (load(a)?, load(b)?);
    println!("A: {sha_a}\nB: {sha_b}\n");
    print!("{}", report::compare(&a, &b));
    Ok(ExitCode::SUCCESS)
}

/// The NDK programs run in the guest.
struct Fixtures {
    fork: PathBuf,
    binder: PathBuf,
}

impl Fixtures {
    fn build(dir: &Path) -> Result<Fixtures, String> {
        let clang = aim_paths::ndk_clang(35).ok_or("the pinned NDK's clang is missing")?;
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let root = aim_paths::root();
        let compile = |source: PathBuf, out: &Path, libs: &[&str]| -> Result<(), String> {
            let status = Command::new(&clang)
                .args(["-O2", "-Wno-deprecated-declarations", "-o"])
                .arg(out)
                .arg(&source)
                .args(libs)
                .status()
                .map_err(|e| format!("{}: {e}", clang.display()))?;
            if !status.success() {
                return Err(format!("compiling {}: {status}", source.display()));
            }
            Ok(())
        };
        let fixtures = Fixtures {
            fork: dir.join("aim_fork_bench"),
            binder: dir.join("aim_binder_ping"),
        };
        compile(
            root.join("crates/aim-build/src/bench/fork_bench.c"),
            &fixtures.fork,
            &[],
        )?;
        compile(
            root.join("crates/aim-linux-abi/tests/fixtures/binder_ping.c"),
            &fixtures.binder,
            &["-lbinder_ndk"],
        )?;
        Ok(fixtures)
    }
}

/// aim-display and guest-init of one run; dropping it stops both.
struct Session {
    display: Child,
    init: Child,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: signals children we started. guest-init stops its services
        // on SIGTERM.
        unsafe { libc::kill(self.init.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(60);
        while matches!(self.init.try_wait(), Ok(None)) {
            if Instant::now() > deadline {
                let _ = self.init.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.init.wait();
        // SAFETY: as above.
        unsafe { libc::kill(self.display.id() as i32, libc::SIGTERM) };
        let _ = self.display.wait();
    }
}

impl Session {
    fn check(&mut self) -> Result<(), String> {
        match self.init.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => Err(format!("guest-init exited: {status}")),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Runs guest programs through linux-run on one boot's path map and binder
/// host.
struct Guest {
    linux_run: PathBuf,
    path_map: PathBuf,
    binder: String,
    /// The host directory behind the guest's `/data`.
    data: PathBuf,
}

impl Guest {
    /// The program's stdout; an error on a timeout or when linux-run was
    /// killed.
    fn run(&self, argv: &[&str], timeout: Duration) -> Result<String, String> {
        let mut child = Command::new(&self.linux_run)
            .arg("--root")
            .arg(aim_paths::derived_image())
            .arg("--path-map")
            .arg(&self.path_map)
            .args(["--binder", &self.binder])
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("linux-run: {e}"))?;
        let mut stdout = child.stdout.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            out
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() > deadline {
                // SAFETY: the process group of the child we started.
                unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
                let _ = child.wait();
                return Err(format!("{}: timed out after {timeout:?}", argv.join(" ")));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
        if status.signal() == Some(libc::SIGKILL) {
            return Err(format!(
                "{}: linux-run got SIGKILL (the host security agent? #232)",
                argv.join(" ")
            ));
        }
        Ok(out)
    }

    fn sh(&self, script: &str) -> Result<String, String> {
        self.run(&["/system/bin/sh", "-c", script], Duration::from_secs(60))
    }

    fn am(&self, args: &[&str]) -> Result<String, String> {
        let mut argv = vec!["/system/bin/am"];
        argv.extend(args);
        self.run(&argv, Duration::from_secs(90))
    }

    /// An `am start -W` of `component`: cold after a force-stop, else warm
    /// (brought back after HOME). A launch in another state than asked for
    /// does not count.
    fn launch(&self, component: &str, cold: bool) -> Result<Option<f64>, String> {
        let out = if cold {
            self.am(&["start", "-W", "-S", "-n", component])
        } else {
            self.run(
                &["/system/bin/input", "keyevent", "KEYCODE_HOME"],
                Duration::from_secs(30),
            )?;
            std::thread::sleep(Duration::from_secs(1));
            self.am(&["start", "-W", "-n", component])
        };
        // A launch that never reports its first frame does not count; the
        // next one still runs.
        let out = match out {
            Err(e) if e.contains("timed out") => {
                eprintln!("bench: {e}");
                return Ok(None);
            }
            out => out?,
        };
        let Some(launch) = parse::am_start(&out) else {
            eprintln!("bench: {component}: {}", out.trim());
            return Ok(None);
        };
        let expected = launch.total_ms > 0.0
            && if cold {
                launch.state == "COLD"
            } else {
                matches!(launch.state.as_str(), "WARM" | "HOT")
            };
        if !expected {
            eprintln!("bench: {component}: launch state {}", launch.state);
        }
        Ok(expected.then_some(launch.total_ms))
    }

    /// Swipes through PACKAGE's top window (five down, five up) and returns
    /// its HWUI frame statistics over them.
    fn scroll(&self, package: &str) -> Result<Option<parse::Frames>, String> {
        let dumpsys = |extra: &[&str]| {
            let mut argv = vec!["/system/bin/dumpsys", "gfxinfo", package];
            argv.extend(extra);
            self.run(&argv, Duration::from_secs(30))
        };
        dumpsys(&["reset"])?;
        for i in 0..10 {
            let (from, to) = if i < 5 {
                ("1500", "500")
            } else {
                ("500", "1500")
            };
            self.run(
                &["/system/bin/input", "swipe", "540", from, "540", to, "300"],
                Duration::from_secs(30),
            )?;
            std::thread::sleep(Duration::from_millis(400));
        }
        std::thread::sleep(Duration::from_secs(1));
        Ok(parse::gfxinfo(&dumpsys(&[])?))
    }

    fn install(&self, apk: &Path, name: &str) -> Result<Duration, String> {
        let tmp = self.data.join("local/tmp");
        let copy = tmp.join(name);
        // A copy (an APFS clone) in the guest's /data/local/tmp; the
        // original stays untouched.
        fs::copy(apk, &copy).map_err(|e| format!("{}: {e}", apk.display()))?;
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o644))
            .map_err(|e| format!("{}: {e}", copy.display()))?;
        let guest_path = format!("/data/local/tmp/{name}");
        let start = Instant::now();
        let out = self.run(
            &["/system/bin/pm", "install", "-r", "-g", &guest_path],
            Duration::from_secs(600),
        )?;
        let took = start.elapsed();
        let _ = fs::remove_file(&copy);
        if !out.contains("Success") {
            return Err(format!("pm install {name}: {}", out.trim()));
        }
        Ok(took)
    }
}

/// The last presented frame (aim-display's capture: SIGUSR1 writes it on
/// the next vsync): its hash and whether its middle is blank.
fn screen(display: &Child, capture: &Path) -> Option<(u64, bool)> {
    let _ = fs::remove_file(capture);
    // SAFETY: signals the child we started.
    unsafe { libc::kill(display.id() as i32, libc::SIGUSR1) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        let Ok(bytes) = fs::read(capture) else {
            continue;
        };
        // Complete once it holds the size its header gives.
        if bytes.len() >= 6
            && bytes.len() as u32 == u32::from_le_bytes(bytes[2..6].try_into().ok()?)
        {
            let mut h = DefaultHasher::new();
            bytes.hash(&mut h);
            return Some((h.finish(), parse::bmp_middle_blank(&bytes)));
        }
    }
    None
}

/// Seconds from `am start` of URL in Chrome to the last change of the
/// screen before it stays unchanged for SETTLE ("visually complete").
fn page_load(
    guest: &Guest,
    session: &Session,
    capture: &Path,
    url: &str,
) -> Result<Option<f64>, String> {
    let mut last = screen(&session.display, capture).map(|s| s.0);
    let mut blank = true;
    let since = unix_now_f64();
    let start = Instant::now();
    guest.am(&[
        "start",
        "-a",
        "android.intent.action.VIEW",
        "-d",
        url,
        "-n",
        CHROME_MAIN,
    ])?;
    let mut changed = None;
    let mut still = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        let now = screen(&session.display, capture);
        if let Some((hash, is_blank)) = now
            && Some(hash) != last
        {
            last = Some(hash);
            blank = is_blank;
            changed = Some(start.elapsed());
            still = Instant::now();
        } else if changed.is_some() && still.elapsed() >= SETTLE {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    // A page whose renderer died drew nothing; the settled screen is
    // Chrome's own UI.
    let log = guest.run(
        &[
            "/system/bin/logcat",
            "-d",
            "-b",
            "main",
            "-v",
            "epoch",
            "-s",
            "chromium",
        ],
        Duration::from_secs(30),
    )?;
    let deaths = parse::child_deaths(&log, since);
    if deaths > 0 {
        eprintln!("bench: {url}: {deaths} Chrome child process(es) died during the load");
        return Ok(None);
    }
    if blank {
        eprintln!("bench: {url}: the page area stayed blank");
        return Ok(None);
    }
    Ok(changed.map(|d| d.as_secs_f64()))
}

fn processes(path_map: &Path) -> parse::Processes {
    let ps = Command::new("ps")
        .args(["-axww", "-o", "pid=,rss=,time=,command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    parse::guest_processes(&ps, &path_map.to_string_lossy())
}

fn memory(run: &mut Run, prefix: &str, path_map: &Path) {
    let p = processes(path_map);
    run.insert(format!("{prefix}.processes_count"), p.count as f64);
    run.insert(format!("{prefix}.rss_mb"), p.rss_kib as f64 / 1024.0);
}

/// The guest's CPU use (host cores) over `window`: the CPU time the
/// processes alive at its end gained.
fn cpu_cores(path_map: &Path, window: Duration) -> f64 {
    let before = processes(path_map).cpu_s;
    std::thread::sleep(window);
    let after = processes(path_map).cpu_s;
    (after - before).max(0.0) / window.as_secs_f64()
}

/// The first launch (the first after boot or install, cold) on its own,
/// then REPEAT cold and REPEAT warm launches.
fn record_launches(
    guest: &Guest,
    run: &mut Run,
    name: &str,
    component: &str,
) -> Result<(), String> {
    if let Some(ms) = guest.launch(component, true)? {
        run.insert(format!("{name}.first_start_ms"), ms);
    }
    std::thread::sleep(Duration::from_secs(2));
    for cold in [true, false] {
        let mut times = Vec::new();
        for _ in 0..REPEAT {
            if let Some(ms) = guest.launch(component, cold)? {
                times.push(ms);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        if let Some(s) = report::stat(&times) {
            let state = if cold { "cold" } else { "warm" };
            run.insert(format!("{name}.{state}_start_ms"), s.median);
        }
    }
    Ok(())
}

fn record_scroll(guest: &Guest, run: &mut Run, name: &str, package: &str) -> Result<(), String> {
    if let Some(f) = guest.scroll(package)? {
        run.insert(format!("{name}.scroll_frames_count"), f.total);
        run.insert(format!("{name}.scroll_janky_pct"), f.janky_pct);
        run.insert(format!("{name}.scroll_frame_p50_ms"), f.p50_ms);
        run.insert(format!("{name}.scroll_frame_p90_ms"), f.p90_ms);
        run.insert(format!("{name}.scroll_frame_p99_ms"), f.p99_ms);
    }
    Ok(())
}

fn one_run(
    ctx: &Ctx,
    dir: &Path,
    n: usize,
    fixtures: &Fixtures,
    keep: bool,
    run: &mut Run,
) -> Result<(), String> {
    let data = dir.join(format!("data-{n}"));
    if data.exists() {
        aim_android_image::assemble::force_remove(&data)
            .map_err(|e| format!("{}: {e}", data.display()))?;
    }
    fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let log =
        fs::File::create(dir.join(format!("guest-init-{n}.log"))).map_err(|e| e.to_string())?;
    let display = boot::start_display(ctx, dir)?;
    let epoch0 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let t0 = Instant::now();
    let init = boot::guest_init(ctx, &data, &dir.join("display"))
        .arg("--quiet")
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("guest-init: {e}"));
    let init = match init {
        Ok(init) => init,
        Err(e) => {
            let mut display = display;
            let _ = display.kill();
            let _ = display.wait();
            return Err(e);
        }
    };
    let mut session = Session { display, init };
    let guest = Guest {
        linux_run: ctx.workspace.host_bin("linux-run"),
        path_map: data.join("run/path-map"),
        binder: format!("dev.aim.guest-init.{}.binder", session.init.id()),
        data: data.join("data"),
    };
    let capture = dir.join("capture.bmp");
    let result = measure(&guest, &mut session, &capture, fixtures, epoch0, t0, run);
    drop(session);
    if !keep {
        let _ = aim_android_image::assemble::force_remove(&data);
    }
    result
}

fn measure(
    guest: &Guest,
    session: &mut Session,
    capture: &Path,
    fixtures: &Fixtures,
    epoch0: f64,
    t0: Instant,
    run: &mut Run,
) -> Result<(), String> {
    // Boot: sys.boot_completed polled every half second; the milestones
    // before it from the events log's wall-clock times.
    loop {
        session.check()?;
        if t0.elapsed() > Duration::from_secs(300) {
            return Err("no sys.boot_completed after 300 s".into());
        }
        if guest.path_map.exists()
            && guest
                .run(
                    &["/system/bin/getprop", "sys.boot_completed"],
                    Duration::from_secs(10),
                )?
                .trim()
                == "1"
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    run.insert("boot.completed_s".into(), t0.elapsed().as_secs_f64());
    let events = parse::events(&guest.run(
        &["/system/bin/logcat", "-d", "-b", "events", "-v", "epoch"],
        Duration::from_secs(30),
    )?);
    for (tag, name) in [
        ("boot_progress_start", "boot.zygote_s"),
        ("boot_progress_system_run", "boot.system_server_s"),
        ("boot_progress_pms_ready", "boot.pms_ready_s"),
        ("boot_progress_ams_ready", "boot.ams_ready_s"),
        ("boot_progress_enable_screen", "boot.enable_screen_s"),
    ] {
        if let Some(t) = events.get(tag) {
            run.insert(name.into(), t - epoch0);
        }
    }
    memory(run, "boot", &guest.path_map);
    // The first-boot apps (GMS, the launcher, dexopt) keep the guest busy
    // after boot completion; the rest measures a settled system: guest CPU
    // below SETTLED_CORES over a 5 s window, or SETTLE_MAX after boot.
    let completed = Instant::now();
    loop {
        session.check()?;
        let cores = cpu_cores(&guest.path_map, Duration::from_secs(5));
        if cores < SETTLED_CORES || completed.elapsed() > SETTLE_MAX {
            break;
        }
    }
    run.insert("boot.settled_s".into(), t0.elapsed().as_secs_f64());
    memory(run, "idle", &guest.path_map);
    run.insert(
        "idle.cpu_cores".into(),
        cpu_cores(&guest.path_map, Duration::from_secs(10)),
    );

    // Process creation and binder, timed in the guest.
    let tmp = guest.data.join("local/tmp");
    for (fixture, name) in [
        (&fixtures.fork, "aim_fork_bench"),
        (&fixtures.binder, "aim_binder_ping"),
    ] {
        fs::copy(fixture, tmp.join(name)).map_err(|e| format!("{}: {e}", fixture.display()))?;
    }
    let long = Duration::from_secs(120);
    for (argv, prefix, name) in [
        (
            &["/data/local/tmp/aim_fork_bench", "fork", "200"][..],
            "fork:",
            "process.fork",
        ),
        (
            &["/data/local/tmp/aim_fork_bench", "spawn", "50"][..],
            "spawn:",
            "process.fork_exec",
        ),
        (
            &["/data/local/tmp/aim_binder_ping", "ping", "5000"][..],
            "binder ping:",
            "binder.ping",
        ),
    ] {
        let out = guest.run(argv, long)?;
        match parse::latency(&out, prefix) {
            Some(v) => {
                for p in ["p50", "p90", "p99"] {
                    run.insert(format!("{name}_{p}_us"), v[p]);
                }
            }
            None => eprintln!("bench: {}: {}", argv.join(" "), out.trim()),
        }
    }
    session.check()?;

    // Settings, from the image.
    record_launches(guest, run, "settings", SETTINGS_MAIN)?;
    record_scroll(guest, run, "settings", SETTINGS)?;
    session.check()?;

    // A trivial app: Calculator.
    let apps = aim_paths::fetched().join("installed-apps");
    install_app(guest, run, &apps, CALCULATOR, "calculator")?;
    record_launches(guest, run, "calculator", CALCULATOR_MAIN)?;
    session.check()?;

    // Chrome, without its first-run screens (the command-line file is read
    // for the debug app).
    install_app(guest, run, &apps, CHROME, "chrome")?;
    guest.sh(
        "echo '_ --disable-fre --no-default-browser-check --no-first-run' \
         > /data/local/tmp/chrome-command-line && chmod 644 /data/local/tmp/chrome-command-line",
    )?;
    guest.am(&["set-debug-app", "--persistent", CHROME])?;
    record_launches(guest, run, "chrome", CHROME_MAIN)?;
    for (name, url) in PAGES {
        let mut times = Vec::new();
        for i in 0..REPEAT {
            // A distinct URL each time: a new navigation, not a tab switch.
            let sep = if url.contains('?') { '&' } else { '?' };
            if let Some(s) = page_load(guest, session, capture, &format!("{url}{sep}aimbench={i}"))?
            {
                times.push(s);
            }
            session.check()?;
        }
        if let Some(s) = report::stat(&times) {
            run.insert(format!("chrome.load_{name}_s"), s.median);
        }
    }
    // The last page loaded is the heavy one.
    record_scroll(guest, run, "chrome", CHROME)?;
    memory(run, "chrome", &guest.path_map);
    Ok(())
}

fn install_app(
    guest: &Guest,
    run: &mut Run,
    apps: &Path,
    package: &str,
    name: &str,
) -> Result<(), String> {
    let apk = installed_apk(apps, package)?;
    let took = guest.install(&apk, &format!("aim-bench-{name}.apk"))?;
    run.insert(format!("{name}.install_s"), took.as_secs_f64());
    Ok(())
}

/// `_build/installed-apps/<package>/<version>/<sha>/base.apk`.
fn installed_apk(apps: &Path, package: &str) -> Result<PathBuf, String> {
    let only = |dir: &Path| -> Result<PathBuf, String> {
        fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .ok_or(format!("{}: empty", dir.display()))
    };
    let apk = only(&only(&apps.join(package))?)?.join("base.apk");
    apk.exists()
        .then_some(apk.clone())
        .ok_or(format!("{}: missing", apk.display()))
}

fn unix_now_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn git(args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(aim_paths::root())
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn machine() -> Value {
    let sysctl = |name: &str| {
        Command::new("sysctl")
            .args(["-n", name])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let macos = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    json!({
        "model": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cpus": sysctl("hw.ncpu").parse::<u64>().ok(),
        "memory_gb": sysctl("hw.memsize").parse::<u64>().ok().map(|b| b / (1 << 30)),
        "macos": macos,
    })
}
