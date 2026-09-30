//! Audio on the derived image (ADR 0012 phase P5, docs/audio.md): guest-init
//! boots servicemanager, hwservicemanager, system_suspend, our audio HAL and
//! the original audioserver, all under linux-run.
//!
//! - audioserver loads the HAL through the original libaudiohal@aidl, takes
//!   its policy configuration from it and opens the primary output stream;
//! - `audio-hal-check` (hal/audio/check) plays and records through the HAL
//!   the way libaudiohal does; the HAL's log shows what CoreAudio's render
//!   callback received;
//! - an NDK program plays through the original libaaudio and AudioFlinger,
//!   once audioserver registers `media.audio_flinger` (it first waits for
//!   system_server's `activity` service).
//!
//! The HAL's own messages are in logd (services' stdio is /dev/null, as
//! init gives it); the syscall layer's and the host module's are in the
//! service's log file.
//!
//! Every signal is at -90 dBFS: inaudible, but seen by the HAL's peak
//! meter. Skipped unless the pinned NDK, the derived image and the HAL's test
//! client (`cargo aim build`) are present.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use aim_guest_init::{Boot, BootOptions, RunMode};

const HAL_LOG: &str = "vendor.audio-hal-aidl.log";
const HAL_TAG: &str = "android.hardware.audio.service-aidl.aim";
const SERVICES: &[&str] = &[
    "logd",
    "servicemanager",
    "hwservicemanager",
    "system_suspend",
    "vendor.audio-hal-aidl",
    "audioserver",
];

/// The pinned NDK's clang for the guest (arm64 Android), if installed.
fn ndk_clang() -> Option<PathBuf> {
    aim_paths::ndk_clang(35)
}

/// The derived image of `image/overlay.toml` (`cargo aim build derived-image`).
fn derived_image() -> Option<PathBuf> {
    aim_paths::input(aim_paths::derived_image(), "derived-image")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

struct Guest {
    path_map: PathBuf,
    binder: String,
}

impl Guest {
    /// A guest program without an identity: binder peers see it as root,
    /// as `adb root` would run it.
    fn run(&self, argv: &[&str], timeout: Duration) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_linux-run"))
            .arg("--path-map")
            .arg(&self.path_map)
            .arg("--binder")
            .arg(&self.binder)
            .arg("--seclabel")
            .arg("u:r:shell:s0")
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + timeout;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    }
}

