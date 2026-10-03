//! Native persistence for M4 C (#798). The owner retains complete XML
//! documents, including fields outside the query model, and writes ABX.
//! It must be the only writer of the data directory; the original PMS
//! must have stopped before this store is used.
//!
//! The backup/reserve protocol and enabled-state attributes are ported
//! from AOSP android-16.0.0_r1, `ResilientAtomicFile` and `Settings`,
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
//! Native inode metadata replaces Linux chmod/chown on Darwin; fs-verity
//! is not available on APFS.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use aim_android_xml::{Element, Node, Value, abx};
use aim_storage::guest_inode::{self, GuestInode};

use super::write::Enabled;
use super::{State, resilient, restrictions::Restrictions, sibling};

pub mod app_ids;
pub mod install_sources;
pub mod key_sets;
pub mod keystore;
mod native_libraries;
mod package_list;
pub mod permission_gids;
mod removal;
pub mod resources;
pub mod seinfo;
pub mod shared_users;
mod signing;
pub mod update_ownership;
pub mod usage;

/// A failed write may have committed the main file before the reserve
/// copy failed. Callers must publish that state even when reporting it.
#[derive(Debug)]
pub struct WriteError {
    pub committed: bool,
    pub message: String,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WriteError {}

impl WriteError {
    fn before(message: impl ToString) -> Self {
        Self {
            committed: false,
            message: message.to_string(),
        }
    }
}

/// Users whose preferred state already changed before a persistence failure.
/// These users still require home/query publication and change notifications.
#[derive(Debug)]
pub struct PreferredClearError {
    pub changed_users: Vec<u32>,
    pub error: WriteError,
}

impl fmt::Display for PreferredClearError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for PreferredClearError {}

/// The persisted state and its complete restriction documents. Access
/// checks and broadcasts belong to the service using the owner.
pub struct Store {
    data: PathBuf,
    state: State,
    restrictions: BTreeMap<u32, Element>,
    settings_document: Element,
    preferred_users: BTreeSet<u32>,
    list_document: Option<String>,
}

impl Store {
    pub fn open(data: &Path, users: &[u32]) -> Result<Option<Self>, String> {
        let Some(state) = State::read(data, users)? else {
            return Ok(None);
        };
        let settings_document = resilient(
            &data.join("system/packages.xml"),
            &data.join("system/packages-backup.xml"),
            |root| Ok(root.clone()),
        )?
        .ok_or("settings document disappeared while opening owner")?;
        if super::settings::Settings::parse(&settings_document)? != state.settings {
            return Err("settings changed while opening native owner".into());
        }
        let mut restrictions = BTreeMap::new();
        for &user in users {
            let dir = data.join("system/users").join(user.to_string());
            let document = resilient(
                &dir.join("package-restrictions.xml"),
                &dir.join("package-restrictions-backup.xml"),
                |root| Ok(root.clone()),
            )?
            .unwrap_or_else(|| element("package-restrictions"));
            restrictions.insert(user, document);
        }
        let list_document = super::journaled(&data.join("system/packages.list"))?;
        if list_document
            .as_deref()
            .map(super::list::parse)
            .transpose()?
            .unwrap_or_default()
            != state.list
        {
            return Err("packages.list changed while opening native owner".into());
        }
        let preferred_users = restrictions
            .iter()
            .filter_map(|(&user, root)| {
                super::preferred::has_preferred_resolver(root).then_some(user)
            })
            .collect();
        Ok(Some(Self {
            data: data.to_owned(),
            state,
            restrictions,
            settings_document,
            preferred_users,
            list_document,
        }))
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Clear one user's preferred activities under the original Settings rules
    /// (#822). The caller owns the home update and broadcast.
    /// An error with committed=true requires publication even on reserve failure.
    pub fn clear_package_preferred_activities(
        &mut self,
        user: u32,
        package: Option<&str>,
    ) -> Result<bool, WriteError> {
        let original = self
            .restrictions
            .get(&user)
            .ok_or_else(|| WriteError::before(format!("unknown user {user}")))?;
        let mut root = original.clone();
        if !super::preferred::clear_package_document(&mut root, package) {
            return Ok(false);
        }
        Restrictions::parse(&root).map_err(WriteError::before)?;
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let dir = self.data.join("system/users").join(user.to_string());
        let path = dir.join("package-restrictions.xml");
        let backup = dir.join("package-restrictions-backup.xml");
        prepare(&path, &backup, original).map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            self.restrictions.insert(user, root);
        }
        result.map(|()| true)
    }

