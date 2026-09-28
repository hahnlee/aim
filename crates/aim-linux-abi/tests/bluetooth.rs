//! The Bluetooth HAL on the derived image (ADR 0012 phase P5,
//! docs/bluetooth.md): the original servicemanager, our
//! `IBluetoothHci/default` over the CoreBluetooth virtual controller, and a
//! client that speaks HCI through the AIDL interface as the Android stack's
//! HAL layer does (reset, version, an LE scan of real advertisements and,
//! when a named connectable device is near, a GATT Device Name read).
//!
//! The Android stack itself runs in the Bluetooth app (Java), so it waits
//! for ART and system_server. Its native library, built with the shadow
//! call stack, is loaded through the original linker instead, which runs
//! its constructors.
//!
//! Skipped unless the extracted image and the vendor HAL outputs
//! (tools/build-vendor-hals.sh) are present. CoreBluetooth asks for the
//! Bluetooth permission on first use; until it is granted the scan finds
//! nothing, which the test reports but does not fail on. Run it with a
//! private CARGO_TARGET_DIR: it executes linux-run.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use aim_android_init::ImageRoot;
use aim_android_init::props::Ucred;
use aim_android_init::props::load::{
    PropertyInitOptions, create_serialized_property_info, property_init, start_property_service,
};
use aim_android_init::props::protocol::{PROP_SUCCESS, Request, serve};
use aim_binder_host::server::Server;
use aim_guest_init::paths::Layout;
use aim_guest_init::props::mapped_properties;
use aim_guest_init::propsvc::{PropertyEvent, PropertySockets};

const IMAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/android16-image-full"
);
const HALS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../_build/vendor-hals/bin");
const SERVICEMANAGER_LABEL: &str = "u:r:servicemanager:s0";
const INSTANCE: &str = "android.hardware.bluetooth.IBluetoothHci/default";
const AID_SYSTEM: u32 = 1000;

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ndk_clang() -> Option<PathBuf> {
    let sdk = std::env::var_os("ANDROID_SDK_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join("Library/Android/sdk")))?;
    let clang = sdk.join(
        "ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang",
    );
    clang.exists().then_some(clang)
}

/// The derived image of `image/overlay.toml`, assembled (APFS clones) once
/// per overlay identity.
fn derived_image() -> Result<PathBuf, String> {
    let root = source_root();
    let original = Path::new(IMAGE).canonicalize().map_err(|e| e.to_string())?;
    let (_, plan) = aim_android_image::load(&root.join("image/overlay.toml"), &original, &root)
        .map_err(|problems| format!("{problems:?}"))?;
    let identity = aim_android_image::identity::compute("bluetooth-test-original", &plan);
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bluetooth-derived-image");
    aim_android_image::assemble(&plan, &original, &identity, &out)?;
    Ok(out)
}

/// Init's property areas and service; successful sets are reported.
fn start_properties(image: &Path, layout: &Layout) -> Receiver<(String, String)> {
    let (changes, changed) = channel();
    let (up, is_up) = channel();
    let (image, props_dir, socket_dir) = (
        ImageRoot::new(image),
        layout.properties_dir(),
        layout.socket_dir(),
    );
    std::thread::spawn(move || {
        let mut diagnostics = Vec::new();
        let vendor_api_level = aim_android_init::rc::vendor_android_version(&image).unwrap_or(36);
        let info = create_serialized_property_info(&image, vendor_api_level, &mut diagnostics)
            .expect("property_info");
        let mut props = mapped_properties(&props_dir, info).expect("property areas");
        let options = PropertyInitOptions {
            vendor_api_level,
            ..Default::default()
        };
        property_init(&mut props, &image, &options);
        start_property_service(&mut props);
        let (events, requests) = channel();
        let _sockets = PropertySockets::start(&socket_dir, events).expect("property sockets");
        up.send(()).unwrap();
        for PropertyEvent::Set(request) in requests {
            let cred = Ucred {
                pid: request.peer_pid,
                uid: AID_SYSTEM,
                gid: AID_SYSTEM,
            };
            let served = serve(
                &mut props,
                &request.request,
                Some(SERVICEMANAGER_LABEL),
                &cred,
            );
            if served.reply == Some(PROP_SUCCESS)
                && let Request::SetProp2 { name, value } = &request.request
            {
                let _ = changes.send((
                    String::from_utf8_lossy(name).into_owned(),
                    String::from_utf8_lossy(value).into_owned(),
                ));
            }
            if let Some(code) = served.reply {
                request.reply(code);
            }
        }
    });
    is_up
        .recv_timeout(Duration::from_secs(60))
        .expect("property service");
    changed
}

struct Guest<'a> {
    layout: &'a Layout,
    cache: PathBuf,
    binder: String,
}

impl Guest<'_> {
    fn command(&self, seclabel: &str, argv: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_linux-run"));
        c.arg("--path-map")
            .arg(self.layout.path_map_file())
            .arg("--cache")
            .arg(&self.cache)
            .arg("--binder")
            .arg(&self.binder)
            .arg("--seclabel")
            .arg(seclabel)
            .args(argv)
            .stdin(Stdio::null());
        c
    }

    /// Run to completion, killed after `timeout`; the output is read
    /// meanwhile, so a chatty program never blocks on a full pipe.
    fn run(&self, argv: &[&str], timeout: Duration) -> Output {
        let child = self
            .command("u:r:shell:s0", argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let (done, output) = channel();
        std::thread::spawn(move || done.send(child.wait_with_output().unwrap()));
        output.recv_timeout(timeout).unwrap_or_else(|_| {
            // SAFETY: our own child, not yet reaped.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            output.recv().unwrap()
        })
    }
}

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn log_of(path: &Path) -> String {
    let mut text = String::new();
    let _ = std::fs::File::open(path).map(|mut f| f.read_to_string(&mut text));
    text
}

