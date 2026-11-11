//! What a staged scan of one saved package changes, kept before the scan so
//! that its rejection restores the owner without copying every package's
//! state (#2996). Each part names the entries the scan writes; tests check
//! the restored owner against a whole copy.
use super::{SigningScan, UserState};
use crate::package::{
    libraries::Registry,
    owner::{app_ids::DetachedSetting, shared_users::SharedUser},
    settings::{KeySets, Package},
    sign::SigningDetails,
};
use std::{collections::BTreeMap, sync::Arc};

pub(in crate::package::scan) struct Undo {
    name: String,
    setting: Option<(usize, Package)>,
    shared_settings: Vec<crate::package::settings::SharedUser>,
    key_sets: KeySets,
    libraries: Registry,
    detached: Vec<(i32, Arc<DetachedSetting>)>,
    groups: Vec<(String, SharedUser)>,
    registry: crate::package::registry::Undo,
    update_reads: usize,
    disabled_users: Option<super::super::disabled::DisabledUserStates>,
    scanned_users: Option<BTreeMap<i32, UserState>>,
    loaded: Option<Arc<super::super::LoadedPackage>>,
    pending_metadata: bool,
    transferred: bool,
    seinfo: super::super::seinfo::Undo,
    parsed: Vec<(usize, (String, i32, SigningDetails, bool))>,
    permission_admissions: usize,
    #[cfg(test)]
    whole: Box<SigningScan>,
}

impl SigningScan {
    /// The state a scan of the saved package `name` may change.
    pub(in crate::package::scan) fn undo_record(&self, name: &str) -> Undo {
        let setting = self
            .settings
            .packages
            .iter()
            .position(|p| p.name == name)
            .map(|at| (at, self.settings.packages[at].clone()));
        let shared = setting.as_ref().and_then(|(_, p)| p.shared_app_id());
        Undo {
            name: name.into(),
            setting,
            shared_settings: self.settings.shared_users.clone(),
            key_sets: self.settings.key_sets.clone(),
            libraries: self.libraries.clone(),
            detached: self.identities.ids.detached_of(name),
            groups: self
                .identities
                .shared_users
                .iter()
                .filter(|(_, g)| Some(g.app_id) == shared || g.has_package(name) || g.retains(name))
                .map(|(group, g)| (group.clone(), g.clone()))
                .collect(),
            registry: self.package_registry.undo_record(name),
            update_reads: self.update_ownership.queued(),
            disabled_users: self.disabled_users.get(name).cloned(),
            scanned_users: self.scanned_users.get(name).cloned(),
            loaded: self.loaded.get(name).cloned(),
            pending_metadata: self.pending_metadata.contains(name),
            transferred: self.transferred_packages.contains(name),
            seinfo: self.seinfo_undo_record(name),
            parsed: self
                .parsed
                .iter()
                .enumerate()
                .filter(|(_, (n, ..))| n == name)
                .map(|(at, entry)| (at, entry.clone()))
                .collect(),
            permission_admissions: self.permission_admissions.len(),
            #[cfg(test)]
            whole: Box::new(self.clone()),
        }
    }

    pub(in crate::package::scan) fn roll_back(&mut self, undo: Undo) {
        let name = undo.name.as_str();
        if let Some((at, setting)) = undo.setting {
            self.settings.packages[at] = setting;
        }
        self.settings.shared_users = undo.shared_settings;
        self.settings.key_sets = undo.key_sets;
        self.libraries = undo.libraries;
        self.identities.ids.restore_detached(undo.detached);
        self.identities.shared_users.extend(undo.groups);
        self.package_registry.roll_back(undo.registry);
        self.update_ownership.drop_queued_after(undo.update_reads);
        restore(&mut self.disabled_users, name, undo.disabled_users);
        restore(&mut self.scanned_users, name, undo.scanned_users);
        restore(&mut self.loaded, name, undo.loaded);
        if !undo.pending_metadata {
            self.pending_metadata.remove(name);
        } else {
            self.pending_metadata.insert(name.into());
        }
        if !undo.transferred {
            self.transferred_packages.remove(name);
        }
        self.roll_back_seinfo(name, undo.seinfo);
        self.parsed.retain(|(n, ..)| n != name);
        for (at, entry) in undo.parsed {
            self.parsed.insert(at, entry);
        }
        self.permission_admissions
            .truncate(undo.permission_admissions);
        #[cfg(test)]
        assert!(
            *self == *undo.whole,
            "{name}: rollback differs from the owner before its scan"
        );
    }
}

fn restore<T>(map: &mut BTreeMap<String, T>, name: &str, saved: Option<T>) {
    match saved {
        Some(value) => map.insert(name.into(), value),
        None => map.remove(name),
    };
}
