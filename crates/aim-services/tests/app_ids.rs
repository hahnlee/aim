//! Compare UID-slot operations with the image's original AppIdSettingMap.
use aim_services::package::owner::app_ids::{AppIds, Error, Owner};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

mod common {
    pub mod java;
    pub mod runtime;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn allocation_matches_the_original_runtime() {
    let dir = std::env::temp_dir().join(format!("aim-ids-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
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
        .args(sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/AppIdsOracle.java"),
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
        .arg(classes.join("com/android/server/pm/AppIdsOracle.class")));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/app-ids.dex"),
    )
    .unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
    ]));

    let a = Owner::SharedUser("a".into());
    let b = Owner::SharedUser("b".into());
    let mut ids = AppIds::default();
    let mut lines = vec![ids.register_existing(10002, a.clone()).is_ok().to_string()];
    for _ in 0..3 {
        lines.push(ids.acquire(b.clone()).unwrap().to_string());
    }
    ids.remove(10001);
    lines.push(ids.acquire(b.clone()).unwrap().to_string());
    ids.replace(10001, a.clone()).unwrap();
    lines.push((ids.get(10001) == Some(&a)).to_string());
    ids.replace(1000, a.clone()).unwrap();
    lines.push((ids.get(1000) == Some(&a)).to_string());
    ids.remove(10010);
    lines.push(ids.acquire(b.clone()).unwrap().to_string());
    let mut restart = AppIds::default();
    restart.register_existing(10002, a.clone()).unwrap();
    lines.push(restart.acquire(b.clone()).unwrap().to_string());
    let mut full = AppIds::default();
    let mut last = -1;
    for _ in 0..10000 {
        last = full.acquire(a.clone()).unwrap();
    }
    lines.push(last.to_string());
    assert_eq!(full.acquire(b.clone()), Err(Error::Exhausted));
    lines.push("-1".into());
    full.remove(19999);
    assert_eq!(full.acquire(b), Err(Error::Exhausted));
    lines.push("-1".into());
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        lines.join("\n") + "\n"
    );

    use aim_services::package::pkg::*;
    let component = Component {
        name: "new.Class".into(),
        package_name: "new".into(),
        ..Default::default()
    };
    let main = MainComponent {
        component: component.clone(),
        process_name: Some("new:process".into()),
        ..Default::default()
    };
    let mut renamed = AndroidPackage {
        package_name: "new".into(),
        manifest_package_name: Some("new".into()),
        activities: vec![Activity {
            main: main.clone(),
            ..Default::default()
        }],
        receivers: vec![Activity {
            main: main.clone(),
            ..Default::default()
        }],
        services: vec![Service {
            main: main.clone(),
            ..Default::default()
        }],
        providers: vec![Provider {
            main,
            ..Default::default()
        }],
        permissions: vec![Permission {
            component: component.clone(),
            ..Default::default()
        }],
        permission_groups: vec![PermissionGroup {
            component: component.clone(),
            ..Default::default()
        }],
        instrumentations: vec![Instrumentation {
            component,
            ..Default::default()
        }],
        ..Default::default()
    };
    aim_services::package::scan::Identity {
        manifest_name: "new".into(),
        internal_name: "old".into(),
        real_name: Some("new".into()),
    }
    .apply(&mut renamed);
    let mut expected = format!(
        "{} {}\n",
        renamed.package_name,
        renamed.manifest_package_name.as_ref().unwrap()
    );
    for (component, process) in [
        (
            &renamed.activities[0].main.component,
            renamed.activities[0].main.process_name.as_deref(),
        ),
        (
            &renamed.receivers[0].main.component,
            renamed.receivers[0].main.process_name.as_deref(),
        ),
        (
            &renamed.services[0].main.component,
            renamed.services[0].main.process_name.as_deref(),
        ),
        (
            &renamed.providers[0].main.component,
            renamed.providers[0].main.process_name.as_deref(),
        ),
        (&renamed.permissions[0].component, None),
        (&renamed.permission_groups[0].component, None),
        (&renamed.instrumentations[0].component, None),
    ] {
        expected.push_str(&format!(
            "{} {} {} {} {}\n",
            component.package_name,
            component.name,
            process.unwrap_or("-"),
            component.package_name,
            component.name
        ));
    }
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "identity",
    ]));
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected);

    use aim_services::package::{
        settings::Signatures,
        sign::{History, JoinType},
    };
    let cases: Vec<_> = [
        (vec![], None),
        (vec![1], None),
        (vec![2], None),
        (vec![2], Some(vec![(1, 1), (2, 0)])),
        (vec![2], Some(vec![(1, 0), (2, 0)])),
        (vec![3], Some(vec![(1, 3), (2, 8), (3, 0)])),
        (vec![1, 2], None),
        (vec![2, 1], None),
        (vec![1, 3], None),
    ]
    .into_iter()
    .map(|(current, past)| Signatures {
        signatures: current.into_iter().map(|c| vec![c]).collect(),
        past_signatures: past.map(|p| p.into_iter().map(|(c, flags)| (vec![c], flags)).collect()),
        ..Default::default()
    })
    .collect();
    let mut expected = String::new();
    for (i, candidate) in cases.iter().enumerate() {
        for (j, old) in cases.iter().enumerate() {
            let candidate = History::saved(candidate);
            let old = History::saved(old);
            for flags in [0, 1, 2, 3, 8, 31] {
                expected.push_str(&format!(
                    "{i} {j} {flags} {} {} {} {} {}\n",
                    candidate.check_capability(&old, flags),
                    candidate.has_ancestor(&old),
                    candidate.has_ancestor_or_self(&old),
                    candidate.allows_update_from(&old, false),
                    candidate.allows_update_from(&old, true)
                ));
            }
        }
    }
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "trust",
    ]));
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected);

    let mut expected = String::new();
    for (i, candidate) in cases.iter().enumerate() {
        for (j, group) in cases.iter().enumerate() {
            for (k, member) in cases.iter().enumerate() {
                for (kind, mode) in [JoinType::Install, JoinType::Update, JoinType::System]
                    .into_iter()
                    .enumerate()
                {
                    let allowed = History::saved(candidate).can_join_shared_user(
                        &History::saved(group),
                        mode,
                        &[History::saved(member)],
                    );
                    expected.push_str(&format!("{i} {j} {k} {kind} {allowed}\n"));
                }
            }
        }
    }
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "join",
    ]));
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected);

    let ancestry: Vec<_> = [
        (vec![], None),
        (vec![1], None),
        (vec![2], None),
        (vec![2], Some(vec![(1, 3), (2, 0)])),
        (vec![3], Some(vec![(1, 0), (2, 2), (3, 0)])),
        (vec![3], Some(vec![(2, 8), (3, 0)])),
        (vec![3], Some(vec![(4, 3), (2, 2), (3, 0)])),
        (vec![4], Some(vec![(1, 3), (2, 2), (4, 0)])),
        (vec![1, 2], None),
        (vec![2, 1], None),
        (vec![1, 3], None),
        (vec![3], Some(vec![(3, 0)])),
    ]
    .into_iter()
    .map(|(current, past)| Signatures {
        signatures: current.into_iter().map(|c| vec![c]).collect(),
        past_signatures: past.map(|p| p.into_iter().map(|(c, f)| (vec![c], f)).collect()),
        ..Default::default()
    })
    .collect();
    let mut expected = String::new();
    for (i, candidate) in ancestry.iter().enumerate() {
        for (j, other) in ancestry.iter().enumerate() {
            expected.push_str(&format!(
                "{i} {j} {}\n",
                History::saved(candidate).has_common_ancestor(&History::saved(other))
            ));
        }
    }
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "ancestry",
    ]));
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected);

    // Original SystemConfig reads these disposable vendor/OEM fixtures
    // with zero partition permissions: the UID tag has no allow-bit gate.
    let root = boot.data.join("data/local/tmp/uid-image");
    let inputs = [
        (
            "vendor",
            r#"<permissions>
            <oem-defined-uid name="android.uid.Aa" uid="2900" />
            <oem-defined-uid name="android.uid.BB" uid="2900" />
            <oem-defined-uid name="android.uid.update" uid="2901" />
            <oem-defined-uid name="android.uid.signed" uid="+2902" />
            <oem-defined-uid name="android.uid.unicode" uid="٢٩٠٣" />
            <oem-defined-uid name="android.uid.fullwidth" uid="２９０４" />
            <oem-defined-uid name="android.uid.min" uid="-2147483648" />
            <oem-defined-uid name="android.uid.max" uid="2147483647" />
            <oem-defined-uid name="android.uid.space" uid=" 2905" />
            <oem-defined-uid name="android.uid.overflow" uid="2147483648" />
            <oem-defined-uid name="" uid="2906" />
            <oem-defined-uid name="android.uid.empty" uid="" />
            <oem-defined-uid name="android.uid.missing" />
            </permissions>"#,
        ),
        (
            "oem",
            r#"<config><oem-defined-uid name="android.uid.update" uid="2999" />
            <oem-defined-uid name="android.uid.invalid" uid="-1" /></config>"#,
        ),
    ];
    for (partition, xml) in inputs {
        let dir = root.join(partition).join("etc/permissions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("uids.xml"), xml).unwrap();
    }
    let config = aim_services::package::system_config::SystemConfig::read(&root, &|_| None);
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "oem",
        "/data/local/tmp/uid-image/vendor/etc/permissions",
        "/data/local/tmp/uid-image/oem/etc/permissions",
    ]));
    let expected = config
        .oem_defined_uids
        .iter()
        .map(|(name, id)| format!("{name} {id}\n"))
        .collect::<String>();
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected);
    assert_eq!(config.oem_defined_uids.len(), 9);
    assert_eq!(config.rejected_oem_uids.len(), 5);
    assert!(
        config
            .rejected_oem_uids
            .iter()
            .all(|r| r.path == "vendor/etc/permissions/uids.xml")
    );
    let bootstrap = aim_services::package::owner::shared_users::Bootstrap::new(&config);
    assert_eq!(bootstrap.shared_users.len(), 14);
    assert_eq!(bootstrap.rejected.len(), 4);

    let state = aim_services::package::State::read(&boot.data.join("data"), &[0])
        .unwrap()
        .unwrap();
    let before = state.clone();
    assert!(!state.settings.packages.is_empty());
    assert!(!state.settings.shared_users.is_empty());
    let restored = AppIds::restore(&state.settings).unwrap();
    for group in &state.settings.shared_users {
        assert_eq!(
            restored.get(group.app_id),
            Some(&Owner::SharedUser(group.name.clone()))
        );
    }
    let seeded = aim_services::package::owner::shared_users::Bootstrap::new(&Default::default());
    // PMS prunes unused seeded groups after scanning; the bootstrap
    // input must not recreate them in a restored published snapshot.
    let mut retained = 0;
    for (name, group) in seeded.shared_users {
        if let Some(saved) = state.settings.shared_users.iter().find(|g| g.name == name) {
            assert_eq!(saved.app_id, group.app_id);
            assert_eq!(restored.get(group.app_id), Some(&Owner::SharedUser(name)));
            retained += 1;
        }
    }
    assert!(retained > 0);
    let mut merged = aim_services::package::owner::shared_users::Bootstrap::restore(
        &Default::default(),
        &state.settings,
    )
    .unwrap();
    merged.prune_unused(&state.settings);
    assert_eq!(merged.shared_users.len(), state.settings.shared_users.len());
    for saved in &state.settings.shared_users {
        let group = &merged.shared_users[&saved.name];
        assert_eq!(group.app_id, saved.app_id);
        assert_eq!(group.signatures, saved.signatures);
        assert_eq!(merged.ids.get(saved.app_id), restored.get(saved.app_id));
    }
    for saved in &state.settings.packages {
        assert_eq!(merged.ids.get(saved.app_id), restored.get(saved.app_id));
    }
    let image = aim_paths::derived_image();
    let mut platform =
        aim_services::package::parse::Platform::load(&image, Default::default()).unwrap();
    let density =
        String::from_utf8(run(boot.command().args(["shell", "wm", "density"])).stdout).unwrap();
    let density = density
        .lines()
        .filter_map(|line| {
            line.strip_prefix("Physical density: ")
                .or_else(|| line.strip_prefix("Override density: "))
                .map(|n| n.parse().unwrap())
        })
        .last()
        .expect("original display density");
    platform.density_dpi = Some(density);
    let data_files = boot.data.join("data");
    let apks = aim_services::package::write::Apks {
        files: Box::new(move |path| {
            Some(if let Some(relative) = path.strip_prefix("/data/") {
                data_files.join(relative)
            } else {
                image.join(path.trim_start_matches('/'))
            })
        }),
        platform,
    };
    let inputs = aim_services::package::scan::Inputs::load(&state, &apks).unwrap();
    assert_eq!(inputs.active.len(), state.settings.packages.len());
    for (name, record) in &inputs.active {
        assert_eq!(&record.identity.internal_name, name);
        assert_eq!(&record.parsed.package_name, name);
        let previous = record
            .settings
            .signatures
            .as_ref()
            .expect("saved signing details");
        assert!(
            History::verified(&record.signing).allows_update_from(&History::saved(previous), false),
            "normal signing gate: {name}"
        );
        if record.settings.shared_user {
            let group = state
                .settings
                .shared_users
                .iter()
                .find(|g| g.app_id == record.settings.app_id)
                .unwrap();
            let members: Vec<_> = inputs
                .active
                .values()
                .filter(|r| r.settings.shared_user && r.settings.app_id == group.app_id)
                .map(|r| History::verified(&r.signing))
                .collect();
            let details = group
                .signatures
                .as_ref()
                .expect("saved shared signing details");
            assert!(
                History::verified(&record.signing).can_join_shared_user(
                    &History::saved(details),
                    JoinType::Update,
                    &members,
                ),
                "shared signing gate: {name}"
            );
        }
        assert_eq!(
            merged.ids.get(record.settings.app_id),
            restored.get(record.settings.app_id)
        );
    }
    let first_api = String::from_utf8(
        run(boot
            .command()
            .args(["shell", "getprop", "ro.product.first_api_level"]))
        .stdout,
    )
    .unwrap()
    .trim()
    .parse()
    .unwrap_or(0);
    let mut signing_scan = aim_services::package::scan::SigningScan::new(
        &Default::default(),
        &state.settings,
        first_api,
    )
    .unwrap();
    // This checks the supplied persisted-record order. The complete
    // image/data scan chooses its own order before invoking this owner.
    for package in &state.settings.packages {
        let outcome = signing_scan.apply(&inputs.active[&package.name]).unwrap();
        assert!(
            outcome.system_signature_mismatch.is_none(),
            "unexpected OTA replacement: {}",
            package.name
        );
    }
    for (saved, committed) in state
        .settings
        .packages
        .iter()
        .zip(&signing_scan.settings.packages)
    {
        let mut expected = saved.clone();
        let keys = committed.signatures.as_ref().unwrap().public_keys.clone();
        assert!(keys.as_ref().is_some_and(|k| !k.is_empty()));
        expected.signatures.as_mut().unwrap().public_keys = keys;
        // This loader forces fresh verification. Original collectCertificatesLI
        // selects parsed-code signatures on that path; a boot cache hit instead
        // clones saved signing (the separate collection policy tracked in #918).
        expected.signatures.as_mut().unwrap().current_flags =
            inputs.active[&saved.name].signing.current_flags.clone();
        assert_eq!(
            &expected, committed,
            "signing scan changed persisted metadata: {}",
            saved.name
        );
    }
    assert_eq!(
        signing_scan.settings.shared_users,
        state.settings.shared_users
    );
    signing_scan.identities.prune_unused(&signing_scan.settings);
    for group in &state.settings.shared_users {
        assert_eq!(
            signing_scan.identities.ids.get(group.app_id),
            restored.get(group.app_id)
        );
    }
    assert_eq!(state, before);

    // Valid DER certificates exercise the original merge constructor,
    // which rebuilds its public-key set. These histories are relationship
    // fixtures, not claims that any synthesized lineage verified an APK.
    use aim_services::package::sign::{MergeRule, SigningDetails};
    use std::borrow::Cow;
    let certs: Vec<_> = inputs
        .active
        .values()
        .flat_map(|r| r.signing.signatures.iter())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .take(4)
        .collect();
    assert_eq!(certs.len(), 4);
    for (i, certificate) in certs.iter().enumerate() {
        fs::write(
            boot.data.join(format!("data/local/tmp/merge-cert-{i}.der")),
            certificate,
        )
        .unwrap();
    }
    let merged_cases: Vec<_> = [
        (vec![], None),
        (vec![1], None),
        (vec![2], None),
        (vec![2], Some(vec![(1, 3), (2, 0)])),
        (vec![3], Some(vec![(1, 0), (2, 2), (3, 0)])),
        (vec![3], Some(vec![(2, 8), (3, 0)])),
        (vec![3], Some(vec![(4, 3), (2, 2), (3, 0)])),
        (vec![4], Some(vec![(1, 3), (2, 2), (4, 0)])),
        (vec![1, 2], None),
        (vec![2, 1], None),
        (vec![1, 3], None),
        (vec![3], Some(vec![(3, 0)])),
        (vec![2], Some(vec![(1, 0), (2, 8)])),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (current, past))| {
        let saved = Signatures {
            current_flags: vec![i as i32 + 1; current.len()],
            scheme_version: if current.is_empty() {
                0
            } else {
                i as i32 % 4 + 1
            },
            signatures: current.into_iter().map(|c| certs[c - 1].clone()).collect(),
            past_signatures: past.map(|p| {
                p.into_iter()
                    .map(|(c, f)| (certs[c - 1].clone(), f))
                    .collect()
            }),
            ..Default::default()
        };
        (SigningDetails::from_saved(&saved).unwrap(), saved)
    })
    .collect();
    let describe = |details: &SigningDetails| {
        let current = details
            .signatures
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "{}:{}",
                    certs.iter().position(|c| c == s).unwrap() + 1,
                    details.current_flags.get(i).copied().unwrap_or(0)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let past = details
            .past_signing_certificates
            .as_ref()
            .map(|p| {
                p.iter()
                    .map(|(s, f)| format!("{}:{f}", certs.iter().position(|c| c == s).unwrap() + 1))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "-".into());
        format!(
            "{} {} {} {}",
            details.scheme_version,
            if current.is_empty() { "-" } else { &current },
            past,
            details.public_keys.len()
        )
    };
    for mode in ["merge", "group-merge"] {
        let mut expected = String::new();
        for (i, (a, saved)) in merged_cases.iter().enumerate() {
            for (j, (b, _)) in merged_cases.iter().enumerate() {
                if mode == "merge" {
                    for (r, rule) in [
                        MergeRule::SelfCapability,
                        MergeRule::OtherCapability,
                        MergeRule::RestrictedCapability,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let merged = a.merge_lineage_with(b, rule).unwrap();
                        let unchanged = matches!(&merged, Cow::Borrowed(s) if std::ptr::eq(*s, a));
                        expected
                            .push_str(&format!("{i} {j} {r} {unchanged} {}\n", describe(&merged)));
                    }
                } else {
                    for (k, (member, _)) in merged_cases.iter().enumerate() {
                        let mut group = aim_services::package::owner::shared_users::SharedUser::new(
                            10001, 0, 0,
                        );
                        group.signatures = Some(saved.clone());
                        let changed = group
                            .merge_authorized_lineage(b, std::slice::from_ref(member))
                            .unwrap();
                        let result =
                            SigningDetails::from_saved(group.signatures.as_ref().unwrap()).unwrap();
                        expected
                            .push_str(&format!("{i} {j} {k} {changed} {}\n", describe(&result)));
                    }
                }
            }
        }
        let original = run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.pm.AppIdsOracle",
            mode,
            "/data/local/tmp/merge-cert-0.der",
            "/data/local/tmp/merge-cert-1.der",
            "/data/local/tmp/merge-cert-2.der",
            "/data/local/tmp/merge-cert-3.der",
        ]));
        assert_eq!(
            String::from_utf8(original.stdout).unwrap(),
            expected,
            "{mode}"
        );
    }
    for (name, group) in &mut merged.shared_users {
        let group_id = group.app_id;
        for (package, record) in inputs
            .active
            .iter()
            .filter(|(_, r)| r.settings.shared_user && r.settings.app_id == group_id)
        {
            let others: Vec<_> = inputs
                .active
                .iter()
                .filter(|(n, r)| {
                    *n != package && r.settings.shared_user && r.settings.app_id == group.app_id
                })
                .map(|(_, r)| r.signing.clone())
                .collect();
            assert!(
                !group
                    .merge_authorized_lineage(&record.signing, &others)
                    .unwrap(),
                "saved group changed: {name}"
            );
        }
        let saved = state
            .settings
            .shared_users
            .iter()
            .find(|g| &g.name == name)
            .unwrap();
        assert_eq!(group.signatures, saved.signatures);
    }

    let static_libraries: Vec<_> = inputs
        .active
        .values()
        .filter(|r| r.parsed.static_shared_library_name.is_some())
        .collect();
    assert!(!static_libraries.is_empty());
    for record in static_libraries {
        assert_ne!(record.identity.internal_name, record.identity.manifest_name);
        assert_eq!(
            record.identity.internal_name,
            format!(
                "{}_{}",
                record.identity.manifest_name, record.parsed.static_shared_lib_version
            )
        );
    }
    for package in &state.settings.packages {
        if package.shared_user {
            assert!(matches!(
                restored.get(package.app_id),
                Some(Owner::SharedUser(_))
            ));
        } else {
            assert_eq!(
                restored.get(package.app_id),
                Some(&Owner::Package(package.name.clone()))
            );
        }
    }
    assert_eq!(state, before);
    // Persist only into this test's own fixture; the original PMS keeps
    // sole ownership of its mounted data. Store does not write live data.
    let signature_data = data.0.join("signature-store");
    fs::create_dir_all(signature_data.join("system")).unwrap();
    fs::copy(
        boot.data.join("data/system/packages.xml"),
        signature_data.join("system/packages.xml"),
    )
    .unwrap();
    let mut store = aim_services::package::owner::Store::open(&signature_data, &[0])
        .unwrap()
        .unwrap();
    let mut desired = store.state().settings.clone();
    for package in &mut desired.packages {
        let verified = signing_scan
            .settings
            .packages
            .iter()
            .find(|p| p.name == package.name)
            .unwrap();
        assert_eq!(package.app_id, verified.app_id);
        package.signatures = verified.signatures.clone();
    }
    for group in &mut desired.shared_users {
        let verified = signing_scan
            .settings
            .shared_users
            .iter()
            .find(|g| g.name == group.name)
            .unwrap();
        assert_eq!(group.app_id, verified.app_id);
        group.signatures = verified.signatures.clone();
    }
    store.commit_signatures(&desired).unwrap();
    let persisted = aim_services::package::State::read(&signature_data, &[0])
        .unwrap()
        .unwrap();
    assert_eq!(store.state(), &persisted);
    // Current flags are not explicit writeXml fields. Compare their restored
    // table-derived values separately against the actual original reader.
    let guest_xml = boot.data.join("data/local/tmp/appids-signatures.xml");
    fs::copy(signature_data.join("system/packages.xml"), &guest_xml).unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "read-store-signatures",
        "/data/local/tmp/appids-signatures.xml",
    ]));
    let mut actual: Vec<_> = String::from_utf8(original.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let flags = |s: &Signatures| {
        (0..s.signatures.len())
            .map(|i| s.current_flags.get(i).copied().unwrap_or(0).to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let public_keys = |s: &Signatures| {
        use sha2::{Digest, Sha256};
        s.public_keys
            .as_ref()
            .map(|keys| {
                keys.iter()
                    .map(|key| {
                        let key = key.as_ref().unwrap();
                        let hash: String = Sha256::digest(&key.bytes)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        format!("{}:{hash}", key.class)
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "null".into())
    };
    let mut expected = Vec::new();
    for p in &persisted.settings.packages {
        if let Some(s) = &p.signatures {
            expected.push(format!(
                "package:{}:sigs:{}:{}",
                p.name,
                flags(s),
                public_keys(s)
            ));
        }
        if let Some(s) = &p.install_source.initiating_package_signatures {
            expected.push(format!(
                "package:{}:install-initiator-sigs:{}:{}",
                p.name,
                flags(s),
                public_keys(s)
            ));
        }
    }
    for g in &persisted.settings.shared_users {
        if let Some(s) = &g.signatures {
            expected.push(format!(
                "shared-user:{}:sigs:{}:{}",
                g.name,
                flags(s),
                public_keys(s)
            ));
        }
    }
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "original XML restored current flags and public keys"
    );
    let mut restored = persisted.settings.clone();
    for package in &mut desired.packages {
        if let Some(signatures) = &mut package.signatures {
            signatures.public_keys = None;
            signatures.current_flags.clear();
        }
    }
    for group in &mut desired.shared_users {
        if let Some(signatures) = &mut group.signatures {
            signatures.public_keys = None;
            signatures.current_flags.clear();
        }
    }
    for p in &mut restored.packages {
        if let Some(s) = &mut p.signatures {
            s.current_flags.clear();
            s.public_keys = None;
        }
    }
    for g in &mut restored.shared_users {
        if let Some(s) = &mut g.signatures {
            s.current_flags.clear();
            s.public_keys = None;
        }
    }
    assert_eq!(restored, desired);
    let path = signature_data.join("system/packages.xml");
    assert_eq!(
        fs::read(&path).unwrap(),
        fs::read(path.with_file_name("packages.xml.reservecopy")).unwrap()
    );

    let rewritten = boot.data.join("data/local/tmp/native-signatures.xml");
    fs::copy(&path, &rewritten).unwrap();
    let root = aim_android_xml::read(&fs::read(&path).unwrap()).unwrap();
    let mut expected = String::new();
    let hex = |b: &[u8]| b.iter().map(|v| format!("{v:02x}")).collect::<String>();
    for node in root.children() {
        let signatures = match node.name.as_str() {
            "package" | "updated-package" => {
                let packages = if node.name == "package" {
                    &desired.packages
                } else {
                    &desired.disabled_system_packages
                };
                &packages
                    .iter()
                    .find(|p| p.name == node.string("name").unwrap())
                    .unwrap()
                    .signatures
            }
            "shared-user" => {
                &desired
                    .shared_users
                    .iter()
                    .find(|g| g.name == node.string("name").unwrap())
                    .unwrap()
                    .signatures
            }
            _ => continue,
        };
        let Some(signatures) = signatures else {
            continue;
        };
        let current = signatures
            .signatures
            .iter()
            .map(|c| hex(c))
            .collect::<Vec<_>>()
            .join(",");
        let past = signatures
            .past_signatures
            .as_ref()
            .map(|p| {
                p.iter()
                    .map(|(c, f)| format!("{}:{f}", hex(c)))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "-".into());
        let key_count = SigningDetails::from_saved(signatures)
            .unwrap()
            .public_keys
            .len();
        expected.push_str(&format!(
            "{} {} {} {} {} {key_count}\n",
            node.name,
            node.string("name").unwrap(),
            signatures.scheme_version,
            current,
            past
        ));
    }
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "read-signatures",
        "/data/local/tmp/native-signatures.xml",
    ]));
    let actual = String::from_utf8(original.stdout).unwrap();
    assert_eq!(actual.lines().count(), expected.lines().count());
    for (line, (actual, expected)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(actual, expected, "original signature reader row {line}");
    }
    println!(
        "original reader accepted {} native signature owners",
        actual.lines().count()
    );

    println!(
        "restored {} active packages and {} shared UID groups",
        state.settings.packages.len(),
        state.settings.shared_users.len()
    );
}
