//! Graphics buffers and GLES on the derived image (ADR 0012 phase P4,
//! docs/graphics-buffers.md, docs/gles-driver.md): an NDK program allocates
//! an AHardwareBuffer through the original libui (our allocator HAL over
//! binder, our mapper in-process), creates an EGL context through the
//! original libEGL loader (our GLES driver, ANGLE on the host) and renders
//! into a pbuffer, into the buffer and into a window surface (an
//! ImageReader's queue).
//!
//! Skipped unless the extracted image, the pinned NDK, the ANGLE build and
//! the vendor HAL outputs (tools/build-vendor-hals.sh) are all present.
//! Run it with a private CARGO_TARGET_DIR: it executes linux-run.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use darwin_android_init::ImageRoot;
use darwin_android_init::props::Ucred;
use darwin_android_init::props::load::{
    KernelBootProperties, PropertyInitOptions, create_serialized_property_info, property_init,
    start_property_service,
};
use darwin_android_init::props::protocol::{PROP_SUCCESS, Request, serve};
use darwin_binder_host::server::Server;
use darwin_guest_init::paths::Layout;
use darwin_guest_init::props::mapped_properties;
use darwin_guest_init::propsvc::{PropertyEvent, PropertySockets};

const IMAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/android16-image-full"
);
const ANGLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/angle-source/out/DarwinArtRelease"
);
const SERVICEMANAGER_LABEL: &str = "u:r:servicemanager:s0";
const ALLOCATOR: &str = "android.hardware.graphics.allocator.IAllocator/default";
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
    let (_, plan) = darwin_android_image::load(&root.join("image/overlay.toml"), &original, &root)
        .map_err(|problems| format!("{problems:?}"))?;
    let identity = darwin_android_image::identity::compute("graphics-test-original", &plan);
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("graphics-derived-image");
    darwin_android_image::assemble(&plan, &original, &identity, &out)?;
    Ok(out)
}

/// Init's property areas and service. `ro.hardware.egl` is what
/// init.darwin.rc sets in early-init.
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
        let vendor_api_level =
            darwin_android_init::rc::vendor_android_version(&image).unwrap_or(36);
        let info = create_serialized_property_info(&image, vendor_api_level, &mut diagnostics)
            .expect("property_info");
        let mut props = mapped_properties(&props_dir, info).expect("property areas");
        let options = PropertyInitOptions {
            kernel: KernelBootProperties {
                bootconfig: vec![("androidboot.hardware".into(), "darwin".into())],
                ..Default::default()
            },
            vendor_api_level,
            ..Default::default()
        };
        property_init(&mut props, &image, &options);
        props.init_set("ro.hardware.egl", "darwin");
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
            .arg("--gpu")
            .arg(ANGLE)
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
        let deadline = Instant::now() + Duration::from_secs(180);
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
fn allocates_and_renders_through_the_original_libui_and_libegl() {
    if !Path::new(IMAGE).join("system/bin/servicemanager").exists() {
        eprintln!("skipped: extracted image not found at {IMAGE}");
        return;
    }
    let Some(clang) = ndk_clang() else {
        eprintln!("skipped: the pinned NDK (r28c) is not installed");
        return;
    };
    if !Path::new(ANGLE).join("libGLESv2.dylib").exists() {
        eprintln!("skipped: no ANGLE build at {ANGLE}");
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
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("graphics-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let layout = Layout::new(image.clone(), dir.join("data"), Some(dir.join("run")));
    layout.prepare().unwrap();
    std::fs::write(layout.path_map_file(), layout.path_map().to_file_text()).unwrap();
    let changed = start_properties(&image, &layout);
    let binder = format!("dev.darwinart.test.graphics.{}", std::process::id());
    let _server = Server::start(&binder).expect("binder host");
    let guest = Guest {
        layout: &layout,
        cache: dir.join("cache"),
        binder,
    };

    // init's GenerateLinkerConfiguration: the sphal namespace that
    // libEGL and libui load vendor drivers into.
    let apexes = darwin_guest_init::apex::scan(&ImageRoot::new(&image));
    std::fs::write(
        layout.apex_info_list(),
        darwin_guest_init::apex::apex_info_list_xml(&apexes),
    )
    .unwrap();
    let linkerconfig = guest.run(&[
        "/apex/com.android.runtime/bin/linkerconfig",
        "--target",
        "/linkerconfig",
    ]);
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

    let allocator_log = dir.join("allocator.log");
    let log = std::fs::File::create(&allocator_log).unwrap();
    let _allocator = Kill(
        guest
            .command(
                "u:r:hal_graphics_allocator_default:s0",
                &["/vendor/bin/hw/android.hardware.graphics.allocator-service.darwin"],
            )
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let found = format!("Service {ALLOCATOR}: found");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let out = guest.run(&["/system/bin/service", "check", ALLOCATOR]);
        if String::from_utf8_lossy(&out.stdout).contains(&found) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "allocator never registered:\n{}",
            log_of(&allocator_log)
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    let program = layout.data.join("data/local/tmp/gles_triangle");
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    let built = Command::new(clang)
        .args(["-O2", "-o"])
        .arg(&program)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gles_triangle.c"))
        .args(["-lEGL", "-lGLESv3", "-lnativewindow", "-lmediandk"])
        .status()
        .unwrap();
    assert!(built.success());
    let out = guest.run(&["/data/local/tmp/gles_triangle"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{stdout}");
    assert!(
        out.status.success() && stdout.contains("ok done"),
        "{:?}\n{stdout}\n{}\nallocator:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        log_of(&allocator_log)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
