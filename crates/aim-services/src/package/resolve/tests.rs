//! Resolution over a constructed state: the cuts, the order of results,
//! the match flags, stopped and disabled components, visibility, the
//! explicit and per-package paths and providers by authority.

use std::sync::Arc;

use super::super::intent::{ComponentName, FLAG_EXCLUDE_STOPPED_PACKAGES, Intent};
use super::super::intent_filter::{IntentFilter, ParsedIntentInfo};
use super::super::model::{PackageState, PackageUserState, State, User};
use super::super::pkg::{
    Activity, AndroidPackage, Component, MainComponent, Provider, Service, booleans,
};
use super::super::uri::Uri;
use super::*;

const VIEW: &str = "android.intent.action.VIEW";
const SEND: &str = "android.intent.action.SEND";
const DEFAULT: &str = "android.intent.category.DEFAULT";
const MATCH_DEFAULT_ONLY: i64 = 0x0001_0000;

fn filter(action: &str, ty: Option<&str>, scheme: Option<&str>, priority: i32) -> ParsedIntentInfo {
    let mut f = IntentFilter::default();
    f.add_action(action);
    f.add_category(DEFAULT);
    if let Some(ty) = ty {
        f.add_data_type(ty).unwrap();
    }
    if let Some(s) = scheme {
        f.add_data_scheme(s);
    }
    f.priority = priority;
    ParsedIntentInfo {
        filter: f,
        has_default: true,
        ..ParsedIntentInfo::default()
    }
}

fn main(package: &str, name: &str, intents: Vec<ParsedIntentInfo>) -> MainComponent {
    MainComponent {
        component: Component {
            name: name.into(),
            package_name: package.into(),
            intents,
            ..Component::default()
        },
        enabled: true,
        exported: true,
        direct_boot_aware: false,
        ..MainComponent::default()
    }
}

fn package(
    name: &str,
    app_id: i32,
    system: bool,
    edit: impl FnOnce(&mut AndroidPackage),
) -> PackageState {
    let mut pkg = AndroidPackage {
        package_name: name.into(),
        uid: app_id,
        booleans: booleans::ENABLED
            | booleans::HAS_CODE
            | if system { booleans::SYSTEM } else { 0 },
        ..AndroidPackage::default()
    };
    edit(&mut pkg);
    let mut ps = PackageState {
        name: name.into(),
        app_id,
        target_sdk_version: 35,
        pkg: Some(Arc::new(pkg)),
        ..PackageState::default()
    };
    ps.is.system = system;
    ps.users.insert(0, PackageUserState::default());
    ps
}

fn state() -> Arc<State> {
    let packages = [
        package("a.viewer", 10001, true, |p| {
            p.activities = vec![
                Activity {
                    main: main(
                        "a.viewer",
                        "a.viewer.Image",
                        vec![filter(VIEW, Some("image/*"), None, 0)],
                    ),
                    ..Activity::default()
                },
                Activity {
                    main: main(
                        "a.viewer",
                        "a.viewer.Web",
                        vec![filter(VIEW, None, Some("https"), 0)],
                    ),
                    ..Activity::default()
                },
            ];
        }),
        package("b.gallery", 10002, false, |p| {
            p.activities = vec![Activity {
                main: main(
                    "b.gallery",
                    "b.gallery.Open",
                    vec![filter(VIEW, Some("image/png"), None, 0)],
                ),
                ..Activity::default()
            }];
            p.services = vec![Service {
                main: main(
                    "b.gallery",
                    "b.gallery.Sync",
                    vec![filter(SEND, None, None, 0)],
                ),
                ..Service::default()
            }];
            p.providers = vec![Provider {
                main: main("b.gallery", "b.gallery.Media", Vec::new()),
                authority: Some("b.media;b.media2".into()),
                ..Provider::default()
            }];
        }),
        package("c.high", 10003, true, |p| {
            p.activities = vec![Activity {
                main: main(
                    "c.high",
                    "c.high.Open",
                    vec![filter(VIEW, Some("image/png"), None, 5)],
                ),
                ..Activity::default()
            }];
        }),
        package("d.caller", 10004, false, |_| {}),
        package("android", 1000, true, |_| {}),
        package("f.jpeg", 10006, true, |p| {
            p.activities = vec![Activity {
                main: main(
                    "f.jpeg",
                    "f.jpeg.Show",
                    vec![filter(VIEW, Some("image/jpeg"), None, 0)],
                ),
                ..Activity::default()
            }];
        }),
        package("e.queries", 10005, false, |p| {
            p.queries_packages = vec!["b.gallery".into()]
        }),
    ];
    Arc::new(State {
        packages: packages.into_iter().map(|p| (p.name.clone(), p)).collect(),
        users: [(
            0,
            User {
                unlocking_or_unlocked: true,
                ..User::default()
            },
        )]
        .into(),
        ..State::default()
    })
}

