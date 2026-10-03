use super::*;
use aim_service_aidl::{write_byte_array, write_int_array};

/// Strings as `PackageFeed.strings` writes them.
fn strings(p: &mut Parcel, items: &[&str]) {
    p.write_i32(items.len() as i32);
    for s in items {
        p.write_string16(Some(s));
    }
}

/// A `SigningDetails` as `PackageFeed.signing` writes it.
fn signing(p: &mut Parcel, scheme: i32, signers: &[&[u8]], past: Option<&[(&[u8], i32)]>) {
    p.write_i32(scheme);
    p.write_i32(signers.len() as i32);
    for s in signers {
        write_byte_array(p, Some(s));
        p.write_i32(0);
    }
    p.write_i32(1);
    p.write_string16(Some("sun.security.rsa.RSAPublicKeyImpl"));
    write_byte_array(p, Some(b"serialized-key"));
    match past {
        None => p.write_i32(-1),
        Some(past) => {
            p.write_i32(past.len() as i32);
            for (der, flags) in past {
                write_byte_array(p, Some(der));
                p.write_i32(*flags);
            }
        }
    }
}

/// Original SharedLibraryInfo bytes inside the feed's byte-array envelope.
fn library(p: &mut Parcel, name: &str, dependency: Option<&str>) {
    use crate::package::model::SharedLibrary;
    let new = |name: &str| SharedLibrary {
        name: Some(name.into()),
        path: Some("/system/framework/lib.jar".into()),
        version: -1,
        declaring: ("android".into(), 0),
        dependents: vec![("com.example.app".into(), 7)],
        optional_dependents: Some(vec![None, Some(("optional.consumer".into(), 19))]),
        cert_digests: Some(vec![None, Some("certificate.digest".into())]),
        ..Default::default()
    };
    let mut library = new(name);
    if let Some(name) = dependency {
        library.dependencies.push(new(name));
    }
    let mut parcel = Parcel::new();
    crate::package::info::write_libraries(&mut parcel, Some(&[library]));
    // Strip writeTypedList's count and present marker.
    write_byte_array(p, Some(&parcel.data()[8..]));
}

/// A package record in `PackageFeed.packageState`'s layout.
fn package_record(name: &str, shared_user_app_id: Option<i32>) -> Vec<u8> {
    package_record_with_mime(
        name,
        shared_user_app_id,
        &[(Some("images".into()), vec![Some("image/png".into())])],
    )
}

