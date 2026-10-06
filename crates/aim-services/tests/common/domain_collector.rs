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
    grouped_owners(directory, false);
    legacy_inputs(directory);
    persistence_defaults(directory);
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

pub fn grouped_owners(directory: &Path, verify: bool) {
    use aim_services::package::domain_verification::{
        State,
        owner::{ApprovalInput, Input, Owner},
    };
    let entries = [
        ("disabled", 0, 0),
        ("ask", 1, 20),
        ("always", 2, 10),
        ("selected", 3, 30),
        ("Aa", 4, 40),
        ("BB", 4, 40),
        ("a", 4, 0),
        ("A", 4, 0),
        ("İ", 4, 0),
        ("ı", 4, 0),
        ("instant", 5, 50),
        ("missing", 6, 0),
    ];
    let mut xml = String::from("<domain-verifications><active>");
    let mut legacy = String::from("<domain-verifications-legacy>");
    let mut users = Parcel::new();
    users.write_i32(entries.len() as i32);
    let mut codes = Vec::new();
    let code =
        AndroidPackage::read_cache_entry(&fs::read(directory.join("domain-owner.cache")).unwrap())
            .unwrap();
    for (i, (suffix, level, time)) in entries.iter().enumerate() {
        let name = format!("fixture.owner.{suffix}");
        let id = format!("00000000-0000-0000-0000-{:012x}", i + 1);
        users.write_string16(Some(&name));
        users.write_string16(Some(&id));
        users.write_i32(*level);
        users.write_i64(*time);
        if !verify { fs::write(directory.join(format!("domain-selection-{suffix}.id")), &id).unwrap(); }

        xml.push_str(&format!(
            "<package-state packageName='{name}' id='{id}' hasAutoVerifyDomains='true'><state>"
        ));
        if *level == 4 || *level == 6 {
            xml.push_str("<domain name='h0.example' state='1'/>");
        }
        xml.push_str("</state><user-states>");
        if *level != 0 { xml.push_str("<user-state userId='0' allowLinkHandling='true'>"); }
        if *level == 3 {
            xml.push_str("<enabled-hosts><host name='h0.example'/></enabled-hosts>");
        }
        if *level != 0 { xml.push_str("</user-state>"); }
        xml.push_str("</user-states></package-state>");
        if matches!(level, 1 | 2) {
            legacy.push_str(&format!("<user-states packageName='{name}'><user-state userId='0' state='{level}'/></user-states>"));
        }
        let mut code = code.clone();
        code.package_name = name;
        let user = aim_services::package::restrictions::UserState {
            installed: *level != 0,
            enabled: 1,
            instant_app: *level == 5,
            first_install_time: *time,
            ..Default::default()
        };
        codes.push((code, id, *level, user));
    }
    xml.push_str("</active></domain-verifications>");
    legacy.push_str("</domain-verifications-legacy>");
    if !verify {
        fs::write(directory.join("domain-owners-group.input"), xml).unwrap();
        fs::write(directory.join("domain-owners-group.legacy"), legacy).unwrap();
        fs::write(directory.join("domain-owners-group.users"), users.data()).unwrap();
        return;
    }
    let mut state = State::default();
    state
        .read(&aim_android_xml::read(xml.as_bytes()).unwrap())
        .unwrap();
    state
        .read_legacy(&aim_android_xml::read(legacy.as_bytes()).unwrap())
        .unwrap();
    let mut owner = Owner::new(state, Default::default());
    for (code, id, _, _) in &codes {
        owner
            .add(
                Input {
                    id,
                    name: &code.package_name,
                    code: Some(code),
                    signatures: &[],
                    system: false,
                    restrict_domains: true,
                    pre_verified: None,
                },
                &Default::default(),
            )
            .unwrap();
    }
    let mut expected = Parcel::new();
    for v2 in [false, true] {
        for user_id in [0, 10] {
            for host in ["h0.example", "unknown.invalid"] {
                let owners = owner
                    .owners(host, user_id, |name| {
                        let (code, _, level, user) = codes
                            .iter()
                            .find(|(code, ..)| code.package_name == name)
                            .unwrap();
                        Ok((*level != 6).then_some(ApprovalInput {
                            code,
                            user: (user_id == 0).then_some(user),
                            settings_v2: v2,
                            policy: Policy {
                                restrict_domains: true,
                                linked_app: false,
                            },
                        }))
                    })
                    .unwrap()
                    .into_iter()
                    .map(|(name, overrideable)| {
                        Some(aim_services::package::domain_verification::parcels::Owner {
                            name,
                            overrideable,
                        })
                    })
                    .collect::<Vec<_>>();
                aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::write_get_owners_for_domain_reply(&mut expected, Some(&owners));
            }
        }
    }
    assert_eq!(
        expected.data(),
        fs::read(directory.join("domain-owners-group.original")).unwrap(),
        "original grouped Owners replies"
    );
    let bytes = fs::read(directory.join("domain-selection.status")).unwrap();
    let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
    let mut index = 0;
    for phase in 0..3 {
        let v2 = phase != 2;
        if phase == 1 { for suffix in ["Aa", "BB", "a", "A", "İ", "ı", "instant"] {
            owner.set_link_handling_internal(Some(&format!("fixture.owner.{suffix}")), false, 0, &[]).unwrap();
        }}
        for user_id in [0, 10] { for suffix in ["disabled", "selected", "always", "Aa", "instant"] {
            for enabled in [true, false] { for multiple in [false, true] {
                let mut hosts = std::collections::BTreeSet::from(["h0.example".to_string()]); if multiple { hosts.insert("h1.example".into()); }
                let status = owner.set_user_selection(&format!("fixture.owner.{suffix}"), user_id, &hosts, enabled, |name| {
                    let (code, _, level, user) = codes.iter().find(|(code, ..)| code.package_name == name).unwrap();
                    Ok((*level != 6).then_some(ApprovalInput {code, user: (user_id == 0).then_some(user), settings_v2: v2, policy: Policy {restrict_domains: true, linked_app: false}}))
                }).unwrap();
                assert_eq!(status, reader.read_i32().unwrap(), "original selection status case={index}");
                let root = aim_android_xml::read(&fs::read(directory.join(format!("domain-selection-{index}.original"))).unwrap()).unwrap();
                let mut original = State::default();
                original.read(root.children().find(|e| e.name == "domain-verifications").unwrap()).unwrap();
                original.read_legacy(root.children().find(|e| e.name == "domain-verifications-legacy").unwrap()).unwrap();
                assert_eq!(canonical(owner.persisted()), canonical(original), "original selection state case={index}");
                index += 1;
            }}
        }}
    }
    assert_eq!(reader.remaining(), 0);
    eprintln!("Original user selection: {index} status/state transitions");

}