fn names(list: &[ResolveInfo]) -> Vec<String> {
    list.iter().map(|r| r.component().1.to_owned()).collect()
}

fn view(ty: Option<&str>, data: Option<&str>) -> Intent {
    Intent {
        action: Some(VIEW.into()),
        ty: ty.map(Into::into),
        data: data.map(Uri::parse),
        ..Intent::default()
    }
}

#[test]
fn orders_by_priority_then_system_then_package() {
    let r = Resolution::new(state(), &Default::default()).unwrap();
    let list = r
        .query_intent_activities(
            &view(None, None),
            Some("image/png"),
            MATCH_DEFAULT_ONLY,
            0,
            SYSTEM_UID,
        )
        .unwrap();
    // c.high's priority 5 first; then the system package, then b.
    assert_eq!(
        names(&list),
        ["c.high.Open", "a.viewer.Image", "b.gallery.Open"]
    );
    assert_eq!(list[0].priority, 5);
    assert!(list[1].system && !list[2].system);
    assert_eq!(list[1].user_handle, 0);
}

#[test]
fn filters_by_visibility_package_and_state() {
    let s = state();
    let r = Resolution::new(s.clone(), &Default::default()).unwrap();
    let q = |r: &Resolution, uid: i32, intent: &Intent| {
        names(
            &r.query_intent_activities(intent, Some("image/png"), 0, 0, uid)
                .unwrap(),
        )
    };
    // An app without <queries> sees no other app...
    assert!(q(&r, 10004, &view(None, None)).is_empty());
    // ...unless platform compat turns FILTER_APPLICATION_QUERY off for it
    // or DeviceConfig turns filtering off.
    let mut compat = (*s).clone();
    compat
        .packages
        .get_mut("d.caller")
        .unwrap()
        .filter_application_query = Some(false);
    let all = ["c.high.Open", "a.viewer.Image", "b.gallery.Open"];
    assert_eq!(
        q(
            &Resolution::new(Arc::new(compat), &Default::default()).unwrap(),
            10004,
            &view(None, None)
        ),
        all
    );
    let mut off = (*s).clone();
    off.platform.query_filtering_disabled = true;
    assert_eq!(
        q(
            &Resolution::new(Arc::new(off), &Default::default()).unwrap(),
            10004,
            &view(None, None)
        ),
        all
    );
    // ...and one that queries b.gallery sees it.
    assert_eq!(q(&r, 10005, &view(None, None)), ["b.gallery.Open"]);
    // The intent's package limits the result.
    let mut i = view(None, None);
    i.package = Some("a.viewer".into());
    assert_eq!(q(&r, SYSTEM_UID, &i), ["a.viewer.Image"]);
    // Stopped packages, when the intent excludes them.
    let mut stopped = (*s).clone();
    stopped
        .packages
        .get_mut("b.gallery")
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .stopped = true;
    let r2 = Resolution::new(Arc::new(stopped), &Default::default()).unwrap();
    let mut i = view(None, None);
    i.flags = FLAG_EXCLUDE_STOPPED_PACKAGES;
    assert_eq!(q(&r2, SYSTEM_UID, &i), ["c.high.Open", "a.viewer.Image"]);
    // A disabled component.
    let mut disabled = (*s).clone();
    let us = disabled
        .packages
        .get_mut("c.high")
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap();
    us.disabled_components = vec!["c.high.Open".into()];
    let r3 = Resolution::new(Arc::new(disabled), &Default::default()).unwrap();
    assert_eq!(
        q(&r3, SYSTEM_UID, &view(None, None)),
        ["a.viewer.Image", "b.gallery.Open"]
    );
    // A locked user matches direct boot aware components only.
    let mut locked = (*s).clone();
    locked.users.get_mut(&0).unwrap().unlocking_or_unlocked = false;
    let r4 = Resolution::new(Arc::new(locked), &Default::default()).unwrap();
    assert!(q(&r4, SYSTEM_UID, &view(None, None)).is_empty());
}

