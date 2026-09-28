//! Java on the syscall layer (ADR 0012 phase P2, docs/art-exception-patches.md):
//! the image's original `dalvikvm64`, linker64, bionic and linker
//! configuration run a hello-world dex on the ART exception (the rebuilt
//! libart and friends and the regenerated boot image, which the derived
//! image of `image/overlay.toml` puts in place of the originals), first
//! without a boot image, then with it.
//!
//! Skipped unless the derived image (`cargo aim build`, which builds the
//! ART exception and its boot image), a JDK and the SDK's d8 are present.
//! Run it with a private CARGO_TARGET_DIR: it executes linux-run.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use aim_android_init::ImageRoot;
use aim_guest_init::paths::Layout;

/// What the fixture prints (checked against a host computation of `work`).
const EXPECTED: &str = "hello from ART: 1000 49168000 caught";
/// Without a boot image: the ART module's boot class path plus core-icu4j,
/// whose natives libart registers at start-up.
const IMAGELESS_BOOT_JARS: [&str; 6] = [
    "/apex/com.android.art/javalib/core-oj.jar",
    "/apex/com.android.art/javalib/core-libart.jar",
    "/apex/com.android.art/javalib/okhttp.jar",
    "/apex/com.android.art/javalib/bouncycastle.jar",
    "/apex/com.android.art/javalib/apache-xml.jar",
    "/apex/com.android.i18n/javalib/core-icu4j.jar",
];
/// The first mainline framework jar of the image's BOOTCLASSPATH; the boot
/// image extension covers it.
const MAINLINE_EXTENSION_JAR: &str =
    "/apex/com.android.adservices/javalib/framework-adservices.jar";
/// What dalvikvm64 maps: bionic, the ART, i18n and statsd APEXes, the
/// system libraries and the boot image.
const TRANSLATED: [&str; 6] = [
    "apex/com.android.runtime",
    "apex/com.android.art",
    "apex/com.android.i18n",
    "apex/com.android.os.statsd",
    "system/lib64",
    "system/framework",
];

/// javac and the newest SDK build-tools' d8.
fn java_tools() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let jdk = ["/opt/homebrew/opt/openjdk@17", "/opt/homebrew/opt/openjdk"]
        .iter()
        .map(PathBuf::from)
        .find(|j| j.join("bin/javac").exists())?;
    let sdk = aim_paths::sdk()?;
    let mut tools: Vec<PathBuf> = std::fs::read_dir(sdk.join("build-tools"))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path().join("d8")))
        .filter(|d8| d8.exists())
        .collect();
    tools.sort();
    Some((jdk.join("bin/javac"), tools.pop()?, jdk))
}

/// The derived image of `image/overlay.toml` (`cargo aim build derived-image`).
fn derived_image() -> Option<PathBuf> {
    aim_paths::input(aim_paths::derived_image(), "derived-image")
}

/// Compile the fixture to `<out>/classes.dex`.
fn build_dex(out: &Path) -> PathBuf {
    let (javac, d8, jdk) = java_tools().unwrap();
    let classes = out.join("classes");
    std::fs::create_dir_all(&classes).unwrap();
    let ok = Command::new(javac)
        .args(["--release", "11", "-d"])
        .arg(&classes)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/java/Hello.java"))
        .status()
        .unwrap();
    assert!(ok.success());
    let ok = Command::new(d8)
        .env("JAVA_HOME", jdk)
        .args(["--min-api", "35", "--output"])
        .arg(out)
        .arg(classes.join("Hello.class"))
        .status()
        .unwrap();
    assert!(ok.success());
    out.join("classes.dex")
}

/// The boot class path recorded in the original boot image's oat header;
/// the regenerated boot image covers the same jars.
fn image_boot_class_path() -> String {
    let oat =
        std::fs::read(aim_paths::original_image().join("system/framework/arm64/boot.oat")).unwrap();
    let key = b"bootclasspath\0";
    let at = oat.windows(key.len()).position(|w| w == key).unwrap() + key.len();
    let len = oat[at..].iter().position(|&b| b == 0).unwrap();
    String::from_utf8(oat[at..at + len].to_vec()).unwrap()
}

/// Removes `dir`, including the read-only entries of the translation cache.
fn remove_tree(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fn writable(path: &Path) {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    writable(&entry.path());
                }
            }
        }
    }
    writable(dir);
    let _ = std::fs::remove_dir_all(dir);
}

struct Guest {
    layout: Layout,
    cache: PathBuf,
}