fn attachment_inputs(directory: &Path) {
    fs::write(directory.join("domain-cleanup-legacy.input"), "<domain-verifications-legacy><user-states packageName='fixture.domains'><user-state userId='10' state='2'/></user-states></domain-verifications-legacy>").unwrap();
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
        let extra = |name, id| {
            format!(
                "<package-state packageName='{name}' id='00000000-0000-0000-0000-{id}' hasAutoVerifyDomains='false'><user-states><user-state userId='10' allowLinkHandling='false'/><user-state userId='11' allowLinkHandling='true'/></user-states></package-state>"
            )
        };
        let xml = xml.replace(
            "</domain-verifications>",
            &format!(
                "<active>{}</active><restored>{}</restored></domain-verifications>",
                extra("pending.only", "00000000000e"),
                extra("restored.only", "00000000000f")
            ),
        );
        let xml = xml.replace("</domain-verifications>", "<active><package-state packageName='competitor' id='00000000-0000-0000-0000-000000000011' hasAutoVerifyDomains='true'><user-states><user-state userId='0' allowLinkHandling='true'><enabled-hosts><host name='h0.example'/></enabled-hosts></user-state><user-state userId='10' allowLinkHandling='true'><enabled-hosts><host name='h0.example'/></enabled-hosts></user-state></user-states></package-state></active></domain-verifications>");
        fs::write(directory.join(format!("domain-owner-{case}.input")), xml).unwrap();
    }
}

