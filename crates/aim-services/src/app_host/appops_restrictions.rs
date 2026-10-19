//! AppOpsService's client user restrictions and mode watchers at
//! `android-16.0.0_r1`: `AppOpsRestrictionsImpl`'s user restrictions and
//! their excluded packages and tags (`PackageTagsList`) by client token and
//! user, `mOpUserRestrictions`' clients, and the `ModeCallback`s of
//! `startWatchingModeWithFlags` by switch op and package, with the watchers
//! `notifyOpChanged` calls for an op.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Reader, Result as ParcelResult,
};
use aim_service_aidl::ReadParcelable;

use super::app_op_infos::{APP_OP_INFOS, OP_NONE};

/// `Parcel.VAL_NULL`, `VAL_STRING`.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
/// `ModeCallback.ALL_OPS`.
pub(super) const ALL_OPS: i32 = -2;
/// `AppOpsManager.CALL_BACK_ON_SWITCHED_OP`.
pub(super) const CALL_BACK_ON_SWITCHED_OP: i32 = 1 << 1;

/// `PackageTagsList`: packages, each with its attribution tags (empty: all
/// of them; none: a null set, as its parcel may carry one).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PackageTags(Vec<(Option<String>, Option<Tags>)>);

/// A package's attribution tags, any of which may be null.
type Tags = Vec<Option<String>>;

impl PackageTags {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn tags(&self, package: &str) -> Option<&Option<Vec<Option<String>>>> {
        self.0
            .iter()
            .find(|(p, _)| p.as_deref() == Some(package))
            .map(|(_, tags)| tags)
    }

    /// `includes`.
    fn includes(&self, package: &str) -> bool {
        self.tags(package).is_some()
    }

    /// `contains`: a package's every tag where its set is empty; a null set
    /// fails as the original's `isEmpty` of it does.
    fn contains(&self, package: &str, tag: Option<&str>) -> Result<bool, Exception> {
        match self.tags(package) {
            None => Ok(false),
            Some(None) => Err(Exception::new(EX_NULL_POINTER, "tags")),
            Some(Some(tags)) => Ok(tags.is_empty() || tags.iter().any(|t| t.as_deref() == tag)),
        }
    }
}

/// `CREATOR`: the count, then each package (`readString8`) and its tags
/// (`readArraySet`: each a `readValue` string).
impl ReadParcelable for PackageTags {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let count = r.read_i32()?;
        let mut packages = Vec::new();
        for _ in 0..count.max(0) {
            let package = r.read_string8()?;
            let size = r.read_i32()?;
            let tags = if size < 0 {
                None
            } else {
                let mut tags = Vec::new();
                for _ in 0..size {
                    tags.push(match r.read_i32()? {
                        VAL_NULL => None,
                        VAL_STRING => r.read_string16()?,
                        _ => return Err(BAD_VALUE),
                    });
                }
                Some(tags)
            };
            packages.push((package, tags));
        }
        Ok(PackageTags(packages))
    }
}

/// The user restrictions of `AppOpsRestrictionsImpl` and the clients of
/// `mOpUserRestrictions`, by client token; `L` links a client's death.
pub(super) struct UserRestrictions<L> {
    /// `mOpUserRestrictions`.
    clients: HashMap<u32, L>,
    /// `mUserRestrictions`: each token's restricted ops by user.
    restricted: HashMap<u32, BTreeMap<i32, BTreeSet<i32>>>,
    /// `mUserRestrictionExcludedPackageTags`, by token and user.
    excluded: HashMap<u32, BTreeMap<i32, PackageTags>>,
}

impl<L> Default for UserRestrictions<L> {
    fn default() -> Self {
        UserRestrictions {
            clients: HashMap::new(),
            restricted: HashMap::new(),
            excluded: HashMap::new(),
        }
    }
}

impl<L> UserRestrictions<L> {
    pub(super) fn has_client(&self, token: u32) -> bool {
        self.clients.contains_key(&token)
    }

    pub(super) fn add_client(&mut self, token: u32, link: L) {
        self.clients.insert(token, link);
    }

    pub(super) fn remove_client(&mut self, token: u32) -> Option<L> {
        self.clients.remove(&token)
    }

