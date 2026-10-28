//! Run the image's original dex-use owner and protobuf/JNI code without a guest boot.
use std::{fs, os::fd::AsRawFd, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

// android-16.0.0_r1 art/libartservice/service/proto/dex_use.proto:
// primary loader a.b at t=123, plus a secondary path with user 0. The latter
// forces ArtJni validation during construction, before any notification.
fn existing_state() -> Vec<u8> {
    fn field(number: u8, bytes: &[u8]) -> Vec<u8> {
        assert!(bytes.len() < 128);
        [vec![number * 8 + 2, bytes.len() as u8], bytes.to_vec()].concat()
    }
    let record = [field(1, b"a.b"), vec![0x18, 123]].concat();
    let primary = [field(1, b"/data/app/a.b/base.apk"), field(2, &record)].concat();
    let secondary = [field(1, b"/data/user/0/a.b/extra.dex"), field(2, &[])].concat();
    field(
        1,
        &[field(1, b"a.b"), field(2, &primary), field(3, &secondary)].concat(),
    )
}

#[test]
#[ignore = "requires pinned derived image, JDK/d8 and debug linux-run; no guest boot"]
fn original_dex_use_owner_starts_records_and_reloads_existing_state() {
    use aim_android_init::{
        ImageRoot,
        props::{
            PropertyService,
            load::{self, PropertyInitOptions},
        },
    };
    use common::runtime::{Data, run, run_with_status_receipt};

    // Same shared base-image lease as boots/builders, opened read-only: this
    // test neither attaches images nor changes a shared input or its sidecar.
    let base = fs::canonicalize(aim_paths::system_image()).expect("NOT RUN: system image missing");
    let lease = fs::File::open(base.with_file_name(format!(
        "{}.use.lock",
        base.file_name().unwrap().to_str().unwrap()
    )))
    .unwrap();
    assert_eq!(
        unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
        0
    );

    let data = Data(std::env::temp_dir().join(format!("aim-dex-use-owner-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let image = aim_paths::derived_image();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    let dex = data.0.join("dex");
    for directory in [&stubs, &classes, &dex] {
        fs::create_dir(directory).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .arg(
            aim_paths::root().join("crates/aim-services/tests/fixtures/OriginalDexUseStartup.java"),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .args(
            fs::read_dir(classes.join("com/android/server/art"))
                .unwrap()
                .map(|entry| entry.unwrap().path()),
        ));

    let mut map = format!("root\t/\t{}\n", image.display());
    for name in ["data", "dev", "tmp", "linkerconfig"] {
        let directory = data.0.join(name);
        fs::create_dir(&directory).unwrap();
        map.push_str(&format!("rw\t/{name}\t{}\n", directory.display()));
    }
    map.push_str(&format!(
        "ro\t/oracle.dex\t{}\n",
        dex.join("classes.dex").display()
    ));
    let path_map = data.0.join("path-map");
    fs::write(&path_map, map).unwrap();
    let root = ImageRoot::new(&image);
    let info = load::create_serialized_property_info(&root, 36, &mut vec![]).unwrap();
    let mut properties = PropertyService::new(info).unwrap();
    load::property_init(&mut properties, &root, &PropertyInitOptions::default());
    properties
        .areas()
        .write_to_dir(&data.0.join("dev/__properties__"))
        .unwrap();

    // A standalone VM needs only these native namespaces, with all three
    // sharing the already loaded runtime. SystemProcessHost uses a shared
    // system-server class loader instead; its lifecycle is tested separately.
    let libraries = "/system/${LIB}:/apex/com.android.art/${LIB}:/apex/com.android.runtime/${LIB}/bionic:/apex/com.android.i18n/${LIB}:/apex/com.android.os.statsd/${LIB}";
    let mut config =
        String::from("dir.test=/apex\n[test]\nadditional.namespaces=system,com_android_art\n");
    for namespace in ["default", "system", "com_android_art"] {
        config.push_str(&format!("namespace.{namespace}.isolated=false\nnamespace.{namespace}.visible=true\nnamespace.{namespace}.search.paths={libraries}\n"));
    }
    for namespace in ["system", "com_android_art"] {
        config.push_str(&format!("namespace.{namespace}.links=default\nnamespace.{namespace}.link.default.allow_all_shared_libs=true\n"));
    }
    fs::write(data.0.join("linkerconfig/ld.config.txt"), config).unwrap();
    let oat = fs::read(image.join("system/framework/arm64/boot.oat")).unwrap();
    let key = b"bootclasspath\0";
    let at = oat
        .windows(key.len())
        .position(|window| window == key)
        .unwrap()
        + key.len();
    let end = oat[at..].iter().position(|byte| *byte == 0).unwrap();
    let bcp = format!(
        "{}:/system/framework/services.jar:/apex/com.android.art/javalib/service-art.jar:/oracle.dex",
        std::str::from_utf8(&oat[at..at + end]).unwrap()
    );
    // Keep the oracle and original Injector in the same loader (package-private
    // constructor); every original class/JNI library still comes from the image.
    let invoke = |mode: &str| {
        let mut command = Command::new(aim_paths::root().join("target/debug/linux-run"));
        command
            .env_clear()
            .envs(
                aim_linux_abi::default_android_env()
                    .iter()
                    .filter_map(|s| s.split_once('=')),
            )
            .args(["--inherit-env", "--root"])
            .arg(&image)
            .arg("--path-map")
            .arg(&path_map)
            .arg("/apex/com.android.art/bin/dalvikvm64")
            .arg(format!("-Xbootclasspath:{bcp}"))
            .arg(format!("-Xbootclasspath-locations:{bcp}"))
            .args([
                "-Xint",
                "-Djava.library.path=/apex/com.android.art/lib64:/system/lib64",
                "-cp",
                "/oracle.dex",
                "com.android.server.art.OriginalDexUseStartup",
                mode,
            ]);
        command
    };
    let output = invoke("uninitialized").output().unwrap();
    assert!(
        !output.status.success(),
        "original accepted missing ART initializer"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("ArtModuleServiceInitializer.getArtModuleServiceManager")
    );
    let state = data.0.join("data/system/package-dex-usage.pb");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, []).unwrap();
    run_with_status_receipt(&mut invoke("empty"));
    let existing = existing_state();
    fs::write(&state, &existing).unwrap();
    run_with_status_receipt(&mut invoke("existing"));
    assert_eq!(
        fs::read(&state).unwrap(),
        existing,
        "startup rewrote existing state"
    );
    run_with_status_receipt(&mut invoke("record"));
    let saved = fs::read(&state).unwrap();
    assert_ne!(saved, existing, "original did not persist the new load");
    run_with_status_receipt(&mut invoke("reload"));
    assert_eq!(
        fs::read(&state).unwrap(),
        saved,
        "reload rewrote saved state"
    );
}