impl Guest {
    /// Runs `argv` with init's environment plus `env`.
    fn run(&self, argv: &[&str], env: &[(&str, String)]) -> (Output, Duration) {
        let init_env = aim_linux_abi::default_android_env();
        let started = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_linux-run"))
            .env_clear()
            .envs(init_env.iter().filter_map(|v| v.split_once('=')))
            .envs(env.iter().map(|(k, v)| (k, v)))
            .arg("--inherit-env")
            .arg("--path-map")
            .arg(self.layout.path_map_file())
            .arg("--cache")
            .arg(&self.cache)
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = started + Duration::from_secs(300);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let out = child.wait_with_output().unwrap();
        (out, started.elapsed())
    }

    /// dalvikvm64 on the hello-world dex; returns its stdout and the wall
    /// time from exec to exit.
    fn hello(&self, options: &[&str], env: &[(&str, String)]) -> (String, Duration) {
        let mut argv = vec!["/apex/com.android.art/bin/dalvikvm64"];
        argv.extend_from_slice(options);
        argv.extend_from_slice(&["-cp", "/data/local/tmp/hello.dex", "Hello"]);
        let (out, took) = self.run(&argv, env);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success() && stdout.contains(EXPECTED),
            "{:?} {argv:?}\nstdout:\n{stdout}\nstderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        (stdout, took)
    }
}

#[test]
fn hello_world_runs_on_the_art_exception() {
    if java_tools().is_none() {
        aim_paths::skip("no JDK or SDK build-tools d8");
        return;
    }
    let Some(image) = derived_image() else { return };

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("art-{}", std::process::id()));
    remove_tree(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let layout = Layout::new(image.clone(), dir.join("data"), Some(dir.join("run")));
    layout.prepare().unwrap();
    std::fs::write(layout.path_map_file(), layout.path_map().to_file_text()).unwrap();
    let apexes = aim_guest_init::apex::scan(&ImageRoot::new(&image));
    std::fs::write(
        layout.apex_info_list(),
        aim_guest_init::apex::apex_info_list_xml(&apexes),
    )
    .unwrap();
    let tmp = layout.data.join("data/local/tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(build_dex(&dir.join("dex")), tmp.join("hello.dex")).unwrap();
    let guest = Guest {
        layout,
        cache: dir.join("cache"),
    };
    let (out, _) = guest.run(
        &[
            "/apex/com.android.runtime/bin/linkerconfig",
            "--target",
            "/linkerconfig",
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "linkerconfig: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // No boot image: the runtime starts from the jars (ClassLinker::
    // InitWithoutImage).
    let jars = IMAGELESS_BOOT_JARS.join(":");
    let (bcp, locations) = (
        format!("-Xbootclasspath:{jars}"),
        format!("-Xbootclasspath-locations:{jars}"),
    );
    let no_image = [
        bcp.as_str(),
        locations.as_str(),
        "-Ximage:/nonexistent/boot.art",
        "-Xnoimage-dex2oat",
    ];
    let no_image_int: Vec<&str> = no_image.iter().copied().chain(["-Xint"]).collect();
    let (stdout, _) = guest.hello(&no_image_int, &[]);
    eprint!("{stdout}");

    // The regenerated boot image at its default location, with the boot
    // class path init exports: the image's DEX2OATBOOTCLASSPATH, then the
    // first mainline framework jar (covered by the boot image extension).
    let dex2oat_bcp = image_boot_class_path();
    let boot_env = [
        ("DEX2OATBOOTCLASSPATH", dex2oat_bcp.clone()),
        (
            "BOOTCLASSPATH",
            format!("{dex2oat_bcp}:{MAINLINE_EXTENSION_JAR}"),
        ),
    ];
    type Configuration<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, String)]);
    let configurations: [Configuration; 4] = [
        ("no boot image, interpreter", &no_image_int, &[]),
        ("no boot image, JIT", &no_image, &[]),
        ("boot image, interpreter", &["-Xint"], &boot_env),
        ("boot image, JIT", &[], &boot_env),
    ];
    // Start-up to exit, best of two: first with every file rewritten at load
    // time, then with the translation cache filled for what dalvikvm64 maps.
    let measure = |guest: &Guest| -> Vec<Duration> {
        configurations
            .iter()
            .map(|(_, options, env)| (0..2).map(|_| guest.hello(options, env).1).min().unwrap())
            .collect()
    };
    let load_time = measure(&guest);
    let translated = Command::new(env!("CARGO_BIN_EXE_linux-translate"))
        .arg("--cache")
        .arg(&guest.cache)
        .args(TRANSLATED.iter().map(|d| image.join(d)))
        .output()
        .unwrap();
    assert!(translated.status.success(), "{translated:?}");
    let cached = measure(&guest);
    for (i, (name, _, _)) in configurations.iter().enumerate() {
        eprintln!(
            "{name}: {:?} (load-time rewriting), {:?} (translation cache)",
            load_time[i], cached[i]
        );
    }
    remove_tree(&dir);
}
