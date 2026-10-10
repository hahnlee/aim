//! Verify the inactive C UserManager redirect fixture against original services.jar.
use std::{fs, process::Command};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Data, run};

fn user_manager_calls(jar: &[u8], owner: &str) -> usize {
    use aim_android_image::dex::{self, Dex};
    let mut count = 0;
    let mut at = 0;
    while jar[at..at + 4] == [0x50, 0x4b, 0x03, 0x04] {
        let short = |offset| u16::from_le_bytes(jar[offset..offset + 2].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(jar[at + 18..at + 22].try_into().unwrap()) as usize;
        let name_length = short(at + 26);
        let start = at + 30 + name_length + short(at + 28);
        let name = std::str::from_utf8(&jar[at + 30..at + 30 + name_length]).unwrap();
        if name.ends_with(".dex") {
            assert_eq!(short(at + 8), 0, "original dex entry must be stored");
            let data = &jar[start..start + size];
            let dex = Dex::parse(data).unwrap();
            for class in &dex.classes {
                if class.descriptor != "Lcom/android/server/pm/UserManagerService;"
                    && !class.descriptor.starts_with("Lcom/android/server/pm/UserManagerService$") {
                    continue;
                }
                let names = dex.members(class).unwrap().1.into_iter()
                    .map(|method| dex.method(method).unwrap().1)
                    .collect::<std::collections::BTreeSet<_>>();
                for name in names {
                    for code in dex.methods_named(class, &name).unwrap() {
                        let instructions = dex::units(data, &code).unwrap();
                        let mut pc = 0;
                        while pc < instructions.len() {
                            if matches!(instructions[pc] & 0xff, 0x6e..=0x72 | 0x74..=0x78)
                                && dex.method(u32::from(instructions[pc + 1])).unwrap().0 == owner {
                                count += 1;
                            }
                            pc += dex::instruction_units(&instructions, pc).unwrap();
                        }
                    }
                }
            }
        }
        at = start + size;
    }
    count
}

#[test]
#[ignore = "requires original pinned services.jar, JDK and d8; inactive redirect fixture"]
fn native_user_manager_redirects_match_all_original_call_sites() {
    let directory = std::env::temp_dir().join(format!("aim-umredirect-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let data = Data(directory);
    let repo = aim_paths::root();
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &repo.join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(repo.join("java/device-services/src/com/android/server/pm/NativeUserManagerBridge.java")));
    let mut class_files = Vec::new();
    let mut pending = vec![classes.clone()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "class") {
                class_files.push(path);
            }
        }
    }
    class_files.sort();
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
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
        .args(class_files));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let original = aim_paths::original_image_with("system/framework/services.jar").expect("original services.jar is required");
    let services = fs::read(original.join("system/framework/services.jar")).unwrap();
    let fixture = fs::read(dex.join("classes.dex")).unwrap();
    let manifest = include_str!("fixtures/native-user-manager-redirects");
    let redirects = aim_android_image::redirect::parse(manifest).unwrap();
    assert_eq!(redirects.iter().map(|entry| entry.calls).sum::<usize>(), 14);
    let patched = aim_android_image::redirect::redirect_jar(&services, &redirects, &[&fixture]).unwrap();
    assert_ne!(patched, services);
    assert_eq!(user_manager_calls(&services, "Lcom/android/server/pm/PackageManagerService;"), 14);
    assert_eq!(user_manager_calls(&patched, "Lcom/android/server/pm/PackageManagerService;"), 0);
    assert_eq!(user_manager_calls(&patched, "Lcom/android/server/pm/NativeUserManagerBridge;"), 14);
    fs::write(data.0.join("services.redirected.fixture.jar"), patched).unwrap();
    println!("all 14 original UM concrete PMS sites symbolically redirected; global image unchanged");
}