pub fn verify_uri_dto(directory: &Path) {
    use aim_binder_host::parcel::Reader;
    use aim_service_aidl::{ReadParcelable, WriteParcelable};
    use aim_services::package::domain_verification::uri_parcel::{Filter, Group};
    let bytes = fs::read(directory.join("uri-dto.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]);
    let count = reader.read_i32().unwrap();
    assert_eq!(count, 68);
    for case in 0..count {
        let group = reader.read_i32().unwrap() != 0;
        let expected_filter = if !group {
            Some(Filter {
                uri_part: reader.read_i32().unwrap(),
                pattern_type: reader.read_i32().unwrap(),
                filter: reader.read_string16().unwrap(),
            })
        } else {
            None
        };
        let mode = if group {
            reader.read_i32().unwrap()
        } else {
            -1
        };
        let payload = aim_service_aidl::read_byte_array(&mut reader)
            .unwrap()
            .unwrap();
        let mut input = Reader::new(&payload, &[]);
        let mut output = Parcel::new();
        if group {
            let value = Group::read_from(&mut input).unwrap();
            assert_eq!(value.action, mode - 1);
            assert_eq!(
                value.filters.as_ref().map(Vec::len),
                match mode {
                    0 => None,
                    1 => Some(0),
                    2 => Some(1),
                    _ => Some(2),
                }
            );
            if mode >= 2 {
                assert!(value.filters.as_ref().unwrap()[0].is_none());
            }
            if mode == 3 {
                assert_eq!(
                    value.filters.as_ref().unwrap()[1],
                    Some(Filter {
                        uri_part: 2,
                        pattern_type: 99,
                        filter: None
                    })
                );
            }
            value.write_to(&mut output);
        } else {
            let value = Filter::read_from(&mut input).unwrap();
            assert_eq!(Some(&value), expected_filter.as_ref());
            value.write_to(&mut output);
        }
        assert_eq!(input.remaining(), 0);
        assert_eq!(output.data(), payload, "original URI DTO case={case}");
    }
    assert_eq!(reader.remaining(), 0);
    let bytes = fs::read(directory.join("uri-input-bundles.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]); let cases = reader.read_i32().unwrap();
    for _ in 0..cases {
        let mode = reader.read_i32().unwrap(); let data = aim_service_aidl::read_byte_array(&mut reader).unwrap().unwrap();
        let bundle = aim_services::package::domain_verification::uri_bundle::Bundle::read_from(&mut Reader::new(&data, &[])).unwrap();
        let entries = bundle.entries().unwrap();
        assert_eq!(entries.len(), if mode == 0 {0} else {1});
        if mode != 0 {
            assert_eq!(entries[0].key.as_deref(), Some("x.example"));
            let groups = entries[0].groups().unwrap();
            match mode {
                1 | 2 | 6..=9 => assert!(groups.is_none()),
                3 => assert!(groups.unwrap().is_empty()),
                4 => assert!(groups.unwrap()[0].is_none()),
                5 => {let groups = groups.unwrap(); assert_eq!(groups[0].as_ref().unwrap().action, 99); assert_eq!(groups[0].as_ref().unwrap().filters.as_ref().unwrap()[0].as_ref().unwrap().filter, None);}
                10 | 11 => {
                    let groups = groups.unwrap();
                    assert_eq!(groups.len(), (mode - 9) as usize);
                    assert!(groups[0].is_none());
                    if mode == 11 {assert_eq!(groups[1].as_ref().unwrap().action, 99);}
                }
                _ => unreachable!(),
            }
        }
    }
    assert_eq!(reader.remaining(), 0);
    let bytes = fs::read(directory.join("uri-conversion-errors.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]);
    assert_eq!(reader.read_i32().unwrap(), 3);
    for mode in 0..3 {
        assert_eq!(reader.read_i32().unwrap(), mode);
        let expected = reader.read_string16().unwrap().unwrap();
        let parcel = match mode {
            0 => None,
            1 => Some(Group {action: 0, filters: None}),
            _ => Some(Group {action: 0, filters: Some(vec![None])}),
        };
        assert_eq!(aim_services::package::domain_verification::uri_parcel::groups_to_model(Some(vec![parcel])).unwrap_err(), expected);
    }
    assert_eq!(reader.remaining(), 0);
    let bytes = fs::read(directory.join("uri-null-match.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]); let cases = reader.read_i32().unwrap();
    for case in 0..cases {
        let filter = Filter {uri_part: reader.read_i32().unwrap(), pattern_type: reader.read_i32().unwrap(), filter: reader.read_string16().unwrap()};
        let uri = reader.read_string16().unwrap().unwrap(); let expected = reader.read_i32().unwrap();
        let error_class = reader.read_string16().unwrap(); let error_message = reader.read_string16().unwrap();
        let result = filter.match_data(&aim_services::package::uri::Uri::parse(&uri));
        if let Err(error) = &result {
            assert_eq!(Some(error.java_class()), error_class.as_deref(), "URI exception class case={case}");
            assert_eq!(Some(error.message()), error_message, "URI exception message case={case}");
            let reply = aim_services::package::component_resolver::MimeGroupError::UriMatching(error.clone()).reply();
            if let Some(expected) = error.binder_exception() {
                let reply = reply.unwrap();
                let exception = aim_binder_host::parcel::Reader::new(reply.data(), reply.objects()).read_exception().unwrap().unwrap_err();
                assert_eq!(exception.code, expected.code); assert_eq!(exception.message, expected.message);
            } else {assert!(matches!(reply, Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION)));}
        } else {assert!(error_class.is_none() && error_message.is_none());}
        let actual = match result {
            Ok(value) => i32::from(value),
            Err(aim_services::package::domain_verification::uri_parcel::MatchError::NullPattern(_)) => -1,
            Err(aim_services::package::domain_verification::uri_parcel::MatchError::InvalidPattern(_)) => -2,
            Err(aim_services::package::domain_verification::uri_parcel::MatchError::IndexOutOfBounds {..}) => -3,
        };
        let mut model = aim_services::package::intent_filter::UriRelativeFilterGroup::new(0);
        model.add_nullable(filter.uri_part, filter.pattern_type, filter.filter.as_deref());
        assert_eq!(model.filters[0].filter, filter.filter);
        let model_result = aim_services::package::intent_filter::UriRelativeFilterGroup::match_groups(&[model], &aim_services::package::uri::Uri::parse(&uri));
        let model_actual = match model_result {
            Ok(value) => i32::from(value),
            Err(aim_services::package::domain_verification::uri_parcel::MatchError::NullPattern(_)) => -1,
            Err(aim_services::package::domain_verification::uri_parcel::MatchError::InvalidPattern(_)) => -2,
            Err(aim_services::package::domain_verification::uri_parcel::MatchError::IndexOutOfBounds {..}) => -3,
        };
        assert_eq!(model_actual, expected, "original model URI match case={case}");
        assert_eq!(actual, expected, "original nullable URI match case={case} filter={filter:?} uri={uri}");
    }
    assert_eq!(reader.remaining(), 0); eprintln!("Original nullable URI matching: {cases} cases");

}

pub fn verify_uuid(directory: &Path) {
    use aim_binder_host::parcel::Reader;
    use aim_services::package::domain_verification::uuid;
    let bytes = fs::read(directory.join("domain-uuid-digits.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]);
    for c in 0..=65535 {
        assert_eq!(
            uuid::digit(c).map_or(-1, i32::from),
            reader.read_i32().unwrap(),
            "original hex digit U+{c:04X}"
        );
    }
    assert_eq!(reader.remaining(), 0);
    let bytes = fs::read(directory.join("domain-uuid.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]);
    let cases = reader.read_i32().unwrap();
    for case in 0..cases {
        let strict = reader.read_bool().unwrap();
        let value = reader.read_string16().unwrap().unwrap();
        let failed = reader.read_i32().unwrap();
        let result = reader.read_string16().unwrap().unwrap();
        let expected = if failed == 0 { Ok(result) } else { Err(result) };
        assert_eq!(
            uuid::parse(&value, strict),
            expected,
            "original UUID case={case} strict={strict} input={value:?}"
        );
    }
    assert_eq!(reader.remaining(), 0);
    eprintln!("Original UUID modes: {cases} cases and 65,536 hex digits");
}

pub fn verify_owner_sort(directory: &Path) {
    use aim_binder_host::parcel::Reader;
    let fold_bytes = fs::read(directory.join("domain-name-fold.original")).unwrap();
    let mut fold_reader = Reader::new(&fold_bytes, &[]);
    for unit in 0..=u16::MAX {
        assert_eq!(aim_services::package::domain_verification::names::fold(unit) as i32,
            fold_reader.read_i32().unwrap(), "original char fold U+{unit:04X}");
    }
    assert_eq!(fold_reader.remaining(), 0);
    let bytes = fs::read(directory.join("domain-owner-sort.original")).unwrap();
    let mut reader = Reader::new(&bytes, &[]);
    let cases = reader.read_i32().unwrap();
    assert_eq!(cases, 960);
    let mut rejected = 0;
    for case in 0..cases {
        let mode = reader.read_i32().unwrap();
        let size = reader.read_i32().unwrap() as usize;
        let entries = (0..size)
            .map(|_| {
                (
                    reader.read_i32().unwrap(),
                    reader.read_string16().unwrap().unwrap(),
                    reader.read_i64().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let failed = reader.read_i32().unwrap();
        let original = (0..size)
            .map(|_| reader.read_i32().unwrap())
            .collect::<Vec<_>>();
        let mut indices = original.clone();
        indices.sort_unstable();
        assert_eq!(indices, (0..size as i32).collect::<Vec<_>>(), "case={case}");
        assert!(matches!(failed, 0 | 1));
        let compare = |a: usize, b: usize| {
            let (a, b) = (&entries[a], &entries[b]);
            Ok(if a.2 != b.2 {
                (a.2.wrapping_sub(b.2) as i32).cmp(&0)
            } else {
                aim_services::package::domain_verification::names::compare(&a.1, &b.1)
            })
        };
        let actual = aim_services::package::timsort::sort(size, compare);
        if failed != 0 {
            assert!(actual.is_err(), "original rejected owner sort case={case}");
            rejected += 1;
            continue;
        }
        let actual = actual
            .unwrap()
            .into_iter()
            .map(|i| entries[i].0)
            .collect::<Vec<_>>();
        assert_eq!(actual, original, "owner TimSort case={case} mode={mode}");
    }
    assert_eq!(reader.remaining(), 0);
    eprintln!(
        "Original Owners sort: {cases} cases, {rejected} comparator rejections, native TimSort matches"
    );
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
        saved
            .read_legacy(
                &aim_android_xml::read(
                    &fs::read(directory.join("domain-cleanup-legacy.input")).unwrap(),
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
            if let Some(legacy) = root
                .children()
                .find(|e| e.name == "domain-verifications-legacy")
            {
                original.read_legacy(legacy).unwrap();
            }
            let values = owner
                .queries(&code, true, &config, [0, 10])
                .unwrap()
                .unwrap();
            let info = values.verification.as_ref().map(|(id, states)| {
                aim_services::package::domain_verification::parcels::Info::prepare(
                    8, id, "fixture.domains", states,
                ).unwrap()
            });
            let mut reply = Parcel::new();
            aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::write_get_domain_verification_info_reply(&mut reply, info.as_ref());
            assert_eq!(reply.data(), fs::read(directory.join(format!("domain-owner-{case}-{stage}.info"))).unwrap(), "original domain info parcel case={case} stage={stage}");
            let mut expected = Parcel::new();
            expected.write_i32(i32::from(values.verification.is_some()));
            let write_states = |out: &mut Parcel, states: &[(String, i32)]| {
                out.write_i32(states.len() as i32);
                let mut sorted = states.to_vec();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                for (host, state) in &sorted {
                    out.write_string16(Some(host));
                    out.write_i32(*state);
                }
            };
            if let Some((id, states)) = values.verification {
                expected.write_string16(Some(&id));
                write_states(&mut expected, &states);
            }
            for (user, (allowed, states)) in values.users {
                let id = &owner.package("fixture.domains").unwrap().id;
                let prepared = aim_services::package::domain_verification::parcels::UserState::prepare(8, id, "fixture.domains", user, allowed, &states).unwrap();
                let mut reply = Parcel::new();
                aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::write_get_domain_verification_user_state_reply(&mut reply, Some(&prepared));
                assert_eq!(reply.data(), fs::read(directory.join(format!("domain-owner-{case}-{stage}.user-{user}"))).unwrap(), "original user-state parcel case={case} stage={stage} user={user}");
                expected.write_i32(i32::from(allowed));
                write_states(&mut expected, &states);
            }
            let names = owner.valid_verification_package_names();
            expected.write_i32(names.len() as i32);
            for name in names {
                expected.write_string16(Some(&name));
            }
            assert_eq!(
                expected.data(),
                fs::read(directory.join(format!("domain-owner-{case}-{stage}.queries"))).unwrap(),
                "public domain queries case={case} stage={stage}"
            );
            {
                use aim_services::package::restrictions::{UserState, Suspension, SuspendingUser};
                let mut expected = Parcel::new();
                let mut owners_reply = Parcel::new();
                for v2 in [false, true] {
                    for mode in -1..=8 {
                        let mut user = UserState::default();
                        user.installed = mode != 6;
                        user.enabled = if (0..=5).contains(&mode) { mode } else { 1 };
                        user.instant_app = mode == 7;
                        if mode == 8 { user.suspensions = Some(vec![Suspension { package: "suspender".into(), user: SuspendingUser::Resolved(0), params: None }]); }
                        for host in ["h0.example", "h1.example", "h4.example", "h7.example", "h8.example", "h1024.example", "example", "sub.example", "notexample", "unknown.invalid"] {
                            expected.write_i32(owner.approval("fixture.domains", &code, (mode != -1).then_some(&user), 0, v2,
                                Policy { restrict_domains: true, linked_app: case == 1 }, host).unwrap());
                            let owners = owner.owners(host, 0, |_| Ok(Some(aim_services::package::domain_verification::owner::ApprovalInput {
                                code: &code, user: (mode != -1).then_some(&user), settings_v2: v2,
                                policy: Policy { restrict_domains: true, linked_app: case == 1 },
                            }))).unwrap().into_iter().map(|(name, overrideable)| Some(aim_services::package::domain_verification::parcels::Owner { name, overrideable })).collect::<Vec<_>>();
                            aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::write_get_owners_for_domain_reply(&mut owners_reply, Some(&owners));
                        }
                    }
                }
                assert_eq!(owners_reply.data(), fs::read(directory.join(format!("domain-owner-{case}-{stage}.owners"))).unwrap(), "original Owners replies case={case} stage={stage}");
                assert_eq!(expected.data(), fs::read(directory.join(format!("domain-owner-{case}-{stage}.approvals"))).unwrap(), "original approval levels case={case} stage={stage}");
            }
            assert_eq!(
                canonical(owner.persisted()),
                canonical(original),
                "attached domain state case={case} stage={stage}"
            );
        }
        let mut competitor = code.clone();
        competitor.package_name = "competitor".into();
        owner
            .add(
                Input {
                    id: "00000000-0000-0000-0000-000000000012",
                    name: "competitor",
                    code: Some(&competitor),
                    signatures: &[],
                    system: false,
                    restrict_domains: true,
                    pre_verified: None,
                },
                &config,
            )
            .unwrap();
        use aim_services::package::intent_filter::UriRelativeFilterGroup;
        let group = |action, path: &str| {
            let mut g = UriRelativeFilterGroup::new(action);
            g.add(0, 0, path);
            g.add(1, 0, "q=1"); g.add(2, 1, "fragment"); g.add(0, 1, "😀");
            g.add(0, 0, "Aa"); g.add(0, 0, "BB");
            g
        };
        let hosts = [
            "h0.example".to_string(),
            "h1.example".into(),
            "*.wild.example".into(),
            "undeclared.example".into(),
            "-edge.example".into(),
            "numeric.1".into(),
            "bad_name.example".into(),
            "δοκιμή.example".into(),
            "x".into(),
            format!("*.{}.example", "a".repeat(64)),
        ];
        let updates: Vec<_> = hosts
            .iter()
            .map(|h| (h.clone(), Some(vec![group(1, "/first")])))
            .collect();
        owner.set_uri_groups("fixture.domains", &updates).unwrap();
        for stage in ["uri-add", "uri-update"] {
            if stage == "uri-update" {
                owner
                    .set_uri_groups(
                        "fixture.domains",
                        &[
                            ("h0.example".into(), Some(vec![group(0, "/second")])),
                            ("h1.example".into(), Some(vec![])),
                            ("*.wild.example".into(), None),
                        ],
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
            original
                .read_legacy(
                    root.children()
                        .find(|e| e.name == "domain-verifications-legacy")
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(
                canonical(owner.persisted()),
                canonical(original),
                "URI group update case={case} stage={stage}"
            );
        }
        let previous = owner.persisted();
        owner.set_uri_groups("missing", &[]).unwrap();
        assert!(owner.set_uri_groups("missing", &updates).is_err());
        assert_eq!(owner.persisted(), previous);
    let mut requests = hosts.iter().cloned().map(Some).collect::<Vec<_>>();
    requests.push(None);
    requests.push(Some(hosts[0].clone()));
    let bytes = fs::read(directory.join(format!("domain-uri-{case}.reply"))).unwrap();
    let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
    for name in [Some("fixture.domains"), Some("missing"), None] {
        for list in [Some(requests.as_slice()), Some(&[][..]), None] {
            match owner.uri_groups_query(name, list) {
                Ok(groups) => {
                    let value =
                        aim_services::package::domain_verification::parcels::UriGroups::prepare(
                            &groups,
                        )
                        .unwrap();
                    let mut reply = Parcel::new();
                    aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::write_get_uri_relative_filter_groups_reply(&mut reply, Some(&value));
                    let start = reader.position();
                    reader.set_position(start + reply.data().len());
                    assert_eq!(
                        &bytes[start..reader.position()],
                        reply.data(),
                        "original URI Bundle case={case} name={name:?} null-list={}",
                        list.is_none()
                    );
                }
                Err(expected) => {
                    let error = reader.read_exception().unwrap().unwrap_err();
                    assert_eq!(error.code, -4);
                    assert_eq!(error.message, expected);
                }
            }
        }
    }
    assert_eq!(reader.remaining(), 0);

        let mut values = owner.uri_groups("fixture.domains", &hosts);
        values.sort_by(|a, b| a.0.cmp(&b.0));
        let mut expected = Parcel::new();
        expected.write_i32(values.len() as i32);
        for (host, groups) in values {
            expected.write_string16(Some(&host));
            expected.write_i32(groups.len() as i32);
            for g in groups {
                expected.write_i32(g.action);
                expected.write_i32(g.filters.len() as i32);
                for f in g.filters {
                    expected.write_i32(f.uri_part);
                    expected.write_i32(f.pattern_type);
                    expected.write_string16(f.filter.as_deref());
                }
            }
        }
        assert_eq!(
            expected.data(),
            fs::read(directory.join(format!("domain-uri-{case}.original"))).unwrap()
        );
        assert!(owner.uri_groups("missing", &hosts).is_empty());
        use aim_services::package::domain_verification::collector::Policy;
        let policy = Policy {
            restrict_domains: true,
            linked_app: case == 1,
        };
        let id = "00000000-0000-0000-0000-00000000000c";
        let mut hosts = std::collections::BTreeSet::from(["h0.example".into()]);
        let before = owner.persisted();
        assert_eq!(
            owner
                .set_verifier_status(
                    "00000000-0000-0000-0000-000000000000",
                    Some(&code),
                    policy,
                    &mut hosts,
                    1
                )
                .unwrap(),
            1
        );
        hosts.insert("unknown.example".into());
        assert_eq!(
            owner
                .set_verifier_status(id, Some(&code), policy, &mut hosts, 1)
                .unwrap(),
            2
        );
        assert_eq!(
            hosts,
            std::collections::BTreeSet::from(["h0.example".into()])
        );
        assert!(
            owner
                .set_verifier_status(id, Some(&code), policy, &mut Default::default(), 1)
                .is_err()
        );
        assert!(
            owner
                .set_verifier_status(id, Some(&code), policy, &mut hosts, 0)
                .is_err()
        );
        assert_eq!(owner.persisted(), before);
        hosts = (0..=8)
            .map(|h| format!("h{h}.example"))
            .chain(["h1024.example".into()])
            .collect();
        for state in [1, 1024] {
            assert_eq!(
                owner
                    .set_verifier_status(id, Some(&code), policy, &mut hosts, state)
                    .unwrap(),
                0
            );
            let root = aim_android_xml::read(
                &fs::read(directory.join(format!("domain-owner-{case}-verified-{state}.original")))
                    .unwrap(),
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
            original
                .read_legacy(
                    root.children()
                        .find(|e| e.name == "domain-verifications-legacy")
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(
                canonical(owner.persisted()),
                canonical(original),
                "verifier state case={case} state={state}"
            );
        }
        for (stage, name, allowed, id) in [
            ("link-single", Some("fixture.domains"), false, 0),
            ("link-all-users", Some("fixture.domains"), true, -1),
            ("link-all-packages", None, false, 11),
        ] {
            owner
                .set_link_handling_internal(name, allowed, id, &[0, 10])
                .unwrap();
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
            original
                .read_legacy(
                    root.children()
                        .find(|e| e.name == "domain-verifications-legacy")
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(
                canonical(owner.persisted()),
                canonical(original),
                "link handling case={case} stage={stage}"
            );
        }
        let previous = owner.persisted();
        assert!(
            owner
                .set_link_handling_internal(Some("missing"), false, 0, &[0, 10])
                .is_err()
        );
        assert_eq!(owner.persisted(), previous);
        for stage in ["package-user", "user", "package", "pending-restored"] {
            match stage {
                "package-user" => owner.clear_package_for_user("fixture.domains", 10),
                "user" => owner.clear_user(10),
                "package" => owner.clear_package("fixture.domains"),
                _ => {
                    owner.clear_package("pending.only");
                    owner.clear_package("restored.only");
                }
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
            original
                .read_legacy(
                    root.children()
                        .find(|e| e.name == "domain-verifications-legacy")
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(
                canonical(owner.persisted()),
                canonical(original),
                "domain cleanup case={case} stage={stage}"
            );
        }
        assert!(
            owner
                .package_by_id("00000000-0000-0000-0000-00000000000c")
                .is_none()
        );
    }
}

fn legacy_inputs(directory: &Path) {
    let tags = [
        "<domain-verification packageName='foreign.child' status='2'/>",
        "<domain-verification status='2'/>",
        "<domain-verification status='1'/><domain-verification status='2'/>",
        "<domain-verification status='2'/><domain-verification status='1'/>",
        "<domain-verification/>",
        "<domain-verification status='bad'/>",
        "<domain-verification status='-1'/>",
        "<domain-verification status='1024'/>",
    ];
    for (index, tag) in tags.into_iter().enumerate() {
        let xml = format!(
            "<packages><package name='fixture.domains' codePath='/data/app/fixture.domains' userId='10001' domainSetId='00000000-0000-0000-0000-00000000000d'>{tag}</package></packages>"
        );
        fs::write(directory.join(format!("domain-legacy-{index}.input")), xml).unwrap();
    }
}

pub fn verify_legacy(directory: &Path) {
    use aim_services::package::{
        domain_verification::{
            State,
            owner::{Input, Owner},
        },
        settings::Settings,
        system_config::SystemConfig,
    };
    let code =
        AndroidPackage::read_cache_entry(&fs::read(directory.join("domain-owner.cache")).unwrap())
            .unwrap();
    for index in 0..8 {
        let settings = Settings::parse(
            &aim_android_xml::read(
                &fs::read(directory.join(format!("domain-legacy-{index}.input"))).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            settings.legacy_domain_info.keys().collect::<Vec<_>>(),
            [&"fixture.domains".to_string()]
        );
        let mut owner = Owner::new(settings.domain_verification, settings.legacy_domain_info);
        let setting = &settings.packages[0];
        owner
            .add(
                Input {
                    id: setting.domain_set_id.as_deref().unwrap(),
                    name: &setting.name,
                    code: Some(&code),
                    signatures: &[],
                    system: false,
                    restrict_domains: true,
                    pre_verified: None,
                },
                &SystemConfig::default(),
            )
            .unwrap();
        let root = aim_android_xml::read(
            &fs::read(directory.join(format!("domain-legacy-{index}.original"))).unwrap(),
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
        assert_eq!(
            owner.persisted(),
            original,
            "legacy domain import case={index}"
        );
    }
}

// Persistence writes an ArraySet of package values; compare its logical maps.
fn canonical(
    mut state: aim_services::package::domain_verification::State,
) -> aim_services::package::domain_verification::State {
    state.active.sort_by(|a, b| a.name.cmp(&b.name));
    state.restored.sort_by(|a, b| a.name.cmp(&b.name));
    state.legacy.sort_by(|a, b| a.0.cmp(&b.0));
    state
}

fn persistence_defaults(directory: &Path) {
    for (case, value) in [None, Some("bad"), Some("-1"), Some("0"), Some("1")]
        .into_iter()
        .enumerate()
    {
        let attr = |key| value.map(|v| format!(" {key}='{v}'")).unwrap_or_default();
        let xml = format!(
            "<domain-verifications><active><package-state packageName='defaults' id='00000000-0000-0000-0000-000000000020'{}><state><domain name='default.example'{} /></state><user-states><user-state{}{} /></user-states><uri-relative-filter-groups><domain name='default.example' action='0'><uri-relative-filter-group{}><uri-relative-filter{}{} filter='/path'/></uri-relative-filter-group></domain></uri-relative-filter-groups></package-state></active></domain-verifications>",
            attr("hasAutoVerifyDomains"),
            attr("state"),
            attr("userId"),
            attr("allowLinkHandling"),
            attr("action"),
            attr("uri-part"),
            attr("pattern-type")
        );
        fs::write(directory.join(format!("domain-default-{case}.input")), xml).unwrap();
        let legacy = format!(
            "<domain-verifications-legacy><user-states packageName='defaults'><user-state{}{} /></user-states></domain-verifications-legacy>",
            attr("userId"),
            attr("state")
        );
        fs::write(
            directory.join(format!("domain-default-{case}.legacy")),
            legacy,
        )
        .unwrap();
        let mut state = aim_services::package::domain_verification::State::default();
        state
            .read(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-default-{case}.input"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        state
            .read_legacy(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-default-{case}.legacy"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let root = aim_services::package::owner::domains::replace(
            &aim_android_xml::read(b"<packages future='preserve'/>").unwrap(),
            &state,
        )
        .unwrap();
        for (tag, suffix) in [
            ("domain-verifications", "input"),
            ("domain-verifications-legacy", "legacy"),
        ] {
            let section = root.children().find(|e| e.name == tag).unwrap();
            fs::write(
                directory.join(format!("domain-written-{case}.{suffix}")),
                aim_android_xml::abx::write(section).unwrap(),
            )
            .unwrap();
        }
    }
}

pub fn verify_persistence_defaults(directory: &Path) {
    use aim_services::package::domain_verification::State;
    for case in 0..5 {
        let mut expected = State::default();
        expected
            .read(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-default-{case}.input"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        expected
            .read_legacy(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-default-{case}.legacy"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let root = aim_android_xml::read(
            &fs::read(directory.join(format!("domain-default-{case}.original"))).unwrap(),
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
        original
            .read_legacy(
                root.children()
                    .find(|e| e.name == "domain-verifications-legacy")
                    .unwrap(),
            )
            .unwrap();
        let mut written = State::default();
        written
            .read(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-written-{case}.input"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        written
            .read_legacy(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("domain-written-{case}.legacy"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let native_root = aim_android_xml::read(
            &fs::read(directory.join(format!("domain-written-{case}.original"))).unwrap(),
        )
        .unwrap();
        let mut original_written = State::default();
        original_written
            .read(
                native_root
                    .children()
                    .find(|e| e.name == "domain-verifications")
                    .unwrap(),
            )
            .unwrap();
        original_written
            .read_legacy(
                native_root
                    .children()
                    .find(|e| e.name == "domain-verifications-legacy")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            canonical(written),
            canonical(original_written),
            "native domain writer -> original reader case={case}"
        );
        // SettingsXml omits a domain-state value of -1; its reader supplies 0.
        for p in &mut expected.active {
            for (_, state) in &mut p.domains {
                if *state == -1 {
                    *state = 0;
                }
            }
        }
        assert_eq!(
            canonical(expected),
            canonical(original),
            "original domain defaults case={case}"
        );
    }
}
