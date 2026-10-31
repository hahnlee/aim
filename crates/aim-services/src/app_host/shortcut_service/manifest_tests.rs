use super::*;
use crate::package::parse::{attrs, resources};

const ATTRS: &str = "shortcutId shortcutShortLabel shortcutLongLabel shortcutDisabledMessage icon splashScreenTheme enabled action data mimeType identifier targetPackage targetClass name value scheme host port path pathPattern pathPrefix";

/// Synthetic compiled attribute types, passed through the production TypedArray.
/// No image, SDK or missing-input skip is involved.
fn parse(xml: &str, max: usize) -> Result<manifest::Manifest, String> {
    let ids: HashMap<String, u32> = ATTRS
        .split_whitespace()
        .enumerate()
        .map(|(i, n)| (n.into(), i as u32 + 1))
        .collect();
    fn convert(e: &Element, ids: &HashMap<String, u32>) -> aim_apps::res::Element {
        use aim_apps::res::{Attr, Value};
        aim_apps::res::Element {
            name: e.name.clone(),
            attrs: e
                .attrs
                .iter()
                .map(|(n, v)| {
                    let s = v.string().unwrap().into_owned();
                    let (value, kind, data) = if let Some(n) = s.strip_prefix('@') {
                        let id = u32::from_str_radix(n, 16).unwrap();
                        (Value::Ref(id), resources::TYPE_REFERENCE, id)
                    } else if let Some(n) = s.strip_prefix("int:") {
                        let n: i32 = n.parse().unwrap();
                        (Value::Int(n), resources::TYPE_FIRST_INT, n as u32)
                    } else if let Some(n) = s.strip_prefix("float:") {
                        let n: f32 = n.parse().unwrap();
                        (Value::Float(n), resources::TYPE_FLOAT, n.to_bits())
                    } else if s == "true" || s == "false" {
                        (
                            Value::Bool(s == "true"),
                            resources::TYPE_INT_BOOLEAN,
                            u32::from(s == "true"),
                        )
                    } else {
                        (Value::String(s), resources::TYPE_STRING, 0)
                    };
                    Attr {
                        ns: attrs::ANDROID.into(),
                        name: n.clone(),
                        id: ids.get(n).copied().unwrap_or(0),
                        value,
                        kind,
                        data,
                    }
                })
                .collect(),
            children: e.children().map(|e| convert(e, ids)).collect(),
            ..Default::default()
        }
    }
    let root = convert(&aim_android_xml::read(xml.as_bytes()).unwrap(), &ids);
    let res = resources::Resources {
        tables: vec![],
        overlays: &[],
        config: Default::default(),
    };
    let ctx = attrs::Ctx {
        res: &res,
        attrs: &ids,
        error: Default::default(),
    };
    let mut result = manifest::Manifest::default();
    result.read(
        &root,
        &ctx,
        &ComponentName {
            package: "p".into(),
            class: "p.Main".into(),
        },
        max,
        42,
    )?;
    assert_eq!(ctx.error.take(), None);
    Ok(result)
}

fn package(xml: &str) -> Package {
    Package::load(
        PathBuf::new(),
        aim_android_xml::read(xml.as_bytes()).unwrap(),
    )
    .unwrap()
}

