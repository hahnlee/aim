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

use std::collections::BTreeMap;
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
pub mod shared_users;
mod signing;

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

/// The persisted state and its complete restriction documents. Access
/// checks and broadcasts belong to the service using the owner.
pub struct Store {
    data: PathBuf,
    state: State,
    restrictions: BTreeMap<u32, Element>,
    settings_document: Element,
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
        Ok(Some(Self {
            data: data.to_owned(),
            state,
            restrictions,
            settings_document,
        }))
    }

    pub fn state(&self) -> &State {
        &self.state
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