impl Guest {
    /// What `tag` logged, from logd.
    fn log(&self, tag: &str) -> String {
        let out = self.run(
            &["/system/bin/logcat", "-d", "-v", "raw", "-s", tag],
            Duration::from_secs(30),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Wait until `tag` has logged `needle`.
    fn wait_for_log(&self, tag: &str, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.log(tag).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        false
    }
}

/// Stops the boot (and every service it started) when the test ends,
/// passed or failed.
struct Running(Option<std::thread::JoinHandle<String>>);

impl Running {
    fn stop(&mut self) -> String {
        aim_guest_init::boot::request_stop();
        self.0
            .take()
            .map(|t| t.join().unwrap_or_else(|_| "boot panicked".into()))
            .unwrap_or_default()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The value after `key` on the line that starts with `line`.
fn field(text: &str, line: &str, key: &str) -> Option<f64> {
    let l = text.lines().find(|l| l.starts_with(line))?;
    let mut words = l.split_whitespace();
    words.find(|w| *w == key)?;
    words.next()?.trim_end_matches(',').parse().ok()
}

#[test]
fn audioserver_and_the_hal_play_through_coreaudio() {
    let Some(clang) = ndk_clang() else {
        aim_paths::skip("the pinned NDK is not installed");
        return;
    };
    let Some(check) = aim_paths::input(aim_paths::hal_test("audio-hal-check"), "hal/audio-check")
    else {
        return;
    };
    let Some(image) = derived_image() else { return };

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("audio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let data = dir.join("data");
    let logs = aim_storage::data::runtime_of(&data).join("logs");
    // guest-init hosts the binder driver in this process; its services run
    // until the boot's timeout.
    let (ready, is_ready) = channel();
    let mut boot = Running(Some({
        let data = data.clone();
        std::thread::spawn(move || {
            let mut options = BootOptions::new(image, data, RunMode::Run);
            options.only = Some(SERVICES.iter().map(|s| s.to_string()).collect());
            options.linux_run = Some(env!("CARGO_BIN_EXE_linux-run").into());
            options.timeout = Some(Duration::from_secs(90));
            let mut boot = Boot::prepare(options).expect("guest-init");
            ready.send(boot.layout.path_map_file()).unwrap();
            boot.run().summary()
        })
    }));
    let guest = Guest {
        path_map: is_ready.recv().unwrap(),
        binder: format!("dev.aim.guest-init.{}.binder", std::process::id()),
    };
    let hal_log = logs.join(HAL_LOG);

    // audioserver, through the original libaudiohal@aidl: the policy
    // configuration comes from the HAL's ports and routes, and the primary
    // output is opened on a patch to the speaker.
    assert!(
        guest.wait_for_log(
            HAL_TAG,
            "opened Params { input: false",
            Duration::from_secs(60)
        ),
        "audioserver did not open the primary output:\nHAL:\n{}\n{}\naudioserver:\n{}",
        guest.log(HAL_TAG),
        read(&hal_log),
        read(&logs.join("audioserver.log"))
    );
    // The host module reached CoreAudio, not the null sink.
    assert!(!read(&hal_log).contains("null sink"), "{}", read(&hal_log));

    // The HAL's own client, as libaudiohal drives it.
    let tmp = data.join("data/local/tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(&check, tmp.join("audio-hal-check")).unwrap();
    let out = guest.run(
        &["/data/local/tmp/audio-hal-check", "2000", "1000"],
        Duration::from_secs(60),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{stdout}");
    assert!(
        out.status.success() && stdout.contains("ok done"),
        "{:?}\n{stdout}\n{}\nHAL:\n{}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        guest.log(HAL_TAG),
        read(&hal_log)
    );
    let written = field(&stdout, "output written", "written").unwrap();
    assert_eq!(written, 96096.0);
    assert_eq!(field(&stdout, "output drained", "position"), Some(written));
    let latency = field(
        &stdout,
        "output presentation_latency_ms",
        "presentation_latency_ms",
    )
    .unwrap();
    assert!((5.0..200.0).contains(&latency), "latency {latency} ms");
    assert!(field(&stdout, "input read", "read").unwrap() >= 48000.0);
    // What CoreAudio's render callback saw.
    let hal = guest.log(HAL_TAG);
    let standby = hal
        .lines()
        .find(|l| l.contains("output stream in standby: 96096 frames"))
        .unwrap_or_else(|| panic!("no standby report:\n{hal}"));
    eprintln!("{standby}");
    assert!(standby.contains("(0 while active)"), "{standby}");
    assert!(standby.contains("peak -90."), "{standby}");
    let callbacks: u32 = standby
        .split("frames, ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .unwrap();
    assert!(callbacks > 150, "{standby}");

    // Through libaaudio and AudioFlinger, once audioserver is public.
    let found = "Service media.audio_flinger: found";
    let registered = String::from_utf8_lossy(
        &guest
            .run(
                &["/system/bin/service", "check", "media.audio_flinger"],
                Duration::from_secs(30),
            )
            .stdout,
    )
    .contains(found);
    if registered {
        let program = tmp.join("audio_tone");
        let built = Command::new(clang)
            .args(["-O2", "-o"])
            .arg(&program)
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio_tone.c"))
            .args(["-laaudio", "-lm"])
            .status()
            .unwrap();
        assert!(built.success());
        let out = guest.run(
            &["/data/local/tmp/audio_tone", "2000", "0"],
            Duration::from_secs(60),
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        eprintln!("{stdout}");
        assert!(
            out.status.success() && stdout.contains("ok done"),
            "{stdout}"
        );
    } else {
        eprintln!(
            "not run: AAudio through AudioFlinger; audioserver has not registered \
             media.audio_flinger (it waits for system_server's activity service, P3)"
        );
    }

    // init's shutdown: guest-init stops the services and returns.
    guest.run(
        &["/system/bin/setprop", "sys.powerctl", "shutdown"],
        Duration::from_secs(30),
    );
    eprintln!("{}", boot.stop());
    let _ = std::fs::remove_dir_all(&dir);
}
