//! ChangedPackagesTracker, AOSP android-16.0.0_r1 (Apache 2.0).
use std::{collections::BTreeMap, sync::Mutex};
use aim_binder_host::parcel::{Exception, Parcel};

#[derive(Debug, Default)]
pub struct Owner { state: Mutex<State> }
#[derive(Debug, Default)]
struct State {
    sequence: i32,
    by_user: BTreeMap<i32, BTreeMap<i32, String>>,
    last: BTreeMap<i32, BTreeMap<String, i32>>,
}
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
#[derive(Clone, Debug, PartialEq)]
pub struct Changed { pub sequence: i32, pub packages: Vec<String> }
impl aim_service_aidl::WriteParcelable for Changed {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_i32(self.sequence);
        let names: Vec<_> = self.packages.iter().cloned().map(Some).collect();
        aim_service_aidl::write_string_list(parcel, Some(&names));
    }
}
impl Owner {
    pub fn update(&self, package: &str, users: &[i32]) -> i32 {
        let mut state = self.state.lock().unwrap();
        let sequence = state.sequence;
        for user in users.iter().rev() {
            let old = state.last.entry(*user).or_default().insert(package.into(), sequence);
            let entries = state.by_user.entry(*user).or_default();
            if let Some(old) = old { entries.remove(&old); }
            entries.insert(sequence, package.into());
        }
        state.sequence = state.sequence.wrapping_add(1);
        state.sequence
    }
    pub fn get(&self, requested: i32, user: i32) -> Result<Option<Changed>, Exception> {
        let state = self.state.lock().unwrap();
        if requested >= state.sequence { return Ok(None); }
        let Some(entries) = state.by_user.get(&user) else { return Ok(None); };
        if state.sequence.wrapping_sub(requested) < 0 {
            return Err(Exception::illegal_argument("Illegal Capacity"));
        }
        let packages: Vec<_> = entries.range(requested..state.sequence).map(|(_, package)| package.clone()).collect();
        Ok((!packages.is_empty()).then(|| Changed { sequence: state.sequence, packages }))
    }
    pub fn sequence(&self) -> i32 { self.state.lock().unwrap().sequence }
}

/// PMS binder gates precede tracker lookup. Filtering can leave a nonnull
/// ChangedPackages carrying an empty list, as the original does.
pub fn query(query: &super::query::Query<'_>, owner: &Owner, sequence: i32, user: i32)
    -> Result<Result<Option<Changed>, Exception>, super::apps_filter::NotModelled> {
    if super::apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some()
        || !query.state.users.contains_key(&user) { return Ok(Ok(None)); }
    if let Err(error) = query.enforce_cross_user(user, false, false, "getChangedPackages")? { return Ok(Err(error)); }
    let mut changed = match owner.get(sequence, user) { Ok(value) => value, Err(error) => return Ok(Err(error)) };
    if let Some(changed) = &mut changed {
        let mut visible = Vec::new();
        for package in &changed.packages {
            if !query.filtered_including_uninstalled(query.state.packages.get(package), user)? { visible.push(package.clone()); }
        }
        changed.packages = visible;
    }
    Ok(Ok(changed))
}