fn package_record_with_mime(
    name: &str,
    shared_user_app_id: Option<i32>,
    groups: &[(Option<String>, Vec<Option<String>>)],
) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_string16(Some(name));
    p.write_i32(10_100);
    p.write_i32(shared_user_app_id.unwrap_or(-1));
    p.write_string16(Some("/data/app/~~a/com.example.app-b"));
    p.write_string16(None);
    p.write_string16(Some("arm64-v8a"));
    p.write_string16(None);
    p.write_string16(None);
    p.write_string16(Some("default:targetSdkVersion=36"));
    p.write_string16(None);
    p.write_i64(42);
    p.write_i32(36);
    p.write_i32(-1);
    p.write_i32(2);
    p.write_i64(1_000);
    p.write_i64(2_000);
    write_byte_array(&mut p, None);
    // hasSharedUser, isDebuggable, isPrivileged, isSystem.
    let shared = u32::from(shared_user_app_id.is_some());
    p.write_i32((shared | 1 << 3 | 1 << 15 | 1 << 19) as i32);
    p.write_i32(groups.len() as i32);
    for (name, types) in groups {
        p.write_string16(name.as_deref());
        p.write_i32(types.len() as i32);
        for value in types {
            p.write_string16(value.as_deref());
        }
    }
    p.write_i32(1);
    p.write_string16(Some("com.google.android.trichromelibrary"));
    p.write_i64(7);
    p.write_i32(0);
    strings(&mut p, &["/system/framework/lib.jar"]);
    p.write_i32(1);
    library(&mut p, "lib", Some("dep"));
    strings(&mut p, &["com.example.app.PERMISSION"]);
    signing(&mut p, 3, &[b"\x01\x02"], Some(&[(b"\x03", 8)]));
    p.write_bool(true);
    p.write_string16(Some("com.android.vending"));
    p.write_string16(None);
    p.write_string16(None);
    p.write_string16(Some("com.android.vending"));
    p.write_i32(3);
    signing(&mut p, 3, &[b"\x04"], None);
    p.write_bool(true);
    p.write_string16(Some("4a1e7c2d-0000-0000-0000-000000000000"));
    p.write_i32(1);
    p.write_string16(Some("example.com"));
    p.write_i32(1);
    // User 0.
    p.write_i32(1);
    p.write_i32(0);
    p.write_i64(11);
    p.write_i64(12);
    // installed, stopped, suspended, dataExists.
    p.write_i32(1 | 1 << 1 | 1 << 4 | 1 << 8);
    p.write_i32(0);
    p.write_i32(2);
    p.write_string16(Some("shell:2000"));
    strings(&mut p, &[]);
    strings(&mut p, &["com.example.app.Main"]);
    p.write_i32(4);
    p.write_i32(0);
    p.write_string16(None);
    p.write_string16(None);
    p.write_i64(3_000);
    p.write_i32(0);
    p.write_bool(false);
    p.write_bool(true);
    strings(&mut p, &["/product/overlay/a.apk"]);
    strings(&mut p, &[]);
    p.write_string16(Some("com.android.settings"));
    write_int_array(&mut p, Some(&[3003]));
    strings(&mut p, &["android.permission.INTERNET"]);
    p.write_bool(true);
    p.write_bool(false);
    p.write_i32(0);
    // URI relative filter groups: example.com, one blocking group.
    p.write_i32(1);
    p.write_string16(Some("example.com"));
    p.write_i32(1);
    p.write_i32(1);
    p.write_i32(1);
    p.write_i32(0);
    p.write_i32(0);
    p.write_string16(Some("/private"));
    // FILTER_APPLICATION_QUERY overridden off.
    p.write_bool(false);
    p.write_i32(1);
    p.write_string16(Some("com.example.app.Sync"));
    p.write_string16(Some("a;b"));
    p.data().to_vec()
}

fn shared_user_record(name: &str, app_id: i32, packages: &[&str]) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_string16(Some(name));
    p.write_i32(app_id);
    p.write_bool(true);
    p.write_i32(29);
    strings(&mut p, packages);
    signing(&mut p, -1, &[], None);
    p.data().to_vec()
}

fn user_record(id: i32) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i32(id);
    write_byte_array(&mut p, Some(b"<pa />"));
    write_byte_array(&mut p, Some(b"<package-restrictions />"));
    p.write_string16(Some("com.example.browser"));
    p.data().to_vec()
}

fn system_record() -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_bool(true);
    strings(&mut p, &["com.android.settings"]);
    p.write_i32(0x0103_0226);
    p.write_i32(1);
    p.write_string16(Some("android.intent.action.VIEW"));
    p.write_i32(0x0104_0001);
    p.write_string16(Some(""));
    p.write_i32(1);
    p.write_string16(None);
    p.write_string16(Some("com.example/.Installer"));
    p.write_bool(true);
    p.data().to_vec()
}

fn key(kind: i32, name: &str) -> Key {
    Key {
        kind,
        name: name.into(),
    }
}

fn put(inner: &mut Inner, kind: i32, name: &str, bytes: &[u8]) {
    inner.put(key(kind, name), bytes.len(), bytes).unwrap();
}