    /// The tokens the restrictions keep (a key of either map holds its
    /// binder, as the original's `ArrayMap` keys do).
    pub(super) fn tokens(&self) -> BTreeSet<u32> {
        self.clients
            .keys()
            .chain(self.restricted.keys())
            .chain(self.excluded.keys())
            .copied()
            .collect()
    }

    /// `setUserRestriction` of each of `users`: `putUserRestriction` and
    /// `putUserRestrictionExclusions`; whether anything changed.
    pub(super) fn set(
        &mut self,
        token: u32,
        users: &[i32],
        code: i32,
        restricted: bool,
        excluded: Option<&PackageTags>,
    ) -> bool {
        let mut changed = false;
        for &user in users {
            changed |= self.put_restriction(token, user, code, restricted);
            changed |= self.put_exclusions(token, user, excluded);
        }
        changed
    }

    fn put_restriction(&mut self, token: u32, user: i32, code: i32, restricted: bool) -> bool {
        if restricted {
            return self
                .restricted
                .entry(token)
                .or_default()
                .entry(user)
                .or_default()
                .insert(code);
        }
        let Some(users) = self.restricted.get_mut(&token) else {
            return false;
        };
        let Some(codes) = users.get_mut(&user) else {
            return false;
        };
        let changed = codes.remove(&code);
        if codes.is_empty() {
            users.remove(&user);
        }
        if users.is_empty() {
            self.restricted.remove(&token);
        }
        changed
    }

    fn put_exclusions(&mut self, token: u32, user: i32, excluded: Option<&PackageTags>) -> bool {
        if let Some(excluded) = excluded.filter(|e| !e.is_empty()) {
            self.excluded
                .entry(token)
                .or_default()
                .insert(user, excluded.clone());
            return true;
        }
        let Some(users) = self.excluded.get_mut(&token) else {
            return false;
        };
        let changed = users.remove(&user).is_some();
        if users.is_empty() {
            self.excluded.remove(&token);
        }
        changed
    }

    /// `hasUserRestrictions` (`ClientUserRestrictionState.isDefault`'s
    /// negation).
    pub(super) fn has_restrictions(&self, token: u32) -> bool {
        self.restricted.contains_key(&token)
    }

    /// `getUserRestriction` of a token, past its user's exclusions (by
    /// package alone for a check).
    fn restricts(
        &self,
        token: u32,
        user: i32,
        code: i32,
        package: &str,
        tag: Option<&str>,
        is_check: bool,
    ) -> Result<bool, Exception> {
        if !self
            .restricted
            .get(&token)
            .and_then(|users| users.get(&user))
            .is_some_and(|codes| codes.contains(&code))
        {
            return Ok(false);
        }
        let Some(excluded) = self.excluded.get(&token).and_then(|users| users.get(&user)) else {
            return Ok(true);
        };
        if is_check {
            return Ok(!excluded.includes(package));
        }
        Ok(!excluded.contains(package, tag)?)
    }

