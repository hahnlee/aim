//! Compare native streams byte for byte, then deserialize them on the
//! original Android runtime. A disposable boot owns every writable path.
use aim_services::package::sign::serialize_public_keys;
use aim_services::package::{info, sign};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

mod common {
    pub mod java;
    pub mod runtime;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};

impl Boot {
    fn oracle(&self, mode: &str) -> String {
        String::from_utf8(
            run(self.command().args([
                "shell",
                "/apex/com.android.art/bin/dalvikvm64",
                "-cp",
                "/data/local/tmp/key-serialization/keys.dex",
                "PublicKeySerialization",
                mode,
                "/data/local/tmp/key-serialization",
            ]))
            .stdout,
        )
        .unwrap()
    }
}

fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn public_keys_match_and_deserialize_on_the_original_runtime() {
    let dir = std::env::temp_dir().join(format!("ky-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let dex = data.0.join("dex");
    fs::create_dir(&classes).unwrap();
    fs::create_dir(&dex).unwrap();
    let stubs = data.0.join("stubs");
    fs::create_dir(&stubs).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PublicKeySerialization.java"),
        )
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/SigningParcel.java")));
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
        .arg(classes.join("PublicKeySerialization.class"))
        .arg(classes.join("SigningParcel.class")));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let inputs = data.0.join("inputs");
    fs::create_dir(&inputs).unwrap();
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(&classes)
        .args(["PublicKeySerialization", "generate"])
        .arg(&inputs));
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("g"));
    run(boot.start_command().args(["start", "--windows"]));
    boot.wait_ready(Duration::from_secs(300)).expect("key_serialization oracle boot readiness");
    let guest = boot.data.join("data/local/tmp/key-serialization");
    fs::create_dir(&guest).unwrap();
    fs::copy(dex.join("classes.dex"), guest.join("keys.dex")).unwrap();
    for entry in fs::read_dir(&inputs).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), guest.join(entry.file_name())).unwrap();
    }
    let output = boot.oracle("write");
    let mut keys = Vec::new();
    let mut ordered = Vec::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 4, "{line}");
        let spki = fs::read(inputs.join(format!("{}.spki", fields[0]))).unwrap();
        let key = serialize_public_keys(&[spki.clone()]).unwrap().remove(0);
        assert_eq!(key.class, fields[1]);
        assert_eq!(key.bytes, unhex(fields[3]), "{}", fields[0]);
        let original = aim_services::package::pkg::Serialized {
            class: fields[1].into(),
            bytes: unhex(fields[3]),
        };
        assert_eq!(
            sign::deserialize_public_key(&original).unwrap(),
            spki,
            "decode original {} stream",
            fields[0]
        );
        fs::write(guest.join(format!("{}.native", fields[0])), &key.bytes).unwrap();
        ordered.push((fields[2].parse::<i32>().unwrap(), key));
        keys.push(spki);
    }
    assert_eq!(keys.len(), 6);
    ordered.sort_by_key(|(hash, _)| *hash);
    assert_eq!(
        serialize_public_keys(&keys).unwrap(),
        ordered.into_iter().map(|(_, key)| key).collect::<Vec<_>>()
    );
    keys.push(keys[0].clone());
    assert_eq!(serialize_public_keys(&keys).unwrap().len(), 6);
    assert_eq!(boot.oracle("read"), output);

    const GSF: &str = "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk";
    let source =
        android_image_extract::source::FileSource::open(&aim_paths::original_image().join(GSF))
            .unwrap();
    let details = sign::verify(
        &sign::Apk {
            path: GSF,
            data: &source,
            v4: None,
        },
        2,
        true,
        &sign::Build {
            sdk_int: 36,
            release: true,
            always_load_past_certs_v4: true,
        },
    )
    .unwrap();
    let public_keys = details.serialized_public_keys().unwrap();
    let signing = info::SigningInfo {
        scheme_version: details.scheme_version,
        signatures: details.signatures,
        public_keys,
        past_signing_certificates: details
            .past_signing_certificates
            .map(|certs| certs.into_iter().map(|(cert, _)| cert).collect()),
    };
    let mut parcel = aim_binder_host::parcel::Parcel::new();
    info::write_signing_details(&mut parcel, Some(&signing));
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/key-serialization/keys.dex",
        "/system/bin",
        "SigningParcel",
        "com.google.android.gsf",
    ]));
    let original = unhex(String::from_utf8(original.stdout).unwrap().trim());
    assert_eq!(
        parcel.data(),
        original,
        "native SigningInfo must match the original PMS reply"
    );
}