    /// USER_ALL preferred cleanup, in SparseArray's ascending user order.
    /// The pinned Settings method retains its removal list across resolvers:
    /// after the first removal, later existing resolvers are reported changed
    /// even if no choice was removed there. Home updates/broadcasts use this
    /// exact user inventory; they remain the service owner's responsibility.
    pub fn clear_all_package_preferred_activities(
        &mut self,
        package: Option<&str>,
    ) -> Result<Vec<u32>, PreferredClearError> {
        let users: Vec<_> = self.preferred_users.iter().copied().collect();
        let mut changed_users = Vec::new();
        let mut removed = false;
        for user in users {
            match self.clear_package_preferred_activities(user, package) {
                Ok(changed) => {
                    removed |= changed;
                    if removed {
                        changed_users.push(user);
                    }
                }
                Err(error) => {
                    if error.committed {
                        changed_users.push(user);
                    }
                    return Err(PreferredClearError {
                        changed_users,
                        error,
                    });
                }
            }
        }
        Ok(changed_users)
    }

    /// Write the final loaded non-APEX inventory supplied by the graph owner.
    /// The permission owner supplies all active-user GIDs; loaded metadata
    /// supplies labels, paths and flags. Old rows are not a source for those
    /// values. Original Settings writes this after packages.xml (#798).
    pub fn commit_package_list(
        &mut self,
        entries: &[super::list::Entry],
    ) -> Result<(), WriteError> {
        let text = self.validate_package_list(entries)?;
        package_list::write(&self.data, self.list_document.as_deref(), text.as_bytes())?;
        self.list_document = Some(text);
        self.state.list = entries.to_vec();
        Ok(())
    }

    /// Resolve every row's GIDs from the original permission owner before any
    /// disk commit. An owner/transport failure leaves the old list intact.
    pub fn commit_package_list_from_permissions(
        &mut self,
        entries: &[super::list::Entry],
        users: &[i32],
        owner: &aim_binder_host::local::Strong,
    ) -> Result<(), WriteError> {
        self.validate_package_list(entries)?;
        let mut next = entries.to_vec();
        for entry in &mut next {
            entry.gids = permission_gids::query(owner, entry.uid as i32, users)
                .map_err(|error| WriteError::before(format!("permission GID query: {error:?}")))?;
        }
        self.commit_package_list(&next)
    }

    fn validate_package_list(&self, entries: &[super::list::Entry]) -> Result<String, WriteError> {
        for entry in entries {
            let package = self
                .state
                .settings
                .packages
                .iter()
                .find(|p| p.name == entry.name)
                .ok_or_else(|| WriteError::before("packages.list names an unknown setting"))?;
            if i32::try_from(entry.uid).ok() != Some(package.app_id)
                || entry.version_code != package.version_code
            {
                return Err(WriteError::before(
                    "packages.list identity/version disagrees with settings",
                ));
            }
        }
        super::list::serialize(entries).map_err(WriteError::before)
    }

    /// Persist Settings.removeRenamedPackageLPw's real-name key cleanup after
    /// permission uninstall and shared UID conversion. Other mappings stay.
    pub fn commit_removed_renamed_package(&mut self, real_name: &str) -> Result<bool, WriteError> {
        let mut root = self.settings_document.clone();
        let before = root.content.len();
        root.content.retain(|node| {
            !matches!(node, Node::Element(e)
            if e.name == "renamed-package" && e.string("new").as_deref() == Some(real_name))
        });
        if root.content.len() == before {
            return Ok(false);
        }
        let mut expected = self.state.settings.clone();
        expected
            .renamed_packages
            .retain(|(new, _)| new != real_name);
        if super::settings::Settings::parse(&root).map_err(WriteError::before)? != expected {
            return Err(WriteError::before(
                "renamed-package cleanup changed unrelated settings",
            ));
        }
        self.commit_package_document(root).map(|()| true)
    }