#[test]
fn keys_are_in_javas_order() {
    // U+1F600 is a surrogate pair, which sorts before U+FF61 in UTF-16
    // and after it in UTF-8.
    let mut keys = [
        key(PACKAGE, "\u{FF61}"),
        key(PACKAGE, "\u{1F600}"),
        key(USER, "0"),
    ];
    keys.sort();
    assert_eq!(keys[0].name, "\u{1F600}");
    assert_eq!(keys[1].name, "\u{FF61}");
    assert!(key(PACKAGE, "z") < key(SHARED_USER, "a"));
}

#[test]
fn reads_a_package_record() {
    let (p, shared) = record::package(&package_record("com.example.app", Some(1000))).unwrap();
    for library in [
        &p.uses_library_infos[0],
        &p.uses_library_infos[0].dependencies[0],
    ] {
        assert_eq!(
            library.optional_dependents,
            Some(vec![None, Some(("optional.consumer".into(), 19))])
        );
        assert_eq!(
            library.cert_digests,
            Some(vec![None, Some("certificate.digest".into())])
        );
    }
    assert_eq!(shared, Some(1000));
    assert_eq!(p.name, "com.example.app");
    assert_eq!(p.app_id, 10_100);
    assert_eq!(p.path, "/data/app/~~a/com.example.app-b");
    assert_eq!(p.primary_cpu_abi.as_deref(), Some("arm64-v8a"));
    assert_eq!(p.version_code, 42);
    assert_eq!(p.hidden_api_enforcement_policy, 2);
    assert!(p.is.system && p.is.privileged && p.is.debuggable && !p.is.vendor);
    assert_eq!(
        p.mime_groups,
        [(Some("images".into()), vec![Some("image/png".into())])]
    );
    assert_eq!(
        p.uses_static_libraries,
        [("com.google.android.trichromelibrary".into(), 7)]
    );
    let lib = &p.uses_library_infos[0];
    assert_eq!(lib.name.as_deref(), Some("lib"));
    assert_eq!(lib.code_paths, None);
    assert_eq!(lib.dependents, [("com.example.app".into(), 7)]);
    assert_eq!(lib.dependencies[0].name.as_deref(), Some("dep"));
    assert_eq!(p.installed_permissions, ["com.example.app.PERMISSION"]);
    let signatures = p.signatures.as_ref().unwrap();
    assert_eq!(signatures.scheme_version, 3);
    assert_eq!(signatures.signatures, [vec![1, 2]]);
    assert_eq!(
        signatures.public_keys.as_ref().unwrap()[0]
            .as_ref()
            .unwrap()
            .bytes,
        b"serialized-key"
    );
    assert_eq!(signatures.past_signatures, Some(vec![(vec![3], 8)]));
    assert_eq!(
        p.install_source.installer.as_deref(),
        Some("com.android.vending")
    );
    assert_eq!(p.install_source.package_source, 3);
    assert_eq!(
        p.install_source
            .initiating_package_signatures
            .as_ref()
            .unwrap()
            .public_keys
            .as_ref()
            .unwrap()[0]
            .as_ref()
            .unwrap()
            .bytes,
        b"serialized-key"
    );
    assert_eq!(
        p.domain_verification,
        Some((
            "4a1e7c2d-0000-0000-0000-000000000000".into(),
            vec![("example.com".into(), 1)]
        ))
    );
    let (domain, groups) = &p.uri_relative_filter_groups[0];
    assert_eq!(
        (
            domain.as_str(),
            groups[0].action,
            groups[0].filters[0].filter.as_str()
        ),
        ("example.com", 1, "/private")
    );
    assert_eq!(p.filter_application_query, Some(false));
    assert_eq!(
        p.syncable_authorities,
        [("com.example.app.Sync".into(), "a;b".into())]
    );
    let u = &p.users[&0];
    assert!(u.installed && u.stopped && u.data_exists && !u.hidden);
    assert_eq!((u.ce_data_inode, u.de_data_inode), (11, 12));
    assert_eq!(u.enabled, 2);
    assert_eq!(u.last_disable_app_caller.as_deref(), Some("shell:2000"));
    assert_eq!(u.disabled_components, ["com.example.app.Main"]);
    assert_eq!(u.install_reason, 4);
    assert_eq!(u.first_install_time, 3_000);
    assert_eq!(
        u.overlay_paths.as_ref().unwrap().overlay_paths,
        ["/product/overlay/a.apk"]
    );
    assert_eq!(u.suspended_by, ["com.android.settings"]);
    assert_eq!(u.gids, [3003]);
    assert_eq!(u.granted_permissions, ["android.permission.INTERNET"]);
    assert_eq!(u.domain_selection, Some((false, vec![])));
}

