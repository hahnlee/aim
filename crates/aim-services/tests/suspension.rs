//! Compare the native XML owner with the pinned original SuspendDialogInfo.
use aim_android_xml::{Element, Value};
use aim_services::package::restrictions::dialog::DialogInfo;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn suspension_dialogs_match_original_xml_owner() {
    let data = Data(std::env::temp_dir().join(format!("aim-suspend-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
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
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/SuspensionDialogOracle.java"),
        ));
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
        .arg(classes.join("SuspensionDialogOracle.class")));
    common::java::check_linkage(&dex.join("classes.dex"), &[]).unwrap();
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let result = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let directory = boot.data.join("data/local/tmp/suspension-dialogs");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    let mut inputs = vec![aim_android_xml::read(b"<dialog-info/>").unwrap()];
    for field in [
        "iconResId",
        "titleResId",
        "buttonTextResId",
        "dialogMessageResId",
        "buttonAction",
    ] {
        for value in [
            Value::Int(0),
            Value::Int(-1),
            Value::Int(0x7f010001),
            Value::Int(0x81010001u32 as i32),
            Value::Int(0x7f000001),
            Value::Int(1),
            Value::Int(2),
            Value::String("bad".into()),
            Value::Long(1),
            Value::Null,
        ] {
            let mut e = aim_android_xml::read(b"<dialog-info iconResId='2130771969' title='title' buttonText='button' dialogMessage='message' buttonAction='1'/>").unwrap();
            e.attrs.retain(|(name, _)| name != field);
            e.attrs.push((field.into(), value));
            inputs.push(e);
        }
    }
    for field in ["title", "buttonText", "dialogMessage"] {
        for value in [
            Value::String(String::new()),
            Value::Null,
            Value::Int(17),
            Value::String("α & message".into()),
        ] {
            let mut e = aim_android_xml::read(b"<dialog-info iconResId='2130771969' title='title' buttonText='button' dialogMessage='message' buttonAction='1'/>").unwrap();
            e.attrs.retain(|(name, _)| name != field);
            e.attrs.push((field.into(), value));
            inputs.push(e);
        }
    }
    let mut expected = Vec::new();
    for e in &inputs {
        let index = expected.len();
        fs::write(
            directory.join(format!("{index}.xml")),
            aim_android_xml::abx::write(e).unwrap(),
        )
        .unwrap();
        expected.push(DialogInfo::restore(e).save("dialog-info"));
    }
    // Text parser conversions are checked separately from the ABX typed values.
    for xml in [
        "<dialog-info title='text' buttonAction='1'/>",
        "<dialog-info title='' dialogMessage='lost'/>",
        "<dialog-info iconResId='bad' titleResId='-2130640895' title='ignored' buttonAction='bad'/>",
    ] {
        let index = expected.len();
        fs::write(directory.join(format!("{index}.xml")), xml).unwrap();
        expected.push(
            DialogInfo::restore(&aim_android_xml::read(xml.as_bytes()).unwrap())
                .save("dialog-info"),
        );
    }
    let count = expected.len().to_string();
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/suspension-dialogs/oracle.dex",
        "/system/bin",
        "SuspensionDialogOracle",
        "/data/local/tmp/suspension-dialogs",
        &count,
    ]));
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!("DIALOGS {count}\n")
    );
    for (index, native) in expected.into_iter().enumerate() {
        let original: Element =
            aim_android_xml::read(&fs::read(directory.join(format!("{index}.original"))).unwrap())
                .unwrap();
        assert_eq!(native, original, "case {index}");
    }
}