/// CTS ShortcutManagerClientApiTest.testManifestDefinition: resource labels,
/// literal IDs/categories, and Intent.parseIntent rather than intent filters.
#[test]
fn manifest_intents_categories_share_targets_and_capability_contract() {
    let m = parse(r#"<shortcuts>
      <shortcut shortcutId="one" shortcutShortLabel="@7f020000" icon="@7f030000">
        <intent action="view" data="https://example.test/x" mimeType="text/plain" identifier="i" targetPackage="p" targetClass=".Target">
          <categories name="intent-category"/><extra name="s" value="value"/><extra name="n" value="int:3"/><extra name="b" value="true"/>
        </intent><intent action="second"/><categories name="share"/><categories name="share"/>
        <capability-binding key="actions.intent.TEST"><parameter-binding key="x" value="y"/></capability-binding>
      </shortcut>
      <shortcut shortcutId="one" shortcutShortLabel="@7f020000"><intent action="duplicate"/></shortcut>
      <shortcut shortcutId="off" shortcutShortLabel="@7f020000" enabled="false"/>
      <shortcut shortcutId="@7f020000" shortcutShortLabel="@7f020000"><intent action="bad-id"/></shortcut>
      <shortcut shortcutId="literal-label" shortcutShortLabel="literal"><intent action="bad-label"/></shortcut>
      <shortcut shortcutId="float" shortcutShortLabel="@7f020000"><intent action="a"><extra name="f" value="float:1.5"/></intent></shortcut>
      <shortcut shortcutId="no-action" shortcutShortLabel="@7f020000"><intent/></shortcut>
      <share-target targetClass="p.Share"><data mimeType="text/plain" scheme="https" host="example.test" port="443" path="/x" pathPattern=".*" pathPrefix="/"/><category name="share"/><category name="share"/></share-target>
      <share-target targetClass="p.Invalid"><data mimeType="text/plain"/></share-target>
    </shortcuts>"#, 15).unwrap();
    assert_eq!(m.shortcuts.len(), 2);
    let s = &m.shortcuts[0];
    assert_eq!(
        (s.title_res_id, s.icon_res_id, s.timestamp),
        (0x7f020000, 0x7f030000, 42)
    );
    assert_eq!(s.categories, Some(vec!["share".into()]));
    assert_eq!(s.intents.len(), 2);
    let i = &s.intents[0].0.intent;
    assert_eq!(i.flags, 0x1000c000);
    assert_eq!(s.intents[1].0.intent.flags, 0);
    assert_eq!(i.component.as_ref().unwrap().class, ".Target");
    assert_eq!(i.identifier.as_deref(), Some("i"));
    assert_eq!(i.ty.as_deref(), Some("text/plain"));
    assert_eq!(i.categories, Some(vec!["intent-category".into()]));
    assert_eq!(s.intents[0].1.as_ref().unwrap().len(), 3);
    assert_eq!(
        (
            m.shortcuts[1].flags,
            m.shortcuts[1].disabled_reason,
            m.shortcuts[1].rank
        ),
        (FLAG_DISABLED | FLAG_IMMUTABLE, 1, 1)
    );
    assert_eq!(
        m.shortcuts[1].intents[0].0.intent.action.as_deref(),
        Some("android.intent.action.VIEW")
    );
    assert_eq!(m.share_targets.len(), 1);
    assert_eq!(m.share_targets[0].children().count(), 2);
    assert_eq!(
        m.share_targets[0]
            .children()
            .next()
            .unwrap()
            .string("pathPattern")
            .as_deref(),
        Some(".*")
    );
    // The pinned ShortcutParser passes null capabilityBindings, even with tags.
    let mut parcel = Parcel::new();
    ForCreator(s, 0, "p").write_to(&mut parcel);
    let mut reader = parcel.reader();
    let incoming = <Incoming as aim_service_aidl::ReadParcelable>::read_from(&mut reader).unwrap();
    assert_eq!(incoming.unsupported, None);
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn manifest_limit_stops_the_file_and_invalid_extras_fail_parsing() {
    let m = parse(r#"<shortcuts>
      <shortcut shortcutId="a" shortcutShortLabel="@7f020000"><intent action="a"/></shortcut>
      <shortcut shortcutId="b" shortcutShortLabel="@7f020000"><intent action="b"/></shortcut>
      <share-target targetClass="p.Share"><data mimeType="text/plain"/><category name="s"/></share-target>
    </shortcuts>"#, 1).unwrap();
    assert_eq!(m.shortcuts.len(), 1);
    assert!(m.share_targets.is_empty());
    assert!(parse(r#"<shortcuts><shortcut shortcutId="a" shortcutShortLabel="@7f020000"><intent action="a"><extra name="missing"/></intent></shortcut></shortcuts>"#, 15).is_err());
}

#[test]
fn disabled_manifest_entries_and_restored_shortcuts_follow_pinned_rules() {
    let mut p = package(
        r#"<package name="p"><package-info bk_src_version="3"/>
      <shortcut id="off" flags="290"/><shortcut id="on" flags="322" disabled-reason="2"/>
      <shortcut id="restore" flags="66" disabled-reason="100"/>
    </package>"#,
    );
    let m = parse(
        r#"<shortcuts>
      <shortcut shortcutId="off" shortcutShortLabel="@7f020000" enabled="false"/>
      <shortcut shortcutId="on" shortcutShortLabel="@7f020000"><intent action="on"/></shortcut>
      <shortcut shortcutId="unpublished" shortcutShortLabel="@7f020000" enabled="false"/>
    </shortcuts>"#,
        15,
    )
    .unwrap();
    p.reconcile_manifest(m, 2, 20, 15, 100, |_| true).unwrap();
    assert_eq!(p.find("off").unwrap().disabled_reason, 1);
    assert_eq!(p.find("off").unwrap().flags & FLAG_MANIFEST, 0);
    assert_eq!(p.find("on").unwrap().disabled_reason, 0);
    assert!(p.find("unpublished").is_none());
    assert_eq!(p.find("restore").unwrap().disabled_reason, 100);
    p.reconcile_manifest(manifest::Manifest::default(), 3, 30, 15, 100, |_| true)
        .unwrap();
    assert_eq!(p.find("restore").unwrap().disabled_reason, 0);
    assert_eq!(p.find("on").unwrap().disabled_reason, 2);
}

/// CTS ShortcutManagerClientApiTest.testManifestDisabled's pinned behavior;
/// updates follow ShortcutPackage.publishManifestShortcuts/disableWithId.
#[test]
fn manifest_update_reconciles_pins_collisions_counts_and_reload() {
    let mut p = package(
        r#"<package name="p"><package-info version="1" last_udpate_time="10"/>
      <shortcut id="changed" activity="p/p.Main" flags="290" title="old"/>
      <shortcut id="gone" flags="288"/><shortcut id="pinned" flags="290"/>
      <shortcut id="already-disabled" flags="354" disabled-reason="1"/>
      <shortcut id="collision" activity="p/p.Main" flags="1" rank="0" title="publisher"/>
      <shortcut id="excess" activity="p/p.Main" flags="3" rank="1"/>
      <shortcut id="invalid" activity="p/p.Gone" flags="3"/>
      <share-target targetClass="p.Old"/>
    </package>"#,
    );
    let m = parse(r#"<shortcuts>
      <shortcut shortcutId="changed" shortcutShortLabel="@7f020000"><intent action="changed"/></shortcut>
      <shortcut shortcutId="added" shortcutShortLabel="@7f020000"><intent action="added"/></shortcut>
      <shortcut shortcutId="collision" shortcutShortLabel="@7f020000"><intent action="collision"/></shortcut>
      <share-target targetClass="p.Share"><data mimeType="text/plain"/><category name="share"/></share-target>
    </shortcuts>"#, 15).unwrap();
    p.reconcile_manifest(m, 2, 20, 3, 100, |a| a.class == "p.Main")
        .unwrap();
    assert!(p.find("gone").is_none());
    let changed = p.find("changed").unwrap();
    assert_eq!(changed.flags & FLAG_PINNED, FLAG_PINNED);
    assert_eq!(
        changed.intents[0].0.intent.action.as_deref(),
        Some("changed")
    );
    assert!(p.find("added").is_some());
    assert_eq!(
        p.find("collision").unwrap().title.as_deref(),
        Some("publisher")
    );
    for id in ["pinned", "already-disabled", "invalid"] {
        let s = p.find(id).unwrap();
        assert_eq!(
            s.flags & (FLAG_PINNED | FLAG_DISABLED),
            FLAG_PINNED | FLAG_DISABLED
        );
        assert_eq!(s.flags & (FLAG_MANIFEST | FLAG_DYNAMIC), 0);
        assert_eq!(
            s.disabled_reason,
            if id == "already-disabled" { 1 } else { 2 }
        );
    }
    assert_eq!(p.find("excess").unwrap().flags & FLAG_DYNAMIC, 0);
    let dir = std::env::temp_dir().join(format!("aim-shortcut-rescan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    p.file = dir.join("p.xml");
    p.save().unwrap();
    let loaded = Package::load(p.file.clone(), read_resilient(&p.file).unwrap().unwrap()).unwrap();
    assert_eq!((loaded.version, loaded.last_update_time), (2, 20));
    assert_eq!(
        loaded
            .shortcuts
            .iter()
            .map(|s| s.to_xml())
            .collect::<Vec<_>>(),
        p.shortcuts.iter().map(|s| s.to_xml()).collect::<Vec<_>>()
    );
    assert_eq!(
        loaded
            .root
            .children()
            .filter(|e| e.name == "share-target")
            .count(),
        1
    );
    assert_eq!(
        loaded
            .root
            .children()
            .find(|e| e.name == "share-target")
            .unwrap()
            .string("targetClass")
            .as_deref(),
        Some("p.Share")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// Exercise scan selection and actual saves through a live native package
/// session: an OTA must force unchanged system and data packages.
#[test]
fn ota_forces_unchanged_system_packages_and_saves_scan_markers() {
    crate::system::with_discovery_session(|host, publish, _| {
        publish(&mut |pkg| {
            if pkg.package_name == "p" {
                pkg.booleans |= crate::package::pkg::booleans::SYSTEM;
            }
        });
        let service = ShortcutManager::new(host.clone());
        let dir = host.data().unwrap().join("system_ce/0/shortcut_service");
        std::fs::create_dir_all(dir.join("packages")).unwrap();
        let mut p = package(
            r#"<package name="p"><package-info version="1" last_udpate_time="1"/><shortcut id="old" flags="290"/></package>"#,
        );
        p.file = dir.join("packages/p.xml");
        let mut user = User {
            file: dir.join("shortcuts.xml"),
            root: aim_android_xml::read(
                br#"<user last-app-scan-time2="1" last-app-scan-fp="old"/>"#,
            )
            .unwrap(),
            packages: HashMap::from([("p".into(), p)]),
        };
        // Past scan time, newer than the fixture package's lastUpdateTime.
        let scanned = now_ms() - 1;
        user.root.attrs[0].1 = Xml::Long(scanned);
        let (state, _) = host.packages().unwrap();
        assert!(!needs_scan(&state.packages["p"], false, scanned));
        assert!(needs_scan(&state.packages["p"], true, scanned));
        assert!(needs_scan(&state.packages["q"], true, scanned));
        let before = now_ms();
        service.check_package_changes(&mut user).unwrap();
        let p = &user.packages["p"];
        assert_eq!(p.find("old").unwrap().disabled_reason, 2);
        let root = read_resilient(&user.file).unwrap().unwrap();
        assert!(root.long("last-app-scan-time2").unwrap().unwrap() >= before);
        assert_eq!(root.string("last-app-scan-fp").as_deref(), Some(""));
        let loaded =
            Package::load(p.file.clone(), read_resilient(&p.file).unwrap().unwrap()).unwrap();
        assert_eq!(loaded.find("old").unwrap().disabled_reason, 2);
        assert_eq!(loaded.version, state.packages["p"].version_code);
        assert_eq!(
            loaded.last_update_time,
            state.packages["p"].last_update_time
        );
        // Publisher calls also rescan changed package state without a reboot.
        user.packages.get_mut("p").unwrap().version = -1;
        service.on_called_by_publisher(&mut user, "p").unwrap();
        assert_eq!(user.packages["p"].version, state.packages["p"].version_code);
        // A package persistence failure must not publish the cloned package or
        // advance the durable user scan markers.
        let before = std::fs::read(&user.file).unwrap();
        std::fs::write(dir.join("blocked"), "file").unwrap();
        let p = user.packages.get_mut("p").unwrap();
        p.file = dir.join("blocked/p.xml");
        p.version = -1;
        user.root.attrs.retain(|(n, _)| n != "last-app-scan-fp");
        user.root
            .attrs
            .push(("last-app-scan-fp".into(), Xml::String("another-ota".into())));
        assert!(service.check_package_changes(&mut user).is_err());
        assert_eq!(user.packages["p"].version, -1);
        assert_eq!(std::fs::read(&user.file).unwrap(), before);
    });
}