#[test]
fn assembles_chunks() {
    let mut inner = Inner::default();
    let k = key(PARSED, "com.example.app");
    inner.put(k.clone(), 5, b"abc").unwrap();
    assert!(inner.records.is_empty());
    inner.put(k.clone(), 5, b"de").unwrap();
    assert_eq!(&*inner.records[&k].0, b"abcde");
    inner.put(k.clone(), 2, b"a").unwrap();
    assert!(inner.put(key(USER, "0"), 1, b"x").is_err());
    assert!(inner.put(k, 1, b"xy").is_err());
}

/// The digest `PackageFeed.digest` sends for `records`.
fn java_digest(records: &[(Key, &[u8])]) -> Vec<u8> {
    let mut d = Sha256::new();
    for (key, bytes) in records {
        d.update(key.kind.to_be_bytes());
        d.update((key.name.len() as i32).to_be_bytes());
        d.update(key.name.as_bytes());
        d.update(Sha256::digest(bytes));
    }
    d.finalize().to_vec()
}

#[test]
fn publishes_a_batch_whose_digest_matches() {
    let package = package_record("com.example.app", Some(1000));
    let shared = shared_user_record("android.uid.system", 1000, &["com.example.app"]);
    let user = user_record(0);
    let system = system_record();
    let mut inner = Inner::default();
    inner.asked.push((1, Some(77), Instant::now()));
    inner.begin(true);
    put(&mut inner, PACKAGE, "com.example.app", &package);
    put(&mut inner, PARSED, "com.example.app", b"parcel");
    put(&mut inner, SHARED_USER, "android.uid.system", &shared);
    put(&mut inner, USER, "0", &user);
    put(&mut inner, SYSTEM, "", &system);
    let digest = java_digest(&[
        (key(PACKAGE, "com.example.app"), &package),
        (key(PARSED, "com.example.app"), b"parcel"),
        (key(SHARED_USER, "android.uid.system"), &shared),
        (key(USER, "0"), &user),
        (key(SYSTEM, ""), &system),
    ]);
    let state = inner.end(&digest, 1).unwrap();
    assert_eq!((state.generation, state.nonce), (1, Some(77)));
    let p = &state.packages["com.example.app"];
    assert_eq!(p.shared_user.as_deref(), Some("android.uid.system"));
    assert_eq!(p.parcel.as_deref(), Some(&b"parcel"[..]));
    let u = &state.shared_users["android.uid.system"];
    assert_eq!(
        u.private_flags,
        crate::package::settings::PRIVATE_FLAG_PRIVILEGED
    );
    assert_eq!(u.signatures, None);
    let preferred = state.users[&0].preferred_activities.as_deref();
    assert_eq!(preferred, Some(&b"<pa />"[..]));
    assert!(state.system.force_system_packages_queryable);
    assert_eq!(
        state.system.force_queryable_packages,
        ["com.android.settings"]
    );
    assert_eq!(
        state.users[&0].default_browser.as_deref(),
        Some("com.example.browser")
    );
    let restrictions = state.users[&0].restrictions.as_deref();
    assert_eq!(restrictions, Some(&b"<package-restrictions />"[..]));
    let platform = &state.platform;
    assert_eq!(
        platform.resolver_titles,
        [(Some("android.intent.action.VIEW".into()), 0x0104_0001)]
    );
    assert_eq!(
        (
            platform.custom_resolver.as_deref(),
            platform.device_provisioned
        ),
        (None, true)
    );
    assert_eq!(
        platform.instant_app_installer.as_deref(),
        Some("com.example/.Installer")
    );
    assert!(!platform.query_filtering_disabled);
    assert_eq!(inner.ended, Some((1, true)));

    // A later batch removes the parcel; one with a stale digest is not
    // published.
    inner.begin(false);
    inner.records.remove(&key(PARSED, "com.example.app"));
    assert_eq!(inner.end(&digest, 2).err(), Some(Failed::Drifted));
    assert_eq!(inner.ended, Some((2, false)));
    assert_eq!(inner.state.as_ref().unwrap().generation, 1);
    let digest = java_digest(&[
        (key(PACKAGE, "com.example.app"), &package),
        (key(SHARED_USER, "android.uid.system"), &shared),
        (key(USER, "0"), &user),
        (key(SYSTEM, ""), &system),
    ]);
    let state = inner.end(&digest, 2).unwrap();
    assert_eq!((state.generation, state.nonce), (2, None));
    assert_eq!(state.packages["com.example.app"].parcel, None);
}