    /// Persist a completed setting/UID removal after its side owners finish.
    /// Per-user restrictions, permissions and query publication are separate
    /// commits; a committed error still requires publishing the new settings.
    pub fn commit_removed_package_setting(
        &mut self,
        settings: &super::settings::Settings,
        package: &str,
    ) -> Result<(), WriteError> {
        let root = removal::replace(&self.settings_document, settings, package)
            .map_err(WriteError::before)?;
        let result = self.commit_package_document(root);
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            for (_, user) in &mut self.state.users {
                user.restrictions
                    .packages
                    .retain(|(name, _)| name != package);
            }
        }
        result
    }

    /// Remove a deleted setting's saved user entry, preserving other XML.
    /// Call only after the global setting commit; permission files have their
    /// own owner and are not changed by this stage (#798/#822).
    pub fn commit_removed_package_restrictions(
        &mut self,
        package: &str,
        user: u32,
    ) -> Result<bool, WriteError> {
        if self
            .state
            .settings
            .packages
            .iter()
            .any(|p| p.name == package)
        {
            return Err(WriteError::before("package setting still exists"));
        }
        let original = self
            .restrictions
            .get(&user)
            .ok_or_else(|| WriteError::before(format!("unknown user {user}")))?;
        let mut root = original.clone();
        let before = root.content.len();
        root.content.retain(|node| {
            !matches!(node, Node::Element(e)
            if e.name == "pkg" && e.string("name").as_deref() == Some(package))
        });
        if root.content.len() == before {
            return Ok(false);
        }
        Restrictions::parse(&root).map_err(WriteError::before)?;
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let dir = self.data.join("system/users").join(user.to_string());
        let path = dir.join("package-restrictions.xml");
        let backup = dir.join("package-restrictions-backup.xml");
        prepare(&path, &backup, original).map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            self.restrictions.insert(user, root);
            let current = &mut self
                .state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .expect("opened user")
                .1
                .restrictions;
            current.packages.retain(|(name, _)| name != package);
        }
        result.map(|()| true)
    }

    /// Persist an owner-authorized signing scan. This changes signature
    /// state only, retaining every unrelated XML node. Serialized keys
    /// remain in the scan snapshot; packages.xml persists certificates.
    pub fn commit_signatures(
        &mut self,
        settings: &super::settings::Settings,
    ) -> Result<(), WriteError> {
        let root =
            signing::replace(&self.settings_document, settings).map_err(WriteError::before)?;
        self.commit_package_document(root)
    }

    /// Persist owner-authorized single-member shared UID conversions together
    /// with their signing scan. UID numbers and all unrelated metadata stay
    /// fixed. Eligibility and image migration policy belong to the scan owner.
    pub fn commit_shared_uid_migrations(
        &mut self,
        settings: &super::settings::Settings,
    ) -> Result<(), WriteError> {
        let root = signing::replace_migrations(&self.settings_document, settings)
            .map_err(WriteError::before)?;
        self.commit_package_document(root)
    }

    /// Persist completed native-library scan metadata for retained packages.
    /// Required copies and ABI policy belong to the scan/installation owners;
    /// this writer rejects changes to identity, signers or unrelated settings.
    pub fn commit_native_library_metadata(
        &mut self,
        settings: &super::settings::Settings,
    ) -> Result<(), WriteError> {
        let root = native_libraries::replace(&self.settings_document, settings)
            .map_err(WriteError::before)?;
        self.commit_package_document(root)
    }

    /// Persist keyset ownership for retained packages after scan completion.
    /// Other package metadata and publication remain with their owners.
    pub fn commit_key_sets(
        &mut self,
        settings: &super::settings::Settings,
    ) -> Result<(), WriteError> {
        let root = key_sets::replace_registered(&self.settings_document, settings)
            .map_err(WriteError::before)?;
        self.commit_package_document(root)
    }

    /// Persist completed denylist effects on retained active packages. This
    /// writer only clears update owners; policy, async ordering and replica
    /// publication belong to the package commit owner.
    pub fn commit_update_owner_clearings(
        &mut self,
        settings: &super::settings::Settings,
    ) -> Result<(), WriteError> {
        let root =
            update_ownership::persistence::replace_clearings(&self.settings_document, settings)
                .map_err(WriteError::before)?;
        self.commit_package_document(root)
    }

    /// Persist the domain/keyset stage of an owner-authorized boot removal.
    /// Package settings, UID/user state and permission deletion follow later.
    pub fn commit_removed_boot_metadata(&mut self, package: &str) -> Result<(), WriteError> {
        let root =
            key_sets::replace(&self.settings_document, package).map_err(WriteError::before)?;
        self.commit_package_document(root)
    }

    fn commit_package_document(&mut self, root: Element) -> Result<(), WriteError> {
        let persisted = super::settings::Settings::parse(&root).map_err(WriteError::before)?;
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let path = self.data.join("system/packages.xml");
        let backup = self.data.join("system/packages-backup.xml");
        prepare_document(&path, &backup, &self.settings_document, |root| {
            super::settings::Settings::parse(root).map(|_| ())
        })
        .map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            self.settings_document = root;
            self.state.settings = persisted;
        }
        result
    }

    /// Commits an enabled-state change that the service has validated.
    /// The entire user's restriction document is retained; settings,
    /// domain verification and permission files are not rewritten here.
    pub fn commit_enabled(
        &mut self,
        package: &str,
        user: u32,
        enabled: &Enabled,
    ) -> Result<(), WriteError> {
        if !(0..=4).contains(&enabled.enabled)
            || !enabled
                .enabled_components
                .is_disjoint(&enabled.disabled_components)
        {
            return Err(WriteError::before("invalid enabled state"));
        }
        if !self
            .state
            .settings
            .packages
            .iter()
            .any(|p| p.name == package)
        {
            return Err(WriteError::before(format!("unknown package {package}")));
        }
        let mut root = self
            .restrictions
            .get(&user)
            .cloned()
            .ok_or_else(|| WriteError::before(format!("unknown user {user}")))?;
        let index = root.content.iter().position(|node| {
            matches!(node,
            Node::Element(e) if e.name == "pkg" && e.string("name").as_deref() == Some(package))
        });
        let index = index.unwrap_or_else(|| {
            let mut entry = element("pkg");
            entry
                .attrs
                .push(("name".into(), Value::String(package.into())));
            root.content.push(Node::Element(entry));
            root.content.len() - 1
        });
        let Node::Element(entry) = &mut root.content[index] else {
            unreachable!()
        };
        attribute(
            entry,
            "enabled",
            (enabled.enabled != 0).then_some(Value::Int(enabled.enabled)),
        );
        attribute(
            entry,
            "enabledCaller",
            enabled.last_disable_app_caller.clone().map(Value::String),
        );
        for (name, components) in [
            ("enabled-components", &enabled.enabled_components),
            ("disabled-components", &enabled.disabled_components),
        ] {
            entry
                .content
                .retain(|n| !matches!(n, Node::Element(e) if e.name == name));
            if !components.is_empty() {
                let mut list = element(name);
                for component in components {
                    let mut item = element("item");
                    item.attrs
                        .push(("name".into(), Value::String(component.clone())));
                    list.content.push(Node::Element(item));
                }
                entry.content.push(Node::Element(list));
            }
        }
        Restrictions::parse(&root).map_err(WriteError::before)?;
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let dir = self.data.join("system/users").join(user.to_string());
        let path = dir.join("package-restrictions.xml");
        let backup = dir.join("package-restrictions-backup.xml");
        prepare(&path, &backup, &self.restrictions[&user]).map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            self.restrictions.insert(user, root);
            let current = &mut self
                .state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .expect("opened user")
                .1
                .restrictions;
            let updated = &mut current
                .packages
                .iter_mut()
                .find(|(name, _)| name == package)
                .expect("known package")
                .1;
            updated.enabled = enabled.enabled;
            updated.last_disable_app_caller = enabled.last_disable_app_caller.clone();
            updated.enabled_components = enabled.enabled_components.iter().cloned().collect();
            updated.disabled_components = enabled.disabled_components.iter().cloned().collect();
        }
        result
    }
}

