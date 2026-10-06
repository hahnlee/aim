//! Native collector expectations for execution of the pinned original collector.
use aim_binder_host::parcel::Parcel;
use aim_services::package::{
    domain_verification::collector::{self, Kind, Policy},
    intent_filter::{
        ACTION_VIEW, AuthorityEntry, CATEGORY_BROWSABLE, IntentFilter, ParsedIntentInfo,
    },
    pkg::{Activity, AndroidPackage},
};
use std::{fs, path::Path};

fn filter(auto: bool, default: bool, schemes: &[&str], hosts: &[&str]) -> IntentFilter {
    let mut f = IntentFilter {
        auto_verify: auto,
        ..Default::default()
    };
    f.add_action(ACTION_VIEW);
    f.add_category(CATEGORY_BROWSABLE);
    if default {
        f.add_category("android.intent.category.DEFAULT");
    }
    for scheme in schemes {
        f.add_data_scheme(scheme);
    }
    for host in hosts {
        f.add_data_authority(host, None);
    }
    f
}
fn package(filters: Vec<IntentFilter>) -> AndroidPackage {
    let mut pkg = AndroidPackage {
        feature_flag_state: Some(vec![]),
        package_name: "fixture.domains".into(),
        target_sdk_version: 36,
        ..Default::default()
    };
    let mut activity = Activity {
        max_aspect_ratio: Some(0.0),
        min_aspect_ratio: Some(0.0),
        ..Default::default()
    };
    activity.main.component.name = "fixture.domains.Main".into();
    activity.main.component.package_name = pkg.package_name.clone();
    activity.main.component.intents = filters
        .into_iter()
        .map(|filter| ParsedIntentInfo {
            filter,
            ..Default::default()
        })
        .collect();
    pkg.activities.push(activity);
    pkg
}
fn configuration(directory: &Path) {
    use aim_services::package::system_config::SystemConfig;
    let root = directory.join("domain-config");
    let entries = [
        (
            "system/etc/sysconfig",
            "<app-link package='system'/><app-link package='Aa'/><app-link package='BB'/><app-link package=''/><app-link/><app-link package='Aa'/><app-link package=' '/>",
        ),
        (
            "system/etc/permissions",
            "<app-link package='system.permissions'/>",
        ),
        ("vendor/etc/sysconfig", "<app-link package='vendor'/>"),
        (
            "vendor/etc/sysconfig/sku_demo",
            "<app-link package='vendor.sku'/>",
        ),
        ("odm/etc/permissions", "<app-link package='odm'/>"),
        (
            "odm/etc/permissions/sku_demo",
            "<app-link package='odm.sku'/>",
        ),
        ("oem/etc/sysconfig", "<app-link package='oem.forbidden'/>"),
        (
            "product/etc/sysconfig",
            "<app-link package='product'/><app-link package='BB'/>",
        ),
        (
            "product/etc/sysconfig/sku_demo",
            "<app-link package='product.sku'/>",
        ),
        (
            "system_ext/etc/permissions",
            "<app-link package='extension'/>",
        ),
        (
            "apex/com.fixture/etc/permissions",
            "<app-link package='apex.forbidden'/>",
        ),
    ];
    for (path, xml) in entries {
        fs::create_dir_all(root.join(path)).unwrap();
        fs::write(
            root.join(path).join("config.xml"),
            format!("<config>{xml}</config>"),
        )
        .unwrap();
    }
    let mut expected = Parcel::new();
    for sdk in [27, 28, 36] {
        let config = SystemConfig::read(&root, &|name| match name {
            "ro.product.first_api_level" => Some(sdk.to_string()),
            "ro.boot.product.vendor.sku"
            | "ro.boot.product.hardware.sku"
            | "ro.boot.hardware.sku" => Some("demo".into()),
            _ => None,
        });
        assert!(config.linked_apps.contains(&"product.sku".into()));
        assert_eq!(config.linked_apps.contains(&"vendor.sku".into()), sdk <= 27);
        assert!(
            !config
                .linked_apps
                .iter()
                .any(|name| name.ends_with("forbidden"))
        );
        expected.write_i32(config.linked_apps.len() as i32);
        for name in config.linked_apps {
            expected.write_string16(Some(&name));
        }
    }
    fs::write(directory.join("domain-config.input"), expected.data()).unwrap();
}