#[test]
fn overlay_actors_see_their_targets_and_overlays() {
    let mut s = (*state()).clone();
    s.system.named_actors = vec![("ns".into(), "editor".into(), "d.caller".into())];
    let gallery = s.packages.get_mut("b.gallery").unwrap();
    Arc::make_mut(gallery.pkg.as_mut().unwrap()).overlayables =
        Some(vec![("Theme".into(), Some("overlay://ns/editor".into()))]);
    let overlay = package("g.overlay", 10007, false, |p| {
        p.overlay_target = Some("b.gallery".into());
        p.overlay_target_overlayable_name = Some("Theme".into());
        p.activities = vec![Activity {
            main: main(
                "g.overlay",
                "g.overlay.Open",
                vec![filter(VIEW, Some("image/png"), None, 0)],
            ),
            ..Activity::default()
        }];
    });
    s.packages.insert(overlay.name.clone(), overlay);
    let q = |s: &State| {
        let r = Resolution::new(Arc::new(s.clone()), &Default::default()).unwrap();
        names(
            &r.query_intent_activities(&view(None, None), Some("image/png"), 0, 0, 10004)
                .unwrap(),
        )
    };
    assert_eq!(q(&s), ["b.gallery.Open", "g.overlay.Open"]);
    // An actor no SystemConfig names acts on nothing.
    s.system.named_actors[0].1 = "other".into();
    assert!(q(&s).is_empty());
}

#[test]
fn registers_a_syncable_providers_later_authorities_to_a_copy() {
    let mut s = (*state()).clone();
    let gallery = s.packages.get_mut("b.gallery").unwrap();
    // The manifest declares three authorities; b.media is b.gallery.Media's.
    gallery.syncable_authorities = vec![("b.gallery.Sync".into(), "b.sync;b.sync2;b.media".into())];
    Arc::make_mut(gallery.pkg.as_mut().unwrap())
        .providers
        .push(Provider {
            main: main("b.gallery", "b.gallery.Sync", Vec::new()),
            authority: Some("b.sync".into()),
            syncable: true,
            ..Provider::default()
        });
    let r = Resolution::new(Arc::new(s), &Default::default()).unwrap();
    let get = |name| {
        let pi = r
            .resolve_content_provider(name, 0, 0, SYSTEM_UID)
            .unwrap()
            .unwrap();
        (pi.info.item.name, pi.authority, pi.is_syncable)
    };
    let sync = Some("b.gallery.Sync".to_string());
    assert_eq!(get("b.sync"), (sync.clone(), Some("b.sync".into()), true));
    assert_eq!(get("b.sync2"), (sync, Some("b.sync;b.sync2".into()), false));
    assert_eq!(get("b.media").0.as_deref(), Some("b.gallery.Media"));
}