fn element(name: &str) -> Element {
    Element {
        name: name.into(),
        attrs: Vec::new(),
        content: Vec::new(),
    }
}

fn attribute(e: &mut Element, name: &str, value: Option<Value>) {
    e.attrs.retain(|(key, _)| key != name);
    if let Some(value) = value {
        e.attrs.push((name.into(), value));
    }
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Refuse another writer's state. If only the reserve parsed, preserve
/// it as the old backup before startWrite removes the reserve.
fn prepare(path: &Path, backup: &Path, expected: &Element) -> Result<(), String> {
    prepare_document(path, backup, expected, |root| {
        Restrictions::parse(root).map(|_| ())
    })
}

fn prepare_document(
    path: &Path,
    backup: &Path,
    expected: &Element,
    validate: impl Fn(&Element) -> Result<(), String>,
) -> Result<(), String> {
    let parse = |root: &Element| {
        validate(root)?;
        Ok(root.clone())
    };
    let current = resilient(path, backup, parse)?;
    if current.is_none() && expected != &element("package-restrictions") {
        return Err(format!(
            "{} disappeared outside the native owner",
            path.display()
        ));
    }
    if current.as_ref().is_some_and(|root| root != expected) {
        return Err(format!(
            "{} changed outside the native owner",
            path.display()
        ));
    }
    if backup.exists() || current.is_none() {
        return Ok(());
    }
    let main_valid = fs::read(path)
        .ok()
        .and_then(|bytes| aim_android_xml::read(&bytes).ok())
        .is_some_and(|root| &root == expected);
    if !main_valid {
        fs::rename(sibling(path, ".reservecopy"), backup).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn finish(file: &mut File, path: &Path) -> io::Result<()> {
    file.flush()?;
    file.set_permissions(fs::Permissions::from_mode(0o660))?;
    guest_inode::record(
        path,
        GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o660),
        },
    )?;
    file.sync_all()
}

fn write_resilient(path: &Path, backup: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    write_with(path, backup, |file| file.write_all(bytes))
}

/// `startWrite`/`finishWrite`/`failWrite`. Opens both outputs before the
/// main write; a failed main write leaves the previous backup in place.
fn write_with(
    path: &Path,
    backup: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), WriteError> {
    let reserve = sibling(path, ".reservecopy");
    let start = || -> io::Result<(File, File)> {
        if path.exists() {
            if backup.exists() {
                remove(path)?;
            } else {
                fs::rename(path, backup)?;
            }
        }
        remove(&reserve)?;
        Ok((File::create(path)?, File::create(&reserve)?))
    };
    let (mut main, mut copy) = start().map_err(WriteError::before)?;
    if let Err(e) = write(&mut main).and_then(|_| finish(&mut main, path)) {
        drop(main);
        let cleanup = remove(path);
        return Err(WriteError::before(match cleanup {
            Ok(()) => e.to_string(),
            Err(cleanup) => format!("{e}; cleanup: {cleanup}"),
        }));
    }
    drop(main);
    remove(backup).map_err(WriteError::before)?;
    let result = File::open(path).and_then(|mut input| {
        io::copy(&mut input, &mut copy)?;
        finish(&mut copy, &reserve)
    });
    result.map_err(|e| WriteError {
        committed: true,
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests;
