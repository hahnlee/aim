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
    let r = Resolution::new(state(), &Default::default());
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
    let r = Resolution::new(s.clone(), &Default::default());
    let q = |r: &Resolution, uid: i32, intent: &Intent| {
        names(
            &r.query_intent_activities(intent, Some("image/png"), 0, 0, uid)
                .unwrap(),
        )
    };
    // An app without <queries> sees no other app...
    assert!(q(&r, 10004, &view(None, None)).is_empty());
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
    let r2 = Resolution::new(Arc::new(stopped), &Default::default());
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
    let r3 = Resolution::new(Arc::new(disabled), &Default::default());
    assert_eq!(
        q(&r3, SYSTEM_UID, &view(None, None)),
        ["a.viewer.Image", "b.gallery.Open"]
    );
    // A locked user matches direct boot aware components only.
    let mut locked = (*s).clone();
    locked.users.get_mut(&0).unwrap().unlocking_or_unlocked = false;
    let r4 = Resolution::new(Arc::new(locked), &Default::default());
    assert!(q(&r4, SYSTEM_UID, &view(None, None)).is_empty());
}

#[test]
fn resolves_explicit_services_and_providers() {
    let r = Resolution::new(state(), &Default::default());
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
    let best = r
        .resolve_intent(&view(None, None), Some("image/jpeg"), 0, 0, SYSTEM_UID)
        .unwrap();
    assert_eq!(best.unwrap().component().1, "a.viewer.Image");
}

#[test]
fn reports_what_is_not_modelled() {
    let r = Resolution::new(state(), &Default::default());
    // Two web handlers need domain verification.
    let mut s = (*state()).clone();
    let b = s.packages.get_mut("b.gallery").unwrap();
    let mut pkg = (**b.pkg.as_ref().unwrap()).clone();
    pkg.activities.push(Activity {
        main: main(
            "b.gallery",
            "b.gallery.Web",
            vec![filter(VIEW, None, Some("https"), 0)],
        ),
        ..Activity::default()
    });
    b.pkg = Some(Arc::new(pkg));
    let r2 = Resolution::new(Arc::new(s), &Default::default());
    let web = view(None, Some("https://example.com/"));
    assert!(
        r2.query_intent_activities(&web, None, 0, 0, SYSTEM_UID)
            .is_err()
    );
    // A web link with a host may go to instant app resolution; without
    // one, a single web handler is answered.
    assert!(
        r.query_intent_activities(&web, None, 0, 0, SYSTEM_UID)
            .is_err()
    );
    let bare = view(None, Some("https:"));
    let list = r
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
    let r = Resolution::new(state(), &Default::default());
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
