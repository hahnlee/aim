//! The SystemServer exception on the pinned image's services.jar: the
//! patched jar leaves ClipboardService unstarted and everything else as it
//! was, with valid checksums.

use aim_android_image::dex::{self, Dex};
use aim_android_image::redirect;
use aim_android_image::system_server::patch_services_jar;
use sha1::{Digest, Sha1};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const JAR: &str = "system/framework/services.jar";

fn entries(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap();
    // The dex entries are stored: read them through their local headers
    // and check their CRCs.
    let mut out = Vec::new();
    let mut at = 0;
    while bytes[at..at + 4] == [0x50, 0x4b, 0x03, 0x04] {
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
        let (method, size, crc) = (u16_at(at + 8), u32_at(at + 18), u32_at(at + 14));
        let (name_len, extra_len) = (u16_at(at + 26), u16_at(at + 28));
        let name = String::from_utf8_lossy(&bytes[at + 30..at + 30 + name_len]).into_owned();
        let data = at + 30 + name_len + extra_len;
        if method == 0 {
            let body = bytes[data..data + size].to_vec();
            assert_eq!(crc32fast::hash(&body) as usize, crc, "{name}: CRC");
            out.push((name, body));
        }
        at = data + size;
    }
    out
}

#[test]
fn clipboard_is_not_started() {
    let Some(image) = aim_paths::original_image_with(JAR) else {
        return;
    };
    let out = std::env::temp_dir().join(format!("services-{}.jar", std::process::id()));
    patch_services_jar(
        &image.join(JAR),
        &out,
        &["com.android.server.clipboard.ClipboardService".into()],
    )
    .unwrap();
    let original = entries(&image.join(JAR));
    let patched = entries(&out);
    std::fs::remove_file(&out).unwrap();
    assert_eq!(original.len(), patched.len());
    let mut changed = 0;
    for ((name, before), (_, after)) in original.iter().zip(&patched) {
        if before == after {
            continue;
        }
        changed += 1;
        assert_eq!(before.len(), after.len());
        // Checksums as ART's verifier checks them.
        assert_eq!(
            &after[12..32],
            Sha1::digest(&after[32..]).as_slice(),
            "{name}"
        );
        let mut adler = adler2::Adler32::new();
        adler.write_slice(&after[12..]);
        assert_eq!(after[8..12], adler.checksum().to_le_bytes(), "{name}");
        // Exactly the five units of const-class + invoke-virtual differ.
        let differing: Vec<usize> = (32..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        let (first, last) = (differing[0], *differing.last().unwrap());
        assert!(
            last - first < 10,
            "{name}: edits span {first:#x}..{last:#x}"
        );
        assert!(after[first..=last].iter().all(|b| *b == 0));
        let dex = Dex::parse(after).unwrap();
        let class = dex.class("Lcom/android/server/SystemServer;").unwrap();
        let code = dex.methods_named(class, "startOtherServices").unwrap();
        let units = dex::units(after, &code[0]).unwrap();
        let mut at = 0;
        while at < units.len() {
            if units[at] & 0xff == 0x1c {
                let loaded = dex.type_name(u32::from(units[at + 1])).unwrap();
                assert_ne!(loaded, "Lcom/android/server/clipboard/ClipboardService;");
            }
            at += dex::instruction_units(&units, at).unwrap();
        }
    }
    assert_eq!(changed, 1);
}

/// The pinned Java toolchain of the `device-services` node
/// (upstream/java-toolchain.lock): the JDK's home and the build tools.
fn java_toolchain() -> Option<(PathBuf, PathBuf)> {
    let lock =
        std::fs::read_to_string(aim_paths::root().join("upstream/java-toolchain.lock")).ok()?;
    let get = |key: &str| {
        lock.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .unwrap()
    };
    let java = aim_paths::fetched().join("java");
    let jdk = java
        .join(format!("temurin-{}", get("JDK_VERSION")))
        .join(format!("jdk-{}/Contents/Home", get("JDK_VERSION")));
    let tools = java
        .join(format!("build-tools-{}", get("BUILD_TOOLS_VERSION")))
        .join(get("BUILD_TOOLS_DIR"));
    if !jdk.join("bin/javac").exists() || !tools.join("dexdump").exists() {
        aim_paths::skip("no pinned Java toolchain (cargo aim build device-services)");
        return None;
    }
    Some((jdk, tools))
}

/// The fixture's dex, `dev.aim.server.test.Redirects`.
fn fixture_dex(jdk: &Path, tools: &Path, work: &Path) -> Vec<u8> {
    let classes = work.join("classes");
    let ok = Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Redirects.java"))
        .status()
        .unwrap();
    assert!(ok.success());
    let ok = Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(tools.join("lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(jdk)
        .arg("--output")
        .arg(work)
        .arg(classes.join("dev/aim/server/test/Redirects.class"))
        .status()
        .unwrap();
    assert!(ok.success());
    std::fs::read(work.join("classes.dex")).unwrap()
}

/// `dexdump -d -a` of `jar`, one normalized line at a time: what names
/// an index or an offset, which a rewrite changes, is left out. dexdump
/// verifies each dex (ART's `DexFileVerifier`) before it dumps it.
fn dump(dexdump: &Path, jar: &Path) -> (Child, impl Iterator<Item = String> + use<>) {
    let mut child = Command::new(dexdump)
        .args(["-d", "-a"])
        .arg(jar)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = BufReader::new(child.stdout.take().unwrap())
        .split(b'\n')
        .map(|l| String::from_utf8_lossy(&l.unwrap()).into_owned())
        .filter(|l| !l.starts_with("Processing '") && !l.starts_with("Opened '"))
        .map(|l| {
            let mut l = match l.split_once('|') {
                Some((at, rest)) if at.ends_with(' ') && at.contains(": ") => {
                    match rest.strip_prefix('[').and_then(|r| r.split_once("] ")) {
                        Some((_, method)) => method.to_string(),
                        None => rest.to_string(),
                    }
                }
                _ => l,
            };
            if let Some((code, comment)) = l.rsplit_once(" // ")
                && comment.split_once('@').is_some_and(|(kind, index)| {
                    kind.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
                        && index.chars().all(|c| c.is_ascii_hexdigit())
                })
            {
                l = code.to_string();
            }
            if l.starts_with("Annotations on ") || l.trim_start().starts_with("source_file_idx") {
                l.retain(|c| !c.is_ascii_digit());
            }
            l
        });
    (child, lines)
}

const REDIRECTS: &str = "\
com.android.server.SystemServer.dumpHprof java.text.SimpleDateFormat.format dev.aim.server.test.Redirects.format 1 a virtual call; a new proto and type list
com.android.server.SystemServer.dumpHprof java.util.TreeSet.pollLast dev.aim.server.test.Redirects.pollLast 1 a new proto with a shared type list
com.android.server.SystemServer.reportWtf java.lang.StringBuilder.append dev.aim.server.test.Redirects.append 2 two calls
com.android.server.SystemServer.run java.lang.System.loadLibrary dev.aim.server.test.Redirects.loadLibrary 1 a static call
com.android.server.SystemServer.handleEarlySystemWtf com.android.server.am.EventLogTags.writeAmWtf dev.aim.server.test.Redirects.writeAmWtf 1 a static range call
com.android.server.pm.UserManagerService.addRemovingUserIdLocked java.util.LinkedList.add dev.aim.server.test.Redirects.add 1 in classes2.dex
com.android.server.pm.UserManagerService.getNextAvailableId java.util.Iterator.hasNext dev.aim.server.test.Redirects.hasNext 1 an interface call
";

#[test]
fn redirects_calls() {
    let Some(image) = aim_paths::original_image_with(JAR) else {
        return;
    };
    let Some((jdk, tools)) = java_toolchain() else {
        return;
    };
    let work = std::env::temp_dir().join(format!("redirect-{}", std::process::id()));
    let fixture = fixture_dex(&jdk, &tools, &work);
    let services = std::fs::read(image.join(JAR)).unwrap();
    let redirects = redirect::parse(REDIRECTS).unwrap();
    let jar = redirect::redirect_jar(&services, &redirects, &[&fixture]).unwrap();
    let out = work.join("services.jar");
    std::fs::write(&out, &jar).unwrap();

    // Stored entries stay aligned, every CRC holds.
    let original = entries(&image.join(JAR));
    let patched = entries(&out);
    assert_eq!(original.len(), patched.len());
    for ((name, before), (_, after)) in original.iter().zip(&patched) {
        assert_eq!(before == after, name == "classes3.dex", "{name}");
    }

    // The same code, but the redirected calls.
    let dexdump = tools.join("dexdump");
    let (mut a, mut before) = dump(&dexdump, &image.join(JAR));
    let (mut b, mut after) = dump(&dexdump, &out);
    let mut changed = Vec::new();
    let mut lines = 0;
    loop {
        let pair = (before.next(), after.next());
        if pair == (None, None) {
            break;
        }
        lines += 1;
        if pair.0 != pair.1 {
            changed.push(pair);
        }
    }
    for child in [&mut a, &mut b] {
        let mut errors = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut errors)
            .unwrap();
        assert!(child.wait().unwrap().success(), "dexdump: {errors}");
        assert!(!errors.contains("Failure"), "dexdump: {errors}");
    }
    std::fs::remove_dir_all(&work).unwrap();
    assert!(lines > 1_000_000);
    let calls: usize = redirects.iter().map(|r| r.calls).sum();
    assert_eq!(changed.len(), calls, "{changed:#?}");
    for (x, y) in &changed {
        let (x, y) = (x.as_deref().unwrap(), y.as_deref().unwrap());
        assert!(y.starts_with(&x[..4]), "{x} -> {y}");
        assert!(
            y.contains("invoke-static") && y.contains("Ldev/aim/server/test/Redirects;."),
            "{x} -> {y}"
        );
    }
}

#[test]
fn refuses_redirects() {
    let Some(image) = aim_paths::original_image_with(JAR) else {
        return;
    };
    let Some((jdk, tools)) = java_toolchain() else {
        return;
    };
    let work = std::env::temp_dir().join(format!("refused-{}", std::process::id()));
    let fixture = fixture_dex(&jdk, &tools, &work);
    std::fs::remove_dir_all(&work).unwrap();
    let services = std::fs::read(image.join(JAR)).unwrap();
    let refused = |line: &str| {
        let redirects = redirect::parse(line).unwrap();
        match redirect::redirect_jar(&services, &redirects, &[&fixture]) {
            Ok(_) => panic!("accepted: {line}"),
            Err(e) => e,
        }
    };
    let site =
        "com.android.server.pm.UserManagerService.getNextAvailableId java.util.Iterator.hasNext";
    for (to, error) in [
        ("hasNext 2", "1 calls, not 2"),
        ("instanceHasNext 1", "has no static"),
        ("hiddenHasNext 1", "is not public static"),
        ("missing 1", "has no static"),
    ] {
        let line = format!("{site} dev.aim.server.test.Redirects.{to} why\n");
        let got = refused(&line);
        assert!(got.contains(error), "{line}: {got}");
    }
    let got = refused(
        "com.android.server.SystemServer.dumpHprof java.lang.StringBuilder.append dev.aim.server.test.Redirects.append 5 why\n",
    );
    assert!(got.contains("more than one method"), "{got}");
    let got = refused(
        "com.android.server.SystemServer.reportWtf java.lang.StringBuilder.<init> dev.aim.server.test.Redirects.add 1 why\n",
    );
    assert!(got.contains("constructor"), "{got}");
}