    /// Whether a client of `mOpUserRestrictions` restricts `code` for the
    /// package and tag of `user` (`isOpRestrictedLocked`'s loop).
    pub(super) fn any(
        &self,
        code: i32,
        user: i32,
        package: &str,
        tag: Option<&str>,
        is_check: bool,
    ) -> Result<bool, Exception> {
        for &token in self.clients.keys() {
            if self.restricts(token, user, code, package, tag, is_check)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `clearUserRestrictions(token)`: the ops it restricted for any user
    /// (`collectAllUserRestrictedCodes`).
    pub(super) fn clear(&mut self, token: u32) -> BTreeSet<i32> {
        let codes = self
            .restricted
            .remove(&token)
            .map(|users| users.into_values().flatten().collect())
            .unwrap_or_default();
        self.excluded.remove(&token);
        codes
    }
}

/// A `ModeCallback`: the uid it watches (`-1` for all), its flags, the op
/// it is notified of (`ALL_OPS`, `OP_NONE` or one), the uid and pid that
/// registered it, and the link to its death.
struct ModeCallback<L> {
    link: L,
    watching_uid: i32,
    watched_op: i32,
    calling_uid: i32,
    calling_pid: i32,
}

/// A watcher to tell `opChanged` of `op`: its binder and the uid and pid
/// that registered it.
#[derive(Debug, PartialEq)]
pub(super) struct ModeChange {
    pub binder: Binder,
    pub op: i32,
    pub calling_uid: i32,
    pub calling_pid: i32,
}

/// `mModeWatchers`, `mOpModeWatchers` and `mPackageModeWatchers`.
pub(super) struct ModeWatchers<L> {
    callbacks: HashMap<Binder, ModeCallback<L>>,
    ops: BTreeMap<i32, BTreeSet<Binder>>,
    packages: BTreeMap<String, BTreeSet<Binder>>,
}

impl<L> Default for ModeWatchers<L> {
    fn default() -> Self {
        ModeWatchers {
            callbacks: HashMap::new(),
            ops: BTreeMap::new(),
            packages: BTreeMap::new(),
        }
    }
}

/// `opToSwitch`.
fn switch_of(op: i32) -> i32 {
    APP_OP_INFOS[op as usize].switch_code
}

impl<L> ModeWatchers<L> {
    /// `startWatchingModeWithFlags` of a valid `op` (or `OP_NONE`): the
    /// callback made once per binder, then added to the switch op's
    /// watchers and, where it may see `package`, the package's.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn watch(
        &mut self,
        binder: Binder,
        op: i32,
        package: Option<&str>,
        flags: i32,
        calling_uid: i32,
        calling_pid: i32,
        link: impl FnOnce() -> L,
    ) {
        let switch_op = if op != OP_NONE { switch_of(op) } else { op };
        let notified = if flags & CALL_BACK_ON_SWITCHED_OP == 0 {
            if op == OP_NONE { ALL_OPS } else { op }
        } else {
            switch_op
        };
        self.callbacks
            .entry(binder)
            .or_insert_with(|| ModeCallback {
                link: link(),
                watching_uid: -1,
                watched_op: notified,
                calling_uid,
                calling_pid,
            });
        if switch_op != OP_NONE {
            self.ops.entry(switch_op).or_default().insert(binder);
        }
        if let Some(package) = package {
            self.packages
                .entry(package.to_string())
                .or_default()
                .insert(binder);
        }
    }

    /// `stopWatchingMode`: the callback's link, if it watched.
    pub(super) fn unwatch(&mut self, binder: Binder) -> Option<L> {
        let callback = self.callbacks.remove(&binder)?;
        self.ops.retain(|_, watchers| {
            watchers.remove(&binder);
            !watchers.is_empty()
        });
        self.packages.retain(|_, watchers| {
            watchers.remove(&binder);
            !watchers.is_empty()
        });
        Some(callback.link)
    }

    /// `notifyWatchersOnDefaultDevice(code, uid)`: `notifyOpChanged` of each
    /// watcher of `code` that watches `uid` (`UID_ANY` for any), for the
    /// ops it is told of: those whose switch is `code` for `ALL_OPS`, else
    /// `code` or its own op.
    pub(super) fn changes(&self, code: i32, uid: i32, uid_any: i32) -> Vec<ModeChange> {
        let mut changes = Vec::new();
        for binder in self.ops.get(&code).into_iter().flatten() {
            let callback = &self.callbacks[binder];
            if uid != uid_any && callback.watching_uid >= 0 && callback.watching_uid != uid {
                continue;
            }
            let ops: Vec<i32> = match callback.watched_op {
                ALL_OPS => APP_OP_INFOS
                    .iter()
                    .filter(|i| i.switch_code == code)
                    .map(|i| i.code)
                    .collect(),
                OP_NONE => vec![code],
                op => vec![op],
            };
            changes.extend(ops.into_iter().map(|op| ModeChange {
                binder: *binder,
                op,
                calling_uid: callback.calling_uid,
                calling_pid: callback.calling_pid,
            }));
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use aim_binder_host::parcel::Parcel;

    use super::*;

    fn tags(packages: &[(&str, Option<&[&str]>)]) -> PackageTags {
        PackageTags(
            packages
                .iter()
                .map(|(p, t)| {
                    (
                        Some(p.to_string()),
                        t.map(|t| t.iter().map(|t| Some(t.to_string())).collect()),
                    )
                })
                .collect(),
        )
    }

    /// `PackageTagsList.CREATOR` of what `writeToParcel` writes.
    #[test]
    fn package_tags_are_read_as_written() {
        let mut p = Parcel::new();
        p.write_i32(2);
        p.write_string8(Some("a"));
        p.write_i32(0);
        p.write_string8(Some("b"));
        p.write_i32(2);
        p.write_i32(VAL_STRING);
        p.write_string16(Some("t"));
        p.write_i32(VAL_NULL);
        let read = PackageTags::read_from(&mut p.reader()).unwrap();
        assert_eq!(
            read,
            PackageTags(vec![
                (Some("a".into()), Some(vec![])),
                (Some("b".into()), Some(vec![Some("t".into()), None]))
            ])
        );
        assert!(read.includes("a") && !read.includes("c"));
        assert_eq!(read.contains("a", Some("any")), Ok(true));
        assert_eq!(read.contains("b", None), Ok(true));
        assert_eq!(read.contains("b", Some("u")), Ok(false));
    }

    /// `setUserRestriction`, `getUserRestriction` and the exclusions: a
    /// check excludes by package, a note by package and tag; restrictions
    /// are per token and user, and a client counts only once added.
    #[test]
    fn user_restrictions_follow_the_restrictions_impl() {
        let mut r = UserRestrictions::<()>::default();
        assert!(r.set(
            7,
            &[0],
            26,
            true,
            Some(&tags(&[("excluded", Some(&["tag"]))]))
        ));
        assert!(!r.any(26, 0, "app", None, false).unwrap());
        r.add_client(7, ());
        assert!(r.any(26, 0, "app", None, false).unwrap());
        assert!(!r.any(26, 10, "app", None, false).unwrap());
        assert!(!r.any(27, 0, "app", None, false).unwrap());
        assert!(!r.any(26, 0, "excluded", None, true).unwrap());
        assert!(r.any(26, 0, "excluded", Some("other"), false).unwrap());
        assert!(!r.any(26, 0, "excluded", Some("tag"), false).unwrap());
        // Setting it again changes nothing but the exclusions, replaced.
        assert!(r.set(7, &[0], 26, true, None));
        assert!(!r.set(7, &[0], 26, true, None));
        assert!(r.any(26, 0, "excluded", None, true).unwrap());
        assert!(r.set(7, &[0], 26, false, None));
        assert!(!r.has_restrictions(7));
        assert!(r.set(7, &[0], 26, true, None) && r.set(7, &[0], 27, true, None));
        assert_eq!(r.clear(7), [26, 27].into());
        assert!(!r.any(26, 0, "app", None, false).unwrap());
    }

    /// `startWatchingModeWithFlags`' registrations and `notifyOpChanged`'s
    /// ops: a switched op's watchers sit at its switch, `OP_NONE` hears
    /// every op of the switch, CALL_BACK_ON_SWITCHED_OP the switch itself.
    #[test]
    fn mode_watchers_hear_the_ops_they_watch() {
        // OP_GPS (2) switches to OP_COARSE_LOCATION (0).
        let mut w = ModeWatchers::<()>::default();
        let (a, b, c) = (Binder::Handle(1), Binder::Handle(2), Binder::Local(0x30));
        w.watch(a, 2, None, 0, 10_100, 5, || ());
        w.watch(b, 2, None, CALL_BACK_ON_SWITCHED_OP, 10_101, 6, || ());
        w.watch(c, OP_NONE, Some("p"), 0, 1000, 7, || ());
        let mut heard: Vec<(Binder, i32)> = w
            .changes(0, -2, -2)
            .into_iter()
            .map(|c| (c.binder, c.op))
            .collect();
        heard.sort();
        assert_eq!(heard, [(a, 2), (b, 0)]);
        assert!(w.changes(2, -2, -2).is_empty());
        assert!(w.unwatch(a).is_some() && w.unwatch(a).is_none());
        assert_eq!(w.changes(0, -2, -2).len(), 1);
        assert!(w.unwatch(c).is_some());
        assert!(w.packages.is_empty());
    }
}
