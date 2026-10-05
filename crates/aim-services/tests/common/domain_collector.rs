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
pub fn export(directory: &Path) {
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