#[test]
fn dumps_as_dumpsys_package() {
    let mut inner = Inner::default();
    let package = package_record("com.example.app", None);
    put(
        &mut inner,
        DISABLED_SYSTEM_PACKAGE,
        "com.example.app",
        &package,
    );
    let digest = java_digest(&[(key(DISABLED_SYSTEM_PACKAGE, "com.example.app"), &package)]);
    let text = dump(&inner.end(&digest, 0).unwrap());
    put(&mut inner, USER, "0", b"\x01");
    let digest = java_digest(&[
        (key(DISABLED_SYSTEM_PACKAGE, "com.example.app"), &package),
        (key(USER, "0"), b"\x01"),
    ]);
    assert!(matches!(inner.end(&digest, 0), Err(Failed::Unreadable(_))));
    assert_eq!(inner.ended, Some((0, false)));
    assert!(
        text.contains("Hidden system packages:\n  Package [com.example.app]:\n    appId=10100\n")
    );
    assert!(text.contains(
        "    User 0: ceDataInode=11 deDataInode=12 installed=true hidden=false suspended=true \
         distractionFlags=0 stopped=true notLaunched=false enabled=2 instant=false \
         virtual=false quarantined=false\n"
    ));
    assert!(text.contains("      disabledComponents:\n        com.example.app.Main\n"));
    // Arrays.hashCode([1, 2]) = 31 * (31 + 1) + 2.
    assert!(text.contains("signatures=version:3, signatures:[3e2], past signatures:[22 flags: 8]"));
}

#[test]
fn package_records_retain_null_and_empty_mime_names_and_types() {
    let groups = vec![
        (None, vec![None, Some(String::new())]),
        (Some(String::new()), vec![None]),
        (Some("images".into()), vec![Some("image/png".into()), None]),
    ];
    let (package, _) = record::package(&package_record_with_mime("app", None, &groups)).unwrap();
    assert_eq!(package.mime_groups, groups);
}

#[test]
fn mime_feed_rejects_absent_collection_owners_and_duplicate_names() {
    let mut missing_map = Parcel::new();
    missing_map.write_i32(-1);
    assert!(
        record::mime_groups(&mut aim_binder_host::parcel::Reader::new(
            missing_map.data(),
            &[]
        ))
        .is_err()
    );
    let mut missing_types = Parcel::new();
    missing_types.write_i32(1);
    missing_types.write_string16(None);
    missing_types.write_i32(-1);
    assert!(
        record::mime_groups(&mut aim_binder_host::parcel::Reader::new(
            missing_types.data(),
            &[]
        ))
        .is_err()
    );
    let groups = vec![(None, Vec::new()), (None, Vec::new())];
    assert!(record::package(&package_record_with_mime("app", None, &groups)).is_err());
}