fn signatures(directory: &Path) {
    use aim_services::package::domain_verification::owner::signature_hash;
    // Signature accepts bytes without requiring X.509 parsing for this digest API.
    let a = vec![0, 1, 127, 128, 255];
    let b = b"second signer".to_vec();
    let cases = [
        vec![],
        vec![vec![]],
        vec![a.clone()],
        vec![a.clone(), b.clone()],
        vec![b, a.clone()],
        vec![a.clone(), a],
    ];
    let mut expected = Parcel::new();
    expected.write_i32(cases.len() as i32);
    for signatures in cases {
        expected.write_i32(signatures.len() as i32);
        for signature in &signatures {
            expected.write_string16(Some(
                &signature
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<String>(),
            ));
        }
        expected.write_string16(Some(&signature_hash(&signatures)));
    }
    fs::write(directory.join("domain-signatures.input"), expected.data()).unwrap();
}

pub fn export(directory: &Path) {
    configuration(directory);
    signatures(directory);
    attachment_inputs(directory);
    let mut cases = vec![
        package(vec![
            filter(true, false, &["https"], &["seed.example"]),
            filter(false, false, &["https", "custom"], &["legacy.example"]),
            filter(
                true,
                true,
                &["https"],
                &["*.modern.example", "invalid", "modern.example"],
            ),
        ]),
        package(vec![filter(false, false, &["https"], &["linked.example"])]),
        package(vec![filter(
            true,
            true,
            &["https", "custom"],
            &["mixed.example"],
        )]),
        package(vec![filter(
            true,
            true,
            &["https"],
            &[
                "𐀀.example",
                "δοκιμή.example",
                "*.wild.example",
                "*invalid.example",
                "",
                "192.168.1.1",
                "UPPER.EXAMPLE",
                "xn--bcher-kva.example",
                "a..example",
            ],
        )]),
        package(vec![]),
    ];
    // Cross the real one-MiB boundary within one filter, then add another filter.
    let mut boundary = filter(true, true, &["https"], &[]);
    boundary.authorities = Some(
        (0..9000)
            .map(|i| AuthorityEntry::new(&format!("{}{:04}.example", "a".repeat(58), i), None))
            .collect(),
    );
    cases.push(package(vec![
        boundary,
        filter(true, true, &["https"], &["after.example"]),
    ]));
    // Duplicate authorities are legal in the original parcel and count toward its bound.
    let mut duplicates = filter(true, true, &["https"], &[]);
    duplicates.authorities = Some(
        (0..40000)
            .map(|_| AuthorityEntry::new("repeat.example", None))
            .chain([AuthorityEntry::new("tail.example", None)])
            .collect(),
    );
    cases.push(package(vec![duplicates]));
    let mut expected = Parcel::new();
    expected.write_i32(cases.len() as i32);
    for (i, pkg) in cases.iter().enumerate() {
        fs::write(
            directory.join(format!("domain-collector-{i}.cache")),
            pkg.to_cache_entry().unwrap().bytes,
        )
        .unwrap();
        for restrict_domains in [false, true] {
            for linked_app in [false, true] {
                for kind in [Kind::Web, Kind::ValidAutoVerify, Kind::InvalidAutoVerify] {
                    let hosts = collector::collect(
                        pkg,
                        Policy {
                            restrict_domains,
                            linked_app,
                        },
                        kind,
                    );
                    expected.write_i32(hosts.len() as i32);
                    for host in hosts {
                        expected.write_string16(Some(&host));
                    }
                }
            }
        }
    }
    fs::write(directory.join("domain-collector.input"), expected.data()).unwrap();
}

