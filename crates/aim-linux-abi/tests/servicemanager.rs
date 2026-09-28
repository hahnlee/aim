//! The original servicemanager of the pinned image, as the context manager
//! of the daemon-hosted binder driver, serving original clients in other
//! processes (ADR 0012 phase P1; #167, #168). Skipped when the extracted
//! image is absent.
//!
//! The test plays the minimum of init: property areas and the property
//! service (`aim-guest-init`), the guest filesystem view (`--path-map`),
//! and the binder host (`aim-binder-host`, in this process).

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

const SERVICEMANAGER_LABEL: &str = "u:r:servicemanager:s0";
const AID_SYSTEM: u32 = 1000;

/// The extracted pinned image (the `image` node of `cargo aim`).
fn image() -> Option<PathBuf> {
    aim_paths::original_image_with("system/bin/servicemanager")
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = aim_linux_abi::cache::remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Property areas at `/dev/__properties__` and init's property service at
/// `/dev/socket/property_service`. Successful sets are reported on the
/// returned channel.
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
            // Only servicemanager sets properties here.
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

    fn run(&self, argv: &[&str]) -> Output {
        let mut child = self
            .command("u:r:shell:s0", argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let out = child.wait_with_output().unwrap();
                panic!(
                    "{argv:?} timed out\n{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn run_ok(&self, argv: &[&str]) -> String {
        let out = self.run(argv);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{argv:?}: {:?}\n{stdout}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }
}

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The pinned NDK's clang for the guest (arm64 Android), if installed.
fn ndk_clang() -> Option<PathBuf> {
    aim_paths::ndk_clang(35)
}

#[test]
fn original_servicemanager_serves_original_clients() {
    let Some(ref image) = image() else { return };
    let dir = scratch("servicemanager");
    let layout = Layout::new(image.into(), dir.join("data"), Some(dir.join("run")));
    layout.prepare().unwrap();
    std::fs::write(layout.path_map_file(), layout.path_map().to_file_text()).unwrap();
    let changed = start_properties(image, &layout);

    let binder = format!("dev.aim.test.binder.{}", std::process::id());
    let _server = Server::start(&binder).expect("binder host");
    let guest = Guest {
        layout: &layout,
        cache: dir.join("cache"),
        binder,
    };

    let sm_log = dir.join("servicemanager.log");
    let log = std::fs::File::create(&sm_log).unwrap();
    let sm = Kill(
        guest
            .command(SERVICEMANAGER_LABEL, &["/system/bin/servicemanager"])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    // servicemanager sets this once it is the context manager and polls.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match changed.recv_timeout(left) {
            Ok((name, value)) if name == "servicemanager.ready" => {
                assert_eq!(value, "true");
                break;
            }
            Ok(_) => {}
            Err(_) => {
                let mut text = String::new();
                let _ = std::fs::File::open(&sm_log).map(|mut f| f.read_to_string(&mut text));
                panic!("servicemanager never became ready:\n{text}");
            }
        }
    }

    let list = guest.run_ok(&["/system/bin/service", "list"]);
    eprintln!("service list:\n{list}");
    assert!(
        list.contains("manager: [android.os.IServiceManager]"),
        "{list}"
    );
    let check = guest.run_ok(&["/system/bin/service", "check", "manager"]);
    assert!(check.contains("Service manager: found"), "{check}");
    let missing = guest.run_ok(&["/system/bin/service", "check", "servicemanager"]);
    assert!(
        missing.contains("Service servicemanager: not found"),
        "{missing}"
    );
    let cmd = guest.run_ok(&["/system/bin/cmd", "-l"]);
    assert!(cmd.lines().any(|l| l.trim() == "manager"), "{cmd}");

    // A service in a third process: addService, getService and direct
    // calls, its death, and the round-trip latency of small calls.
    let Some(clang) = ndk_clang() else {
        aim_paths::skip("the pinned NDK is not installed (service and latency checks)");
        drop(sm);
        let _ = aim_linux_abi::cache::remove_tree(&dir);
        return;
    };
    let bench = layout.data.join("data/local/tmp/binder_ping");
    std::fs::create_dir_all(bench.parent().unwrap()).unwrap();
    let built = Command::new(clang)
        .args(["-O2", "-Wno-deprecated-declarations", "-o"])
        .arg(&bench)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/binder_ping.c"))
        .arg("-lbinder_ndk")
        .status()
        .unwrap();
    assert!(built.success());
    let bench = "/data/local/tmp/binder_ping";
    let iterations = if cfg!(debug_assertions) {
        "2000"
    } else {
        "20000"
    };
    let ping = guest.run_ok(&[bench, "ping", iterations]);
    eprintln!("{}", ping.trim());
    assert!(ping.starts_with("binder ping: n="), "{ping}");

    let name = "dev.aim.ping";
    let server = Kill(
        guest
            .command("u:r:shell:s0", &[bench, "serve", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let found = format!("Service {name}: found");
    wait_for(|| {
        guest
            .run_ok(&["/system/bin/service", "check", name])
            .contains(&found)
    });
    let call = guest.run_ok(&[bench, "call", name, iterations]);
    eprintln!("{}", call.trim());
    assert!(call.starts_with("binder call: n="), "{call}");
    // servicemanager holds a death notification for each service.
    drop(server);
    let gone = format!("Service {name}: not found");
    wait_for(|| {
        guest
            .run_ok(&["/system/bin/service", "check", name])
            .contains(&gone)
    });
    drop(sm);
    let _ = aim_linux_abi::cache::remove_tree(&dir);
}

fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
}