#[test]
fn hci_through_the_hal_and_the_stack_library() {
    if !Path::new(IMAGE).join("system/bin/servicemanager").exists() {
        eprintln!("skipped: extracted image not found at {IMAGE}");
        return;
    }
    let client_bin = Path::new(HALS).join("bluetooth-hci-client");
    if !client_bin.exists() {
        eprintln!("skipped: no vendor HAL outputs (run tools/build-vendor-hals.sh)");
        return;
    }
    let image = match derived_image() {
        Ok(image) => image,
        Err(e) => {
            eprintln!("skipped: no derived image (run tools/build-vendor-hals.sh): {e}");
            return;
        }
    };

    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bluetooth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let layout = Layout::new(image.clone(), dir.join("data"), Some(dir.join("run")));
    layout.prepare().unwrap();
    std::fs::write(layout.path_map_file(), layout.path_map().to_file_text()).unwrap();
    let changed = start_properties(&image, &layout);
    let binder = format!("dev.aim.test.bluetooth.{}", std::process::id());
    let _server = Server::start(&binder).expect("binder host");
    let guest = Guest {
        layout: &layout,
        cache: dir.join("cache"),
        binder,
    };

    let apexes = aim_guest_init::apex::scan(&ImageRoot::new(&image));
    std::fs::write(
        layout.apex_info_list(),
        aim_guest_init::apex::apex_info_list_xml(&apexes),
    )
    .unwrap();
    let linkerconfig = guest.run(
        &[
            "/apex/com.android.runtime/bin/linkerconfig",
            "--target",
            "/linkerconfig",
        ],
        Duration::from_secs(120),
    );
    assert!(
        linkerconfig.status.success(),
        "linkerconfig: {}",
        String::from_utf8_lossy(&linkerconfig.stderr)
    );

    let sm_log = dir.join("servicemanager.log");
    let log = std::fs::File::create(&sm_log).unwrap();
    let _sm = Kill(
        guest
            .command(SERVICEMANAGER_LABEL, &["/system/bin/servicemanager"])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match changed.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((name, _)) if name == "servicemanager.ready" => break,
            Ok(_) => {}
            Err(_) => panic!("servicemanager never became ready:\n{}", log_of(&sm_log)),
        }
    }

    let hal_log = dir.join("bluetooth-hal.log");
    let log = std::fs::File::create(&hal_log).unwrap();
    let _hal = Kill(
        guest
            .command(
                "u:r:hal_bluetooth_default:s0",
                &["/vendor/bin/hw/android.hardware.bluetooth-service.aim"],
            )
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let found = format!("Service {INSTANCE}: found");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let out = guest.run(
            &["/system/bin/service", "check", INSTANCE],
            Duration::from_secs(60),
        );
        if String::from_utf8_lossy(&out.stdout).contains(&found) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Bluetooth HAL never registered:\n{}",
            log_of(&hal_log)
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    let program = layout.data.join("data/local/tmp/bluetooth-hci-client");
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::copy(&client_bin, &program).unwrap();
    let out = guest.run(
        &["/data/local/tmp/bluetooth-hci-client", "10", "connect"],
        Duration::from_secs(180),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{stdout}");
    let context = || {
        format!(
            "{:?}\n{stdout}\n{}\nHAL:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
            log_of(&hal_log)
        )
    };
    assert!(out.status.success(), "{}", context());
    for step in [
        "ok initialized",
        "ok reset",
        "ok version hci=0x0c",
        "ok features le=true br_edr_not_supported=true",
        "ok scan",
        "ok done",
    ] {
        assert!(stdout.contains(step), "missing {step:?}\n{}", context());
    }
    if stdout.contains("ok scan reports=0 ") {
        eprintln!(
            "note: no advertisements; is Bluetooth allowed for this terminal? (HAL log:\n{})",
            log_of(&hal_log)
        );
    }

    // The stack's native library (SCS-built) loads, constructors and all.
    // Constructors of its HIDL dependencies wait for hwservicemanager.
    let hwsm_log = dir.join("hwservicemanager.log");
    let log = std::fs::File::create(&hwsm_log).unwrap();
    let _hwsm = Kill(
        guest
            .command("u:r:hwservicemanager:s0", &["/system/bin/hwservicemanager"])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match changed.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((name, _)) if name == "hwservicemanager.ready" => break,
            Ok(_) => {}
            Err(_) => panic!(
                "hwservicemanager never became ready:\n{}",
                log_of(&hwsm_log)
            ),
        }
    }
    match ndk_clang() {
        None => eprintln!("skipped the stack library: the pinned NDK (r28c) is not installed"),
        Some(clang) => {
            let program = layout.data.join("data/local/tmp/dlopen_library");
            let built = Command::new(clang)
                .args(["-O2", "-o"])
                .arg(&program)
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dlopen_library.c"))
                .status()
                .unwrap();
            assert!(built.success());
            let out = guest.run(
                &[
                    "/data/local/tmp/dlopen_library",
                    "/apex/com.android.bt/lib64/libbluetooth_jni.so",
                    "JNI_OnLoad",
                ],
                Duration::from_secs(180),
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            eprintln!("{stdout}");
            assert!(
                out.status.success() && stdout.contains("ok symbol JNI_OnLoad"),
                "{:?}\n{stdout}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