#[test]
fn resolves_explicit_services_and_providers() {
    let r = Resolution::new(state(), &Default::default()).unwrap();
    let mut i = view(None, None);
    i.component = Some(ComponentName {
        package: "b.gallery".into(),
        class: "b.gallery.Open".into(),
    });
    let list = r
        .query_intent_activities(&i, None, 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(names(&list), ["b.gallery.Open"]);
    let send = Intent {
        action: Some(SEND.into()),
        ..Intent::default()
    };
    let services = r
        .query_intent_services(&send, None, 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(names(&services), ["b.gallery.Sync"]);
    assert!(
        r.resolve_service(&send, None, 0, 0, 10004)
            .unwrap()
            .is_none()
    );
    let pi = r
        .resolve_content_provider("b.media2", 0, 0, SYSTEM_UID)
        .unwrap()
        .unwrap();
    assert_eq!(pi.info.item.name.as_deref(), Some("b.gallery.Media"));
    assert!(
        r.resolve_content_provider("b.media", 0, 0, 10004)
            .unwrap()
            .is_none()
    );
    assert!(
        r.resolve_content_provider("none", 0, 0, SYSTEM_UID)
            .unwrap()
            .is_none()
    );
    // resolveIntent: one result, or the first of unequal priorities.
    let best = r
        .resolve_intent(&view(None, None), Some("image/png"), 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(best.unwrap().component().1, "c.high.Open");
    // b.gallery's Open, not visible to e.queries' caller: one result.
    let mut i = view(None, None);
    i.package = Some("b.gallery".into());
    let best = r
        .resolve_intent(&i, Some("image/png"), 0, 0, 10005)
        .unwrap();
    assert_eq!(best.unwrap().component().1, "b.gallery.Open");
}

#[test]
fn reports_what_is_not_modelled() {
    let r = Resolution::new(state(), &Default::default()).unwrap();
    let web = view(None, Some("https://example.com/"));
    // Without an instant app resolver, a web link is answered.
    let list = r
        .query_intent_activities(&web, None, 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(names(&list), ["a.viewer.Web"]);
    // With one, a web link with a host goes to instant app resolution;
    // without a host it does not.
    let mut s = (*state()).clone();
    s.platform.instant_app_resolver = Some("g/.Resolver".into());
    s.platform.instant_app_installer = Some("g/.Installer".into());
    let r2 = Resolution::new(Arc::new(s), &Default::default()).unwrap();
    assert!(
        r2.query_intent_activities(&web, None, 0, 0, SYSTEM_UID)
            .is_err()
    );
    let bare = view(None, Some("https:"));
    let list = r2
        .query_intent_activities(&bare, None, 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(names(&list), ["a.viewer.Web"]);
    // Another user's packages.
    assert!(
        r.query_intent_activities(&web, None, 0, 0, 1_010_004)
            .is_err()
    );
}

/// A reply decodes field by field, every byte read.
#[test]
fn decodes_its_replies() {
    let r = Resolution::new(state(), &Default::default()).unwrap();
    let list = r
        .query_intent_activities(&view(None, None), Some("image/png"), 0x40, 0, SYSTEM_UID)
        .unwrap();
    let mut p = Parcel::new();
    let slice = ListSlice {
        creator: "android.content.pm.ResolveInfo".into(),
        items: list,
    };
    pm::write_query_intent_activities_reply(&mut p, Some(&slice));
    let mut reader = Reader::new(p.data(), &[]);
    let v = Resolver::default()
        .decode_reply(pm::QUERY_INTENT_ACTIVITIES, &mut reader)
        .unwrap()
        .unwrap();
    assert_eq!(reader.remaining(), 0);
    let Value::List(items) = v else {
        panic!("{v:?}")
    };
    assert_eq!(items.len(), 3);
    let Value::Fields(fields) = &items[0] else {
        panic!()
    };
    assert_eq!(fields[0].0, "activityInfo");
    assert!(matches!(fields[1], (ref n, Value::Fields(_)) if n == "filter"));
}

/// The state with user 0's preferred and package restrictions.
fn with_preferred(backup: Option<&str>, restrictions: Option<&str>) -> Arc<State> {
    let mut s = (*state()).clone();
    let u = s.users.get_mut(&0).unwrap();
    u.preferred_activities = backup.map(|b| b.as_bytes().to_vec());
    u.restrictions = restrictions.map(|b| b.as_bytes().to_vec());
    s.platform.resolver_titles = vec![(Some(VIEW.into()), 7), (None, 9)];
    s.platform.device_provisioned = true;
    Arc::new(s)
}

fn chosen(s: Arc<State>, ty: &str) -> ResolveInfo {
    let r = Resolution::new(s, &Default::default()).unwrap();
    r.resolve_intent(&view(None, None), Some(ty), 0, 0, SYSTEM_UID)
        .unwrap()
        .unwrap()
}

#[test]
fn chooses_preferred_persistent_or_the_resolver() {
    // a.viewer.Image and f.jpeg.Show tie: the chooser.
    let ri = chosen(with_preferred(None, None), "image/jpeg");
    assert_eq!(
        ri.component(),
        ("android", "com.android.internal.app.ResolverActivity")
    );
    let Info::Activity(ai) = &ri.info else {
        panic!()
    };
    assert_eq!(ai.info.item.label_res, 7);
    assert!(ai.info.item.meta_data.is_some() && ai.info.exported);
    assert_eq!((ri.match_, ri.priority, ri.user_handle), (0, 0, 0));
    // A preferred activity chosen among the same set.
    let pa = |set: &str| {
        format!(
            r#"<preferred-backup><preferred-activities>
<item name="f.jpeg/.Show" match="600000" always="true" set="2">
<set name="f.jpeg/.Show" /><set name="{set}" />
<filter><action name="{VIEW}" /><cat name="{DEFAULT}" /><type name="image/*" /></filter>
</item></preferred-activities></preferred-backup>"#
        )
    };
    let ri = chosen(
        with_preferred(Some(&pa("a.viewer/.Image")), None),
        "image/jpeg",
    );
    assert_eq!(ri.component().1, "f.jpeg.Show");
    // A set that no longer covers the results: ask again.
    let r = Resolution::new(with_preferred(Some(&pa("x/.Y")), None), &Default::default()).unwrap();
    let got = r
        .resolve_intent(&view(None, None), Some("image/jpeg"), 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(
        got.unwrap().component().1,
        "com.android.internal.app.ResolverActivity"
    );
    // A persistent preferred activity wins.
    let ppa = format!(
        r#"<package-restrictions><persistent-preferred-activities>
<item name="a.viewer/.Image"><filter><action name="{VIEW}" /><cat name="{DEFAULT}" />
<type name="image/*" /></filter></item></persistent-preferred-activities></package-restrictions>"#
    );
    let ri = chosen(with_preferred(None, Some(&ppa)), "image/jpeg");
    assert_eq!(ri.component().1, "a.viewer.Image");
}

fn web_filter(host: Option<&str>) -> ParsedIntentInfo {
    let mut f = IntentFilter::default();
    f.add_action(VIEW);
    f.add_category(DEFAULT);
    f.add_category("android.intent.category.BROWSABLE");
    f.add_data_scheme("https");
    if let Some(h) = host {
        f.add_data_authority(h, None);
    }
    ParsedIntentInfo {
        filter: f,
        has_default: true,
        ..ParsedIntentInfo::default()
    }
}

/// Two apps handling example.com links and two browsers.
fn web_state(edit: impl FnOnce(&mut State)) -> Arc<State> {
    let web = |name: &'static str, app_id: i32, host: Option<&'static str>, installed: i64| {
        let mut ps = package(name, app_id, true, |p| {
            p.activities = vec![Activity {
                main: main(name, &format!("{name}.Web"), vec![web_filter(host)]),
                ..Activity::default()
            }];
        });
        let us = ps.users.get_mut(&0).unwrap();
        us.first_install_time = installed;
        us.domain_selection = Some((true, Vec::new()));
        ps
    };
    let mut s = State {
        packages: [
            web("app.one", 10021, Some("example.com"), 100),
            web("app.two", 10022, Some("example.com"), 200),
            web("browser.a", 10023, None, 1),
            web("browser.b", 10024, None, 1),
        ]
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect(),
        users: [(
            0,
            User {
                unlocking_or_unlocked: true,
                ..User::default()
            },
        )]
        .into(),
        ..State::default()
    };
    edit(&mut s);
    Arc::new(s)
}

fn web_names(s: Arc<State>, flags: i64, categories: &[&str]) -> Vec<String> {
    let r = Resolution::new(s, &Default::default()).unwrap();
    let mut i = view(None, Some("https:"));
    // A host would also ask the instant app resolver (#725): resolve
    // with one through the domain filter alone.
    i.data = Some(Uri::parse("https://example.com/a"));
    i.categories = Some(categories.iter().map(|c| c.to_string()).collect());
    let found = r.query_activities_body(&i, None, flags | 0xC0000, SYSTEM_UID, 0, None);
    let mut names = names(&found.unwrap());
    names.sort();
    names
}

#[test]
fn web_links_go_to_approved_apps_or_browsers() {
    let browsable = ["android.intent.category.BROWSABLE"];
    // Not a verification intent (BROWSABLE without matching DEFAULT): the
    // apps and every browser.
    let all = web_names(web_state(|_| {}), 0, &browsable);
    assert_eq!(
        all,
        [
            "app.one.Web",
            "app.two.Web",
            "browser.a.Web",
            "browser.b.Web"
        ]
    );
    // With a default browser, only it of the browsers.
    let one = web_names(
        web_state(|s| s.users.get_mut(&0).unwrap().default_browser = Some("browser.b".into())),
        0,
        &browsable,
    );
    assert_eq!(one, ["app.one.Web", "app.two.Web", "browser.b.Web"]);
    // A verification intent with no approval: the browsers.
    let none = web_names(web_state(|_| {}), MATCH_DEFAULT_ONLY, &browsable);
    assert_eq!(none, ["browser.a.Web", "browser.b.Web"]);
    // Both verified: the last installed.
    let verified = |s: &mut State| {
        for p in ["app.one", "app.two"] {
            let us = s.packages.get_mut(p).unwrap().users.get_mut(&0).unwrap();
            us.domain_selection = Some((true, vec![("example.com".into(), 2)]));
        }
    };
    assert_eq!(
        web_names(web_state(verified), MATCH_DEFAULT_ONLY, &browsable),
        ["app.two.Web"]
    );
    // Link handling turned off for app.two, selection by the user for
    // app.one: app.one.
    let selected = |s: &mut State| {
        let us = s
            .packages
            .get_mut("app.one")
            .unwrap()
            .users
            .get_mut(&0)
            .unwrap();
        us.domain_selection = Some((
            true,
            vec![("*.example.com".into(), 1), ("example.com".into(), 1)],
        ));
        let us = s
            .packages
            .get_mut("app.two")
            .unwrap()
            .users
            .get_mut(&0)
            .unwrap();
        us.domain_selection = Some((false, vec![("example.com".into(), 2)]));
    };
    assert_eq!(
        web_names(web_state(selected), MATCH_DEFAULT_ONLY, &browsable),
        ["app.one.Web"]
    );
}

#[test]
fn mime_owner_failures_do_not_publish_partial_resolvers() {
    let mut ps = package("app", 10001, false, |pkg| {
        let mut intent = filter(VIEW, None, None, 0);
        intent.filter.add_mime_group("images");
        pkg.activities = vec![Activity {
            main: main("app", "app.Image", vec![intent]),
            ..Default::default()
        }];
    });
    ps.mime_groups = vec![
        (
            Some("images".into()),
            vec![Some("image/png".into()), Some(String::new())],
        ),
        (None, vec![None]),
    ];
    let valid = Arc::new(State {
        packages: [("app".into(), ps)].into_iter().collect(),
        ..Default::default()
    });
    let resolver = Resolver::default();
    let first = resolver.resolution(&valid).unwrap();
    let mut invalid = (*valid).clone();
    invalid.packages.get_mut("app").unwrap().mime_groups[0]
        .1
        .push(None);
    let invalid = Arc::new(invalid);
    assert!(matches!(
        resolver.resolution(&invalid),
        Err(MimeGroupError::NullType)
    ));
    assert!(Arc::ptr_eq(&first, &resolver.resolution(&valid).unwrap()));
    let request = Parcel::new();
    let reply = resolver
        .query(
            &invalid,
            pm::QUERY_INTENT_ACTIVITIES,
            1000,
            &mut aim_binder_host::parcel::Reader::new(request.data(), &[]),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        aim_binder_host::parcel::Reader::new(reply.data(), &[])
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        aim_binder_host::parcel::EX_NULL_POINTER
    );
    assert!(
        resolver
            .query(
                &invalid,
                u32::MAX,
                1000,
                &mut aim_binder_host::parcel::Reader::new(request.data(), &[])
            )
            .is_none()
    );
    let mut missing = (*valid).clone();
    missing
        .packages
        .get_mut("app")
        .unwrap()
        .mime_groups
        .remove(0);
    assert!(matches!(
        resolver.resolution(&Arc::new(missing)),
        Err(MimeGroupError::MissingGroup)
    ));
    assert!(Arc::ptr_eq(&first, &resolver.resolution(&valid).unwrap()));
}

#[test]
fn visibility_construction_retains_uri_matcher_errors_and_the_previous_resolver() {
    let querying = package("querying", 10001, false, |pkg| {
        pkg.queries_intents = vec![Intent {
            action: Some(VIEW.into()), data: Some(Uri::parse("https://x/path")),
            ..Intent::default()
        }];
    });
    let target = package("target", 10002, false, |pkg| {
        let mut intent = filter(VIEW, None, Some("https"), 0);
        intent.filter.add_data_authority("x", None);
        pkg.activities = vec![Activity {main: main("target", "target.View", vec![intent]), ..Activity::default()}];
    });
    let valid = Arc::new(State {packages: [("querying".into(), querying), ("target".into(), target)].into_iter().collect(), ..State::default()});
    let resolver = Resolver::default(); let first = resolver.resolution(&valid).unwrap();
    for (pattern, kind) in [(None, 0), (None, 1), (None, 3), (Some("*"), 3), (Some("["), 3)] {
        let mut invalid = (*valid).clone();
        let pkg = Arc::make_mut(invalid.packages.get_mut("target").unwrap().pkg.as_mut().unwrap());
        let mut group = super::super::intent_filter::UriRelativeFilterGroup::new(0);
        group.add_nullable(0, kind, pattern);
        pkg.activities[0].main.component.intents[0].filter.add_uri_relative_filter_group(group);
        let invalid = Arc::new(invalid);
        let MimeGroupError::UriMatching(error) = resolver.resolution(&invalid).err().unwrap() else {panic!("lost URI matcher error")};
        let request = Parcel::new();
        let reply = resolver.query(&invalid, pm::QUERY_INTENT_ACTIVITIES, 1000, &mut aim_binder_host::parcel::Reader::new(request.data(), &[])).unwrap();
        if let Some(expected) = error.binder_exception() {
            let reply = reply.unwrap();
            let exception = aim_binder_host::parcel::Reader::new(reply.data(), reply.objects()).read_exception().unwrap().unwrap_err();
            assert_eq!(exception.code, expected.code); assert_eq!(exception.message, expected.message);
        } else {
            assert!(matches!(reply, Err(QueryError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION))));
            let mut call = ShadowCall {service: "package", descriptor: pm::DESCRIPTOR, code: pm::QUERY_INTENT_ACTIVITIES, flags: 0, sender_pid: 123, sender_euid: 1000, seq: 1, sent: std::time::Instant::now(), dropped: 0, data: aim_binder_host::parcel::Reader::new(request.data(), request.objects())};
            assert!(matches!(resolver.answer(&invalid, &mut call), Some(Answer::Status(aim_binder_host::parcel::UNKNOWN_TRANSACTION))));
        }
        assert!(Arc::ptr_eq(&first, &resolver.resolution(&valid).unwrap()));
    }
}

#[test]
fn intent_queries_return_uri_matcher_exceptions_instead_of_not_modelled() {
    use super::super::domain_verification::uri_parcel::Filter;
    for (pattern, kind) in [(None, 0), (None, 1), (None, 3), (Some("*"), 3), (Some("["), 3)] {
        let target = package("target", 10001, false, |pkg| {
            let mut intent = filter(VIEW, None, Some("https"), 0); intent.filter.add_data_authority("x", None);
            let mut group = super::super::intent_filter::UriRelativeFilterGroup::new(0); group.add_nullable(0, kind, pattern);
            intent.filter.add_uri_relative_filter_group(group);
            let mut component = main("target", "target.View", vec![intent]); component.intent_matching_flags = 2;
            pkg.activities = vec![Activity {main: component.clone(), ..Activity::default()}];
            pkg.services = vec![Service {main: component.clone(), ..Service::default()}];
            pkg.receivers = vec![Activity {main: component.clone(), ..Activity::default()}];
            pkg.providers = vec![Provider {main: component, ..Provider::default()}];
        });
        let caller = package("caller", 10002, false, |pkg| {pkg.queries_packages = vec!["target".into()];});
        let state = Arc::new(State {packages: [("target".into(), target), ("caller".into(), caller)].into(), users: [(0, User {unlocking_or_unlocked: true, ..User::default()})].into(), ..State::default()});
        let resolver = Resolver::default();
        let error = Filter {uri_part: 0, pattern_type: kind, filter: pattern.map(str::to_owned)}.match_data(&Uri::parse("https://x/path")).unwrap_err();
        let expected = error.binder_exception();
        for (code, explicit) in [(pm::QUERY_INTENT_ACTIVITIES, false), (pm::QUERY_INTENT_SERVICES, false), (pm::QUERY_INTENT_RECEIVERS, false), (pm::QUERY_INTENT_CONTENT_PROVIDERS, false), (pm::RESOLVE_INTENT, false), (pm::RESOLVE_SERVICE, false), (pm::QUERY_INTENT_ACTIVITIES, true)] {
            let mut request = Parcel::new(); request.write_interface_token(pm::DESCRIPTOR); request.write_i32(1);
            request.write_string8(Some(VIEW)); request.write_i32(1); request.write_string8(Some("https://x/path"));
            request.write_string8(None); request.write_string8(None); request.write_i32(0); request.write_i32(0);
            request.write_string8(None);
            if explicit {request.write_string16(Some("target")); request.write_string16(Some("target.View"));}
            else {request.write_string16(None);}
            for value in [0, 0, 0, 0, -2, -1, 0, 0] {request.write_i32(value);}
            request.write_string16(None); request.write_i64(MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE); request.write_i32(0);
            let reply = resolver.query(&state, code, if explicit {10002} else {1000}, &mut aim_binder_host::parcel::Reader::new(request.data(), request.objects())).unwrap();
            let Some(expected) = &expected else {
                assert!(matches!(reply, Err(QueryError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION))));
                let mut call = ShadowCall {service: "package", descriptor: pm::DESCRIPTOR, code, flags: 0, sender_pid: 123, sender_euid: if explicit {10002} else {1000}, seq: 1, sent: std::time::Instant::now(), dropped: 0, data: aim_binder_host::parcel::Reader::new(request.data(), request.objects())};
                assert!(matches!(resolver.answer(&state, &mut call), Some(Answer::Status(aim_binder_host::parcel::UNKNOWN_TRANSACTION))));
                continue;
            };
            let reply = reply.unwrap();
            let exception = aim_binder_host::parcel::Reader::new(reply.data(), reply.objects()).read_exception().unwrap().unwrap_err();
            assert_eq!(exception.code, expected.code, "method={code} pattern={pattern:?} kind={kind}");
            assert_eq!(exception.message, expected.message);
        }
    }
}

#[test]
fn all_intent_filters_keep_manifest_order_and_apply_registered_mime_groups() {
    let mut state=state();
    let ps=Arc::make_mut(&mut state).packages.get_mut("a.viewer").unwrap();
    ps.mime_groups=vec![(Some("images".into()),vec![Some("image/png".into())])];
    Arc::make_mut(ps.pkg.as_mut().unwrap()).activities[0].main.component.intents[0].filter.mime_groups=Some(vec!["images".into()]);
    let resolver=Resolver::default();
    let invoke=|name| {
        let mut data=Parcel::new();pm::GetAllIntentFilters {package_name:name}.write(&mut data);
        resolver.query(&state,pm::GET_ALL_INTENT_FILTERS,1000,&mut Reader::new(data.data(),data.objects())).unwrap().unwrap()
    };
    let reply=invoke(Some("a.viewer".into()));let mut r=Reader::new(reply.data(),reply.objects());r.read_exception().unwrap().unwrap();assert_eq!(r.read_i32().unwrap(),1);assert_eq!(r.read_i32().unwrap(),2);assert_eq!(r.read_string16().unwrap().as_deref(),Some("android.content.IntentFilter"));
    assert_eq!(r.read_i32().unwrap(),1);let first=IntentFilter::read(&mut r,&mut Plain).unwrap();assert!(first.types.unwrap().contains(&"image/png".into()));
    assert_eq!(r.read_i32().unwrap(),1);let second=IntentFilter::read(&mut r,&mut Plain).unwrap();assert_eq!(second.schemes,Some(vec!["https".into()]));assert_eq!(r.remaining(),0);
    for name in [None,Some("".into()),Some("missing".into())] {
        let reply=invoke(name);let mut r=Reader::new(reply.data(),reply.objects());r.read_exception().unwrap().unwrap();assert_eq!(r.read_i32().unwrap(),1);assert_eq!(r.read_i32().unwrap(),0);assert_eq!(r.remaining(),0);
    }
}
