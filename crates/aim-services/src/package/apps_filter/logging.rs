//! AppsFilter FeatureConfig app-ID-scoped visibility logging owner.
use std::{collections::BTreeSet, sync::Mutex};
#[derive(Debug, Default)]
pub struct Owner(Mutex<BTreeSet<i32>>);
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
impl Owner {
    pub fn enable(&self, app_id: i32, enabled: bool) {
        let mut ids = self.0.lock().unwrap();
        if enabled { ids.insert(app_id); } else { ids.remove(&app_id); }
    }
    pub fn enabled(&self, app_id: i32) -> bool { self.0.lock().unwrap().contains(&app_id) }
    pub fn decision(&self, calling_uid: i32, target: Option<&str>, filtered: bool) {
        if self.enabled(super::app_id(calling_uid)) {
            eprintln!("AppsFilter visibility: uid={calling_uid} target={} filtered={filtered}", target.unwrap_or("null"));
        }
    }
}