fn attachment_inputs(directory: &Path) {
    let hosts: Vec<_> = (0..=8)
        .map(|i| format!("h{i}.example"))
        .chain(["h1024.example".into()])
        .collect();
    let refs: Vec<_> = hosts.iter().map(String::as_str).collect();
    let code = package(vec![filter(true, true, &["https"], &refs)]);
    fs::write(
        directory.join("domain-owner.cache"),
        code.to_cache_entry().unwrap().bytes,
    )
    .unwrap();
    for case in 0..4 {
        let section = if case < 2 { "active" } else { "restored" };
        let signature = if case == 3 {
            "mismatch".into()
        } else {
            aim_services::package::domain_verification::owner::signature_hash(&[])
        };
        let domains = hosts
            .iter()
            .enumerate()
            .map(|(i, host)| {
                let state = if i == 9 { 1024 } else { i };
                format!("<domain name='{host}' state='{state}'/>")
            })
            .collect::<String>();
        let xml = format!(
            "<domain-verifications><{section}><package-state packageName='fixture.domains' id='00000000-0000-0000-0000-00000000000a' hasAutoVerifyDomains='true' signature='{signature}'><state>{domains}<domain name='gone.example' state='1'/></state><user-states><user-state userId='10' allowLinkHandling='false'><enabled-hosts><host name='h1.example'/><host name='h3.example'/><host name='h6.example'/><host name='h1024.example'/><host name='gone.example'/></enabled-hosts></user-state></user-states></package-state></{section}></domain-verifications>"
        );
        fs::write(directory.join(format!("domain-owner-{case}.input")), xml).unwrap();
    }
}

pub fn verify_attachment(directory: &Path) {
    use aim_services::package::{
        domain_verification::{
            State,
            owner::{Input, Owner},
        },
        system_config::SystemConfig,
    };
    let code =
        AndroidPackage::read_cache_entry(&fs::read(directory.join("domain-owner.cache")).unwrap())
            .unwrap();
    for case in 0..4 {
        let mut saved = State::default();
        saved
            .read(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-owner-{case}.input"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let mut config = SystemConfig::default();
        if case == 1 {
            config.linked_apps.push(code.package_name.clone());
        }
        let mut owner = Owner::new(saved, Default::default());
        let args = |id| Input {
            id,
            name: &code.package_name,
            code: Some(&code),
            signatures: &[],
            system: case == 1,
            restrict_domains: true,
            pre_verified: None,
        };
        owner
            .add(args("00000000-0000-0000-0000-00000000000b"), &config)
            .unwrap();
        for stage in ["add", "migrate"] {
            if stage == "migrate" {
                owner
                    .migrate(
                        "00000000-0000-0000-0000-00000000000b",
                        Some(&code),
                        args("00000000-0000-0000-0000-00000000000c"),
                        &config,
                    )
                    .unwrap();
            }
            let root = aim_android_xml::read(
                &fs::read(directory.join(format!("domain-owner-{case}-{stage}.original"))).unwrap(),
            )
            .unwrap();
            let mut original = State::default();
            original
                .read(
                    root.children()
                        .find(|e| e.name == "domain-verifications")
                        .unwrap(),
                )
                .unwrap();
            let values = owner
                .queries(&code, true, &config, [0, 10])
                .unwrap()
                .unwrap();
            let mut expected = Parcel::new();
            expected.write_i32(i32::from(values.verification.is_some()));
            let write_states = |out: &mut Parcel, states: &[(String, i32)]| {
                out.write_i32(states.len() as i32);
                for (host, state) in states {
                    out.write_string16(Some(host));
                    out.write_i32(*state);
                }
            };
            if let Some((id, states)) = values.verification {
                expected.write_string16(Some(&id));
                write_states(&mut expected, &states);
            }
            for (_, (allowed, states)) in values.users {
                expected.write_i32(i32::from(allowed));
                write_states(&mut expected, &states);
            }
            assert_eq!(
                expected.data(),
                fs::read(directory.join(format!("domain-owner-{case}-{stage}.queries"))).unwrap(),
                "public domain queries case={case} stage={stage}"
            );
            assert_eq!(
                owner.persisted(),
                original,
                "attached domain state case={case} stage={stage}"
            );
        }
    }
}
