use super::rebase_created_user_scan;
use crate::package::{
    owner::usage::Usage,
    scan::{CapturedUsers, SigningScan},
    scan_snapshot::Store,
    settings::{Package, Settings},
};
use std::collections::BTreeMap;

fn snapshot() -> std::sync::Arc<crate::package::scan_snapshot::Snapshot> {
    let mut owner = SigningScan::new(
        &Default::default(),
        &Settings { packages: vec![Package {
            name: "fixture".into(), app_id: 10100, code_path: "/data/app/fixture".into(),
            ..Default::default()
        }], ..Default::default() },
        36,
    ).unwrap();
    owner.capture_user_states(BTreeMap::from([(
        ("fixture".into(), false),
        CapturedUsers { states: BTreeMap::from([(0, Default::default())]), active_aliases: Default::default() },
    )])).unwrap();
    Store::new(owner, Usage::new(["fixture"])).unwrap().capture()
}

fn creation_candidate(base: &crate::package::scan_snapshot::Snapshot) -> SigningScan {
    let mut candidate = base.owner().clone();
    candidate.set_user_state("fixture", 10, Default::default()).unwrap();
    candidate
}

#[test]
fn created_user_rebase_preserves_context_only_and_component_publications() {
    let base = snapshot();
    let same_scan = snapshot();
    let mut candidate = creation_candidate(&base);
    assert!(rebase_created_user_scan(&base, &same_scan, &mut candidate).unwrap().is_none());
    assert!(candidate.scanned_user_states("fixture").unwrap().contains_key(&10));

    let mut usage = base.usage().clone();
    usage.notify("fixture", 0, 42);
    let usage_only = Store::new(base.owner().clone(), usage).unwrap().capture();
    assert!(rebase_created_user_scan(&base, &usage_only, &mut candidate).unwrap().is_none());
    assert!(candidate.scanned_user_states("fixture").unwrap().contains_key(&10));

    let mut changed = base.owner().clone();
    let mut user = changed.scanned_user_states("fixture").unwrap()[&0].clone();
    user.hidden = true;
    changed.set_user_state("fixture", 0, user).unwrap();
    let latest = Store::new(changed, base.usage().clone()).unwrap().capture();
    assert!(rebase_created_user_scan(&base, &latest, &mut candidate).unwrap().is_none());
    let users = candidate.scanned_user_states("fixture").unwrap();
    assert!(users[&0].hidden);
    assert!(users.contains_key(&10));
}

#[test]
fn created_user_rebase_rejects_package_identity_change() {
    let base = snapshot();
    let mut changed = base.owner().clone();
    changed.settings.packages[0].version_code = 2;
    let latest = Store::new(changed, base.usage().clone()).unwrap().capture();
    let mut candidate = creation_candidate(&base);
    let reason = rebase_created_user_scan(&base, &latest, &mut candidate).unwrap().unwrap();
    assert!(reason.contains("unmerged normalized owner"), "{reason}");
    assert!(!candidate.scanned_user_states("fixture").unwrap()[&0].hidden);
}
