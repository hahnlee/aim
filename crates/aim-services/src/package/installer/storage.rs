//! Exclusive native install_sessions.xml and staging owner.
//! Original android-16.0.0_r1 AtomicFile/PackageInstallerSession protocol (AOSP, Apache-2.0).
#[path = "storage/staged.rs"]
mod staged;
use super::{
    Parameters, Record, Session,
    codec::{Object, SessionParams},
};
use aim_android_xml::{Element, Node, Value, abx};
use aim_storage::guest_inode::{self, GuestInode};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
#[derive(Debug)]
pub struct Error {
    pub committed: bool,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
fn before(error: impl ToString) -> Error {
    Error {
        committed: false,
        message: error.to_string(),
    }
}
fn element(name: &str) -> Element {
    Element {
        name: name.into(),
        attrs: Vec::new(),
        content: Vec::new(),
    }
}
fn string(e: &mut Element, name: &str, value: &Option<String>) {
    if let Some(value) = value {
        e.attrs.push((name.into(), Value::String(value.clone())));
    }
}
fn int(e: &mut Element, name: &str, value: i32) {
    e.attrs.push((name.into(), Value::Int(value)));
}
fn long(e: &mut Element, name: &str, value: i64) {
    e.attrs.push((name.into(), Value::Long(value)));
}
fn boolean(e: &mut Element, name: &str, value: bool) {
    e.attrs.push((name.into(), Value::Bool(value)));
}
fn text(e: &Element, name: &str) -> Option<String> {
    e.string(name).map(|value| value.into_owned())
}
/// Only original guest paths are persisted; the native host root stays outside XML.
pub type Labeler =
    std::sync::Arc<dyn Fn(&std::path::Path, &str) -> Result<(), Error> + Send + Sync>;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Claim {
    device: u64,
    inode: u64,
    bytes: Vec<u8>,
}
pub struct Store {
    data: PathBuf,
    file_inode: GuestInode,
    stage_inode: GuestInode,
    claimed: [Option<Claim>; 3],
    labeler: Labeler,
}
impl Store {
    pub fn open(
        data: PathBuf,
        file_inode: GuestInode,
        stage_inode: GuestInode,
        labeler: Labeler,
    ) -> Result<Self, Error> {
        for inode in [&file_inode, &stage_inode] {
            if inode.uid.is_none() || inode.gid.is_none() || inode.mode.is_none() {
                return Err(before("installer creation inode metadata incomplete"));
            }
        }
        if !data.is_dir() {
            return Err(before("installer data root absent"));
        }
        let data = data.canonicalize().map_err(before)?;
        let mut store = Self {
            data,
            file_inode,
            stage_inode,
            labeler,
            claimed: std::array::from_fn(|_| None),
        };
        store.claimed = store.inspect()?;
        Ok(store)
    }
    fn paths(&self) -> [PathBuf; 3] {
        let p = self.data.join("system/install_sessions.xml");
        [
            p.clone(),
            p.with_file_name("install_sessions.xml.bak"),
            p.with_file_name("install_sessions.xml.new"),
        ]
    }
    fn inspect(&self) -> Result<[Option<Claim>; 3], Error> {
        let mut result = std::array::from_fn(|_| None);
        for (index, path) in self.paths().iter().enumerate() {
            match fs::symlink_metadata(path) {
                Ok(meta) => {
                    if !meta.is_file() {
                        return Err(before("installer state path is not a regular file"));
                    }
                    result[index] = Some(Claim {
                        device: meta.dev(),
                        inode: meta.ino(),
                        bytes: fs::read(path).map_err(before)?,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(before(error)),
            }
        }
        Ok(result)
    }
    pub fn recovered(&self) -> Result<Vec<(Session, Record)>, Error> {
        let Some(bytes) = self.claimed[1].as_ref().or(self.claimed[0].as_ref()) else {
            return Ok(Vec::new());
        };
        let root = aim_android_xml::read(&bytes.bytes).map_err(before)?;
        if root.name != "sessions" {
            return Err(before("invalid installer sessions root"));
        }
        let mut records = Vec::new();
        let mut ids = BTreeSet::new();
        for node in root.children().filter(|node| node.name == "session") {
            let id = node
                .int("sessionId")
                .map_err(before)?
                .ok_or_else(|| before("sessionId absent"))?;
            if id <= 0 || !ids.insert(id) {
                return Err(before("duplicate/invalid installer session ID"));
            }
            let user = node
                .int("userId")
                .map_err(before)?
                .ok_or_else(|| before("session user absent"))?;
            let uid = node
                .int("installerUid")
                .map_err(before)?
                .ok_or_else(|| before("installer UID owner absent"))?;
            if user < 0 || uid < 0 {
                return Err(before("negative installer user/UID"));
            }
            for flag in ["isReady", "isFailed", "isApplied"] {
                if node.bool(flag).map_err(before)?.unwrap_or(false) {
                    return Err(before(format!("recovery owner unavailable: {flag}")));
                }
            }
            if node.string("sessionStageCid").is_some() || node.string("appIcon").is_some() {
                return Err(before("external container/icon recovery owner unavailable"));
            }
            let mut params = SessionParams {
                mode: node
                    .int("mode")
                    .map_err(before)?
                    .ok_or_else(|| before("install mode absent"))?,
                install_flags: node
                    .int("installFlags")
                    .map_err(before)?
                    .ok_or_else(|| before("install flags absent"))?,
                install_location: node
                    .int("installLocation")
                    .map_err(before)?
                    .ok_or_else(|| before("install location absent"))?,
                size_bytes: node
                    .long("sizeBytes")
                    .map_err(before)?
                    .ok_or_else(|| before("install size absent"))?,
                app_package_name: text(node, "appPackageName"),
                app_label: text(node, "appLabel"),
                originating_uid: node.int("originatingUid").map_err(before)?.unwrap_or(-1),
                abi_override: text(node, "abiOverride"),
                volume_uuid: text(node, "volumeUuid"),
                install_reason: node
                    .int("installRason")
                    .map_err(before)?
                    .ok_or_else(|| before("install reason absent"))?,
                package_source: node
                    .int("packageSource")
                    .map_err(before)?
                    .ok_or_else(|| before("package source absent"))?,
                multi_package: node.bool("multiPackage").map_err(before)?.unwrap_or(false),
                staged: node.bool("stagedSession").map_err(before)?.unwrap_or(false),
                application_enabled_setting_persistent: node
                    .bool("applicationEnabledSettingPersistent")
                    .map_err(before)?
                    .unwrap_or(false),
                required_installed_version_code: -1,
                unarchive_id: -1,
                auto_install_dependencies_enabled: true,
                ..Default::default()
            };
            if node.bool("isDataLoader").map_err(before)?.unwrap_or(false) {
                params.data_loader_params = Some(
                    super::codec::DataLoader {
                        kind: node
                            .int("dataLoaderType")
                            .map_err(before)?
                            .ok_or_else(|| before("data loader type absent"))?,
                        package: text(node, "dataLoaderPackageName"),
                        class: text(node, "dataLoaderClassName"),
                        arguments: text(node, "dataLoaderArguments"),
                    }
                    .object(),
                );
            }
            params.originating_uri = text(node, "originatingUri").map(uri);
            params.referrer_uri = text(node, "referrerUri").map(uri);
            let mut children = BTreeSet::new();
            let mut grants = Vec::new();
            let mut denies = Vec::new();
            let mut legacy_grants = Vec::new();
            for child in node.children() {
                match child.name.as_str() {
                    "grant-permission" | "deny-permission" => {
                        let name = text(child, "name");
                        let permissions = if child.name == "grant-permission" {
                            &mut grants
                        } else {
                            &mut denies
                        };
                        if !permissions.contains(&name) {
                            permissions.push(name);
                        }
                    }
                    "granted-runtime-permission" => legacy_grants.push(text(child, "name")),
                    "whitelisted-restricted-permission" => params
                        .whitelisted_restricted_permissions
                        .get_or_insert_with(Vec::new)
                        .push(text(child, "name")),
                    "auto-revoke-permissions-mode" => {
                        params.auto_revoke_permissions_mode = child
                            .int("mode")
                            .map_err(before)?
                            .ok_or_else(|| before("auto-revoke mode absent"))?
                    }
                    "preVerifiedDomains"
                    | "sessionFile"
                    | "sessionChecksum"
                    | "sessionChecksumSignature" => {}
                    "childSession" => {
                        children.insert(
                            child
                                .int("sessionId")
                                .map_err(before)?
                                .ok_or_else(|| before("child ID absent"))?,
                        );
                    }
                    _ => {
                        return Err(before(format!(
                            "session child recovery owner unavailable: {}",
                            child.name
                        )));
                    }
                }
            }
            let mut set_permission = |name: Option<String>, state: i32| {
                if let Some((_, current)) = params
                    .permission_states
                    .iter_mut()
                    .find(|(key, _)| key == &name)
                {
                    *current = Some(state);
                } else {
                    params.permission_states.push((name, Some(state)));
                }
            };
            if !legacy_grants.is_empty() {
                for name in legacy_grants {
                    set_permission(name, 1);
                }
            } else {
                for name in grants {
                    set_permission(name, 1);
                }
                for name in denies {
                    set_permission(name, 2);
                }
            }
            params.permission_states.sort_by_key(|(name, _)| {
                name.as_deref().map_or(0, crate::package::info::java_hash)
            });
            let record = Record {
                params: params.clone(),
                installer_uid: uid as u32,
                user: user as u32,
                installer_package: text(node, "installerPackageName"),
                installer_attribution_tag: text(node, "installerAttributionTag"),
                created_millis: node
                    .long("createdMillis")
                    .map_err(before)?
                    .ok_or_else(|| before("created time absent"))?,
                initiating_package: text(node, "installInitiatingPackageName"),
                originating_package: text(node, "installOriginatingPackageName"),
                installer_package_uid: node
                    .int("installerPackageUid")
                    .map_err(before)?
                    .unwrap_or(-1),
            };
            let session = Session {
                id,
                installer_uid: uid as u32,
                original_installer_uid: uid as u32,
                committed: node.bool("committed").map_err(before)?.unwrap_or(false),
                committed_millis: node.long("committedMillis").map_err(before)?.unwrap_or(0),
                resolved_package: None,
                validated_target_sdk: None,
                checksums: super::checksums::Pending::read_xml(node).map_err(before)?,
                user: user as u32,
                parameters: Parameters {
                    multi_package: params.multi_package,
                    staged: params.staged,
                    install_flags: params.install_flags,
                    application_enabled_setting_persistent: params
                        .application_enabled_setting_persistent,
                },
                parent: node.int("parentSessionId").map_err(before)?.unwrap_or(-1),
                children,
                active_count: 0,
                prepared: node.bool("prepared").map_err(before)?.unwrap_or(true),
                sealed: node.bool("sealed").map_err(before)?.unwrap_or(false),
                destroyed: node.bool("destroyed").map_err(before)?.unwrap_or(false),
                client_progress: 0.0,
                reported_progress: 0.0,
                has_app_metadata: false,
                installation_files: node
                    .children()
                    .filter(|child| child.name == "sessionFile")
                    .map(|child| {
                        Ok(super::InstallationFile {
                            location: child.int("location").map_err(before)?.unwrap_or(0),
                            name: text(child, "name"),
                            length: child.long("lengthBytes").map_err(before)?.unwrap_or(-1),
                            metadata: child.bytes_base64("metadata").map_err(before)?,
                            signature: child.bytes_base64("signature").map_err(before)?,
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()?,
                pre_verified_domains: {
                    let mut domains: Vec<String> = node
                        .children()
                        .filter(|child| child.name == "preVerifiedDomains")
                        .filter_map(|child| text(child, "domain"))
                        .collect();
                    domains.sort_by_key(|domain| crate::package::info::java_hash(domain));
                    let mut unique = Vec::new();
                    for domain in domains {
                        if !unique.contains(&domain) {
                            unique.push(domain);
                        }
                    }
                    let domains = unique;
                    (!domains.is_empty()).then_some(domains)
                },
            };
            for (index, file) in session.installation_files.iter().enumerate() {
                if session.installation_files[..index].iter().any(|existing| {
                    existing.location == file.location && existing.name == file.name
                }) {
                    return Err(before("Trying to add a duplicate installation file"));
                }
            }
            if text(node, "sessionStageDir") != self.guest_stage(&session, &record) {
                return Err(before(
                    "persisted installer stage path differs from native owner",
                ));
            }
            // readFromXml restores the stage capability path and flags; stage
            // contents are verified by that session's operation/recovery owner.
            records.push((session, record));
        }
        let map: BTreeMap<_, _> = records.iter().map(|(s, _)| (s.id, s)).collect();
        for (session, _) in &records {
            if session.parent != -1
                && !map
                    .get(&session.parent)
                    .is_some_and(|parent| parent.children.contains(&session.id))
            {
                return Err(before("orphan installer parent relationship"));
            }
            for child in &session.children {
                if !map
                    .get(child)
                    .is_some_and(|child| child.parent == session.id)
                {
                    return Err(before("orphan installer child relationship"));
                }
            }
        }
        Ok(records)
    }
    fn guest_stage(&self, session: &Session, record: &Record) -> Option<String> {
        if session.parameters.multi_package {
            return None;
        }
        Some(if record.params.staged {
            format!("/data/app-staging/session_{}", session.id)
        } else {
            format!("/data/app/vmdl{}.tmp", session.id)
        })
    }
    pub(crate) fn stage_path(&self, session: &Session, record: &Record) -> Result<PathBuf, Error> {
        if record.params.volume_uuid.is_some() {
            return Err(before("adopted volume install storage owner unavailable"));
        }
        self.guest_stage(session, record)
            .map(|path| self.data.join(path.trim_start_matches("/data/")))
            .ok_or_else(|| before("multi-package session has no stage directory"))
    }
    pub fn validate_storage(&self, params: &SessionParams) -> Result<(), Error> {
        if params.volume_uuid.is_some() {
            return Err(before("adopted volume install storage owner unavailable"));
        }
        if params
            .app_icon
            .as_ref()
            .is_some_and(|icon| !icon.objects.is_empty())
        {
            return Err(before(
                "installer icon/data-loader storage owner unavailable",
            ));
        }
        if !params.multi_package {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(self.data.as_os_str().as_bytes()).map_err(before)?;
            let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
                return Err(before(std::io::Error::last_os_error()));
            }
            let available = (stats.f_bavail as u64)
                .saturating_mul(stats.f_frsize as u64)
                .saturating_sub(1 << 30);
            if params.size_bytes > 0 && params.size_bytes as u64 > available {
                return Err(before("No suitable internal storage available"));
            }
        }
        Ok(())
    }
    pub fn prepare_stage(&self, session: &Session, record: &Record) -> Result<(), Error> {
        if session.parameters.multi_package {
            return Ok(());
        }
        let path = self.stage_path(session, record)?;
        let parent = path.parent().unwrap();
        let metadata = fs::symlink_metadata(parent).map_err(before)?;
        if !metadata.is_dir() {
            return Err(before("installer stage parent is not a directory"));
        }
        fs::create_dir(&path).map_err(before)?;
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(self.stage_inode.mode.unwrap()),
        )
        .map_err(before)?;
        guest_inode::record(&path, self.stage_inode).map_err(before)?;
        (self.labeler)(&path, &self.guest_stage(session, record).unwrap())?;
        Ok(())
    }
    pub fn write_target(
        &self,
        session: &Session,
        record: &Record,
        name: &str,
        offset: i64,
    ) -> Result<std::fs::File, Error> {
        if !valid_filename(name) {
            return Err(before("invalid session filename"));
        }
        let path = self.stage_path(session, record)?.join(name);
        let mode = if name == "app.metadata" { 0o640 } else { 0o644 };
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(mode | 0o600)
            .open(&path)
            .map_err(before)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode | 0o600)).map_err(before)?;
        guest_inode::record(
            &path,
            GuestInode {
                mode: Some(mode),
                ..self.stage_inode
            },
        )
        .map_err(before)?;
        (self.labeler)(
            &path,
            &format!("{}/{}", self.guest_stage(session, record).unwrap(), name),
        )?;
        if offset > 0 {
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(offset as u64)).map_err(before)?;
        }
        Ok(file)
    }
    pub fn fetch_package_name(
        &self,
        session: &Session,
        record: &Record,
        policy: &super::native::LitePolicy,
    ) -> Result<String, Error> {
        let path = self.stage_path(session, record)?;
        let names = if record.params.data_loader_params.is_some() {
            session
                .installation_files
                .iter()
                .map(|file| file.name.clone())
                .collect()
        } else {
            self.names(session, record)?
        };
        for name in names {
            let name = name.ok_or_else(|| before("null installation filename"))?;
            let file = path.join(&name);
            if file.is_dir()
                || name.ends_with(".removed")
                || name.ends_with(".idsig")
                || name.ends_with("app.metadata")
                || name.ends_with(".digests")
                || name.ends_with(".digests.signature")
                || policy
                    .art_managed_extensions
                    .iter()
                    .any(|extension| name.ends_with(extension))
            {
                continue;
            }
            return crate::package::parse::lite::package_name(&file, &policy.environment)
                .map_err(before);
        }
        Err(before(format!(
            "Can't fetch package name for session={}",
            session.id
        )))
    }
    pub fn names(&self, session: &Session, record: &Record) -> Result<Vec<Option<String>>, Error> {
        if session.parameters.multi_package {
            return Ok(Vec::new());
        }
        let path = self.stage_path(session, record)?;
        let mut names = Vec::new();
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    let name = entry
                        .map_err(before)?
                        .file_name()
                        .to_string_lossy()
                        .into_owned();
                    if name != "app.metadata" {
                        names.push(Some(name));
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(error) => return Err(before(error)),
        }
        Ok(names)
    }
    pub fn read_file(
        &self,
        session: &Session,
        record: &Record,
        name: Option<&str>,
    ) -> Result<std::fs::File, Error> {
        let name = name
            .filter(|name| valid_filename(name))
            .ok_or_else(|| before("invalid session filename"))?;
        let path = self.stage_path(session, record)?.join(name);
        if guest_inode::read(&path)
            .map_err(before)?
            .and_then(|inode| inode.mode)
            .is_some_and(|mode| mode & 0o444 == 0)
        {
            return Err(before(std::io::Error::from_raw_os_error(libc::EACCES)));
        }
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(before)
    }
    pub fn remove_metadata(&self, session: &Session, record: &Record) -> Result<(), Error> {
        fs::remove_file(self.stage_path(session, record)?.join("app.metadata")).map_err(before)
    }
    pub fn remove_split(
        &self,
        session: &Session,
        record: &Record,
        name: Option<&str>,
    ) -> Result<(), Error> {
        let marker = format!("{}.removed", name.unwrap_or("null"));
        if !valid_filename(&marker) {
            return Err(before("invalid split marker"));
        }
        let path = self.stage_path(session, record)?.join(marker);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(before(error)),
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(before)?;
        guest_inode::record(
            &path,
            GuestInode {
                mode: Some(0),
                ..self.stage_inode
            },
        )
        .map_err(before)?;
        (self.labeler)(
            &path,
            &format!(
                "{}/{}",
                self.guest_stage(session, record).unwrap(),
                path.file_name().unwrap().to_string_lossy()
            ),
        )?;
        Ok(())
    }
    pub fn recovered_icons(&self) -> Result<Vec<(i32, Vec<u8>)>, Error> {
        let directory = self.data.join("system/install_sessions");
        let mut icons = Vec::new();
        match fs::read_dir(directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(before)?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| before("Invalid installer icon filename"))?;
                    if let Some(id) = name
                        .strip_prefix("app_icon.")
                        .and_then(|name| name.strip_suffix(".png"))
                        .and_then(|id| id.parse::<i32>().ok())
                    {
                        let file = OpenOptions::new()
                            .read(true)
                            .custom_flags(libc::O_NOFOLLOW)
                            .open(entry.path())
                            .map_err(before)?;
                        use std::io::Read;
                        let mut bytes = Vec::new();
                        file.take(16 * 1024 * 1024)
                            .read_to_end(&mut bytes)
                            .map_err(before)?;
                        icons.push((id, bytes));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(before(error)),
        }
        Ok(icons)
    }
    pub fn write_icon(&self, id: i32, png: Option<&[u8]>) -> Result<(), Error> {
        let directory = self.data.join("system/install_sessions");
        fs::create_dir_all(&directory).map_err(before)?;
        let path = directory.join(format!("app_icon.{id}.png"));
        if let Some(png) = png {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o640)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(before)?;
            record_inode(
                &path,
                &GuestInode {
                    mode: Some(0o100640),
                    ..self.file_inode
                },
            )
            .map_err(before)?;
            (self.labeler)(
                &path,
                &format!("/data/system/install_sessions/app_icon.{id}.png"),
            )?;
            file.write_all(png).map_err(before)?;
            file.sync_all().map_err(before)?;
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(before(error)),
            }
        }
        Ok(())
    }
    pub fn staging_directories(&self) -> Result<Vec<String>, Error> {
        let mut stages = Vec::new();
        for (relative, all) in [("app", false), ("app-staging", true)] {
            match fs::read_dir(self.data.join(relative)) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry.map_err(before)?;
                        let name = entry
                            .file_name()
                            .into_string()
                            .map_err(|_| before("Invalid staging directory name"))?;
                        if all || name.starts_with("vmdl") && name.ends_with(".tmp") {
                            stages.push(format!("/data/{relative}/{name}"));
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(before(error)),
            }
        }
        Ok(stages)
    }
    pub fn remove_staging_directory(&self, guest: &str) -> Result<(), Error> {
        let valid = guest.strip_prefix("/data/app/").is_some_and(|name| {
            name.starts_with("vmdl") && name.ends_with(".tmp") && valid_filename(name)
        }) || guest
            .strip_prefix("/data/app-staging/")
            .is_some_and(valid_filename);
        if !valid {
            return Err(before("Unowned staging removal path"));
        }
        let path = self.data.join(guest.strip_prefix("/data/").unwrap());
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(before),
            Ok(_) => fs::remove_file(path).map_err(before),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(before(error)),
        }
    }
    pub fn inherit_existing(
        &self,
        session: &Session,
        record: &Record,
        existing: &crate::package::pkg::AndroidPackage,
        files: &crate::package::write::Files,
        policy: &super::native::LitePolicy,
    ) -> Result<(), Error> {
        let stage = self.stage_path(session, record)?;
        let mut replacements = std::collections::BTreeSet::new();
        for entry in fs::read_dir(&stage).map_err(before)? {
            let entry = entry.map_err(before)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| before("Invalid staged filename"))?;
            if entry.path().is_dir()
                || name.ends_with(".removed")
                || name.ends_with(".idsig")
                || name.ends_with("app.metadata")
                || name.ends_with(".digests")
                || name.ends_with(".digests.signature")
                || policy
                    .art_managed_extensions
                    .iter()
                    .any(|extension| name.ends_with(extension))
            {
                continue;
            }
            replacements.insert(
                crate::package::parse::lite::split_name(&entry.path(), &policy.environment)
                    .map_err(before)?,
            );
        }
        let mut inherited = vec![(
            None,
            existing
                .base_apk_path
                .clone()
                .ok_or_else(|| before("Existing base APK unavailable"))?,
        )];
        let names = existing.split_names.as_deref().unwrap_or_default();
        let paths = existing.split_code_paths.as_deref().unwrap_or_default();
        if names.len() != paths.len() {
            return Err(before("Existing split names and paths differ"));
        }
        for (name, path) in names.iter().zip(paths) {
            inherited.push((
                Some(
                    name.clone()
                        .ok_or_else(|| before("Null existing split name"))?,
                ),
                path.clone()
                    .ok_or_else(|| before("Null existing split path"))?,
            ));
        }
        for (split, path) in inherited {
            if replacements.contains(&split)
                || split
                    .as_ref()
                    .is_some_and(|split| stage.join(format!("{split}.removed")).exists())
            {
                continue;
            }
            let name =
                split.map_or_else(|| "base.apk".into(), |split| format!("split_{split}.apk"));
            let host = files(&path).ok_or_else(|| before("Existing APK VFS owner unavailable"))?;
            let mut source = fs::File::open(host).map_err(before)?;
            let mut target = self.write_target(session, record, &name, 0)?;
            target.set_len(0).map_err(before)?;
            std::io::copy(&mut source, &mut target).map_err(before)?;
            target.sync_all().map_err(before)?;
        }
        Ok(())
    }
    pub fn normalize_apks(
        &self,
        session: &Session,
        record: &Record,
        policy: &super::native::LitePolicy,
        pending: &mut super::checksums::Pending,
        files: &super::hardlink::Files,
    ) -> Result<(), Error> {
        let stage = self.stage_path(session, record)?;
        let mut inputs = Vec::new();
        for entry in fs::read_dir(&stage).map_err(before)? {
            let entry = entry.map_err(before)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| before("Invalid staged file name"))?;
            if entry.path().is_dir()
                || name.ends_with(".removed")
                || name.ends_with(".idsig")
                || name.ends_with("app.metadata")
                || name.ends_with(".digests")
                || name.ends_with(".digests.signature")
                || policy
                    .art_managed_extensions
                    .iter()
                    .any(|extension| name.ends_with(extension))
            {
                continue;
            }
            let split = crate::package::parse::lite::split_name(&entry.path(), &policy.environment)
                .map_err(before)?;
            let target =
                split.map_or_else(|| "base.apk".into(), |split| format!("split_{split}.apk"));
            if !valid_filename(&target) {
                return Err(before("Invalid normalized APK filename"));
            }
            if inputs
                .iter()
                .any(|(_, old_target): &(String, String)| old_target == &target)
            {
                return Err(before("Duplicate staged APK split"));
            }
            inputs.push((name, target));
        }
        // Move every source aside first so a swap of incoming filenames cannot
        // overwrite another accepted APK. The stage remains sealed throughout.
        let mut staged = Vec::new();
        for (index, (name, target)) in inputs.into_iter().enumerate() {
            let temporary = format!(".native-apk-{}-{index}", session.id);
            let temporary_path = stage.join(&temporary);
            if temporary_path.exists() {
                return Err(before("APK normalization temporary path already exists"));
            }
            fs::rename(stage.join(&name), &temporary_path).map_err(before)?;
            staged.push((name, target, temporary_path));
        }
        for (name, target, temporary) in staged {
            fs::rename(&temporary, stage.join(&target)).map_err(before)?;
            pending
                .stage_apk(&name, &target, session, record, self, files)
                .map_err(|error| before(format!("Checksum staging: {error:?}")))?;
            let old_signature = stage.join(format!("{name}.idsig"));
            if old_signature.exists() && name != target {
                fs::rename(old_signature, stage.join(format!("{target}.idsig"))).map_err(before)?;
            }
        }
        pending
            .require_consumed()
            .map_err(|error| before(format!("Invalid remaining checksum entries: {error:?}")))
    }
    pub fn validation_path(&self, session: &Session, record: &Record) -> Result<String, Error> {
        self.stage_path(session, record)?;
        self.guest_stage(session, record)
            .ok_or_else(|| before("Multi-package parent has no APK directory"))
    }
    pub fn resolved_path(
        &self,
        session: &Session,
        record: &Record,
    ) -> Result<Option<String>, Error> {
        if !session.prepared || session.parameters.multi_package {
            return Ok(None);
        }
        let path = self.stage_path(session, record)?;
        for entry in fs::read_dir(path).map_err(before)? {
            let entry = entry.map_err(before)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.ends_with(".apk") && entry.file_type().map_err(before)?.is_file() {
                return Ok(Some(format!(
                    "{}/{}",
                    self.guest_stage(session, record).unwrap(),
                    name
                )));
            }
        }
        Ok(None)
    }
    pub fn abandon(&self, records: &[(Session, Record)]) -> Result<(), Error> {
        for (session, record) in records {
            if session.parameters.multi_package {
                continue;
            }
            let path = self.stage_path(session, record)?;
            if path.exists() {
                let meta = fs::symlink_metadata(&path).map_err(before)?;
                if !meta.is_dir() {
                    return Err(before("installer stage changed type"));
                }
                fs::remove_dir_all(&path).map_err(before)?;
            }
        }
        Ok(())
    }
    pub fn write(&mut self, records: &[(Session, Record)]) -> Result<(), Error> {
        if self.inspect()? != self.claimed {
            return Err(before("installer state changed outside exclusive owner"));
        }
        let states = self.staged_states()?;
        let mut root = element("sessions");
        for (session, record) in records {
            if session.destroyed && !session.parameters.staged {
                continue;
            }
            let mut node = self.session_document(session, record)?;
            if let Some(status) = states.get(&session.id) {
                node.attrs.retain(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "isReady" | "isFailed" | "isApplied" | "errorCode" | "errorMessage"
                    )
                });
                boolean(&mut node, "isReady", status.ready);
                boolean(&mut node, "isFailed", status.failed);
                boolean(&mut node, "isApplied", status.applied);
                int(&mut node, "errorCode", status.error_code);
                string(&mut node, "errorMessage", &status.error_message);
            }
            root.content.push(Node::Element(node));
        }
        let bytes = abx::write(&root).map_err(before)?;
        let [main, backup, new] = self.paths();
        if backup.exists() {
            fs::rename(&backup, &main).map_err(before)?;
        }
        let mut committed = false;
        let result = (|| -> Result<(), Error> {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(self.file_inode.mode.unwrap())
                .open(&new)
                .map_err(before)?;
            fs::set_permissions(
                &new,
                fs::Permissions::from_mode(self.file_inode.mode.unwrap()),
            )
            .map_err(before)?;
            guest_inode::record(&new, self.file_inode).map_err(before)?;
            (self.labeler)(&new, "/data/system/install_sessions.xml.new")?;
            file.write_all(&bytes).map_err(before)?;
            file.sync_all().map_err(before)?;
            fs::rename(&new, &main).map_err(before)?;
            committed = true;
            Ok(())
        })();
        if result.is_err() && new.exists() {
            fs::remove_file(&new).map_err(before)?;
        }
        self.claimed = self.inspect().map_err(|error| Error {
            committed,
            message: format!("{}; write result: {result:?}", error.message),
        })?;
        result
    }
    fn session_document(&self, session: &Session, record: &Record) -> Result<Element, Error> {
        self.validate_storage(&record.params)?;
        let mut node = element("session");
        let p = &record.params;
        for (name, value) in [
            ("sessionId", session.id),
            ("userId", session.user as i32),
            ("installerPackageUid", record.installer_package_uid),
            ("installerUid", record.installer_uid as i32),
            ("parentSessionId", session.parent),
            ("mode", p.mode),
            ("installFlags", p.install_flags),
            ("installLocation", p.install_location),
            ("originatingUid", p.originating_uid),
            ("installRason", p.install_reason),
            ("packageSource", p.package_source),
            ("errorCode", 0),
        ] {
            int(&mut node, name, value)
        }
        for (name, value) in [
            ("createdMillis", record.created_millis),
            ("updatedMillis", 0),
            ("committedMillis", session.committed_millis),
            ("sizeBytes", p.size_bytes),
        ] {
            long(&mut node, name, value)
        }
        for (name, value) in [
            ("prepared", session.prepared),
            ("committed", session.committed),
            ("destroyed", session.destroyed),
            ("sealed", session.sealed),
            ("multiPackage", p.multi_package),
            ("stagedSession", p.staged),
            ("isReady", false),
            ("isFailed", false),
            ("isApplied", false),
            ("isDataLoader", p.data_loader_params.is_some()),
            (
                "applicationEnabledSettingPersistent",
                p.application_enabled_setting_persistent,
            ),
        ] {
            boolean(&mut node, name, value)
        }
        for (name, value) in [
            ("installerPackageName", &record.installer_package),
            ("updateOwnererPackageName", &record.installer_package),
            ("installerAttributionTag", &record.installer_attribution_tag),
            ("installInitiatingPackageName", &record.initiating_package),
            ("installOriginatingPackageName", &record.originating_package),
            ("appPackageName", &p.app_package_name),
            ("appLabel", &p.app_label),
            ("abiOverride", &p.abi_override),
            ("volumeUuid", &p.volume_uuid),
        ] {
            string(&mut node, name, value)
        }
        string(&mut node, "errorMessage", &Some(String::new()));
        string(
            &mut node,
            "sessionStageDir",
            &self.guest_stage(session, record),
        );
        for (name, value) in [
            ("originatingUri", &p.originating_uri),
            ("referrerUri", &p.referrer_uri),
        ] {
            if let Some(object) = value {
                let mut reader =
                    aim_binder_host::parcel::Reader::new(&object.bytes, &object.objects);
                reader.read_string16().map_err(before)?;
                let value = crate::package::uri::Uri::read(
                    &mut reader,
                    &mut crate::package::intent_filter::Plain,
                )
                .map_err(before)?
                .ok_or_else(|| before("null URI payload"))?;
                string(&mut node, name, &Some(value.as_str().into()));
            }
        }
        for (name, state) in &p.permission_states {
            let mut child = element(if *state == Some(1) {
                "grant-permission"
            } else {
                "deny-permission"
            });
            string(&mut child, "name", name);
            node.content.push(Node::Element(child));
        }
        if let Some(permissions) = &p.whitelisted_restricted_permissions {
            for name in permissions {
                let mut child = element("whitelisted-restricted-permission");
                string(&mut child, "name", name);
                node.content.push(Node::Element(child));
            }
        }
        let mut child = element("auto-revoke-permissions-mode");
        int(&mut child, "mode", p.auto_revoke_permissions_mode);
        node.content.push(Node::Element(child));
        session.checksums.append_xml(&mut node);
        if let Some(loader) = &p.data_loader_params {
            let loader = super::codec::DataLoader::from_object(loader)
                .map_err(|e| before(format!("data loader parcel: {e}")))?;
            int(&mut node, "dataLoaderType", loader.kind);
            string(&mut node, "dataLoaderPackageName", &loader.package);
            string(&mut node, "dataLoaderClassName", &loader.class);
            string(&mut node, "dataLoaderArguments", &loader.arguments);
        }
        for file in &session.installation_files {
            let mut child = element("sessionFile");
            int(&mut child, "location", file.location);
            string(&mut child, "name", &file.name);
            long(&mut child, "lengthBytes", file.length);
            if let Some(value) = &file.metadata {
                child
                    .attrs
                    .push(("metadata".into(), Value::BytesBase64(value.clone())));
            }
            if let Some(value) = &file.signature {
                child
                    .attrs
                    .push(("signature".into(), Value::BytesBase64(value.clone())));
            }
            node.content.push(Node::Element(child));
        }
        if let Some(domains) = &session.pre_verified_domains {
            for domain in domains {
                let mut child = element("preVerifiedDomains");
                string(&mut child, "domain", &Some(domain.clone()));
                node.content.push(Node::Element(child));
            }
        }
        for id in &session.children {
            let mut child = element("childSession");
            int(&mut child, "sessionId", *id);
            node.content.push(Node::Element(child));
        }
        Ok(node)
    }
}
fn uri(value: String) -> Object {
    let mut p = aim_binder_host::parcel::Parcel::new();
    p.write_string16(Some("android.net.Uri$StringUri"));
    p.write_i32(1);
    p.write_string8(Some(&value));
    Object {
        bytes: p.data().to_vec(),
        objects: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    struct Data(PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-installer-storage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("system")).unwrap();
            Self(path)
        }
        fn main(&self) -> PathBuf {
            self.0.join("system/install_sessions.xml")
        }
        fn open(&self, labeler: Labeler) -> Store {
            let inode = GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o600),
            };
            Store::open(
                self.0.clone(),
                inode,
                GuestInode {
                    mode: Some(0o775),
                    ..inode
                },
                labeler,
            )
            .unwrap()
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn label(path: &std::path::Path, _: &str) -> Result<(), Error> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(before)?;
        let value = b"u:object_r:system_data_file:s0\0";
        if unsafe {
            libc::setxattr(
                path.as_ptr(),
                c"dev.aim.xattr.security.selinux".as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        } != 0
        {
            return Err(before(std::io::Error::last_os_error()));
        }
        Ok(())
    }
    fn persisted_prepared_session(stage: &str) -> String {
        format!("<sessions><session sessionId='7' userId='0' installerUid='10100' createdMillis='1' mode='1' installFlags='16' installLocation='1' sizeBytes='-1' installRason='0' packageSource='0' prepared='true' sessionStageDir='{stage}'/></sessions>")
    }
    #[test]
    fn prepared_missing_stage_recovers_without_creating_files_and_operations_fail() {
        let data = Data::new();
        fs::create_dir(data.0.join("app")).unwrap();
        let xml = persisted_prepared_session("/data/app/vmdl7.tmp");
        fs::write(data.main(), &xml).unwrap();
        let store = data.open(Arc::new(label));
        let records = store.recovered().unwrap();
        assert_eq!(records.len(), 1);
        let (session, record) = &records[0];
        assert!(session.prepared);
        assert_eq!(session.installer_uid, 10100);
        assert_eq!(
            store.guest_stage(session, record).as_deref(),
            Some("/data/app/vmdl7.tmp")
        );
        let stage = data.0.join("app/vmdl7.tmp");
        assert!(!stage.exists());
        let missing = std::io::Error::from_raw_os_error(libc::ENOENT).to_string();
        for error in [
            store.write_target(session, record, "base.apk", 0).unwrap_err(),
            store.resolved_path(session, record).unwrap_err(),
        ] {
            assert!(!error.committed);
            assert_eq!(error.message, missing);
        }
        assert!(!stage.exists());
        assert_eq!(fs::read_to_string(data.main()).unwrap(), xml);
    }
    #[test]
    fn recovery_rejects_foreign_and_non_normalized_stage_paths() {
        for stage in [
            "/data/app/vmdl8.tmp",
            "/data/app/../app/vmdl7.tmp",
            "/data/local/tmp/vmdl7.tmp",
            "/data/app/vmdl7.tmp/",
        ] {
            let data = Data::new();
            fs::write(data.main(), persisted_prepared_session(stage)).unwrap();
            let store = data.open(Arc::new(label));
            let error = store.recovered().err().unwrap();
            assert!(!error.committed);
            assert!(error.message.contains("stage path differs from native owner"));
        }
    }
    #[test]
    fn recovery_state_file_cannot_be_a_symlink() {
        let data = Data::new();
        let other = data.0.join("other.xml");
        fs::write(&other, persisted_prepared_session("/data/app/vmdl7.tmp")).unwrap();
        std::os::unix::fs::symlink(&other, data.main()).unwrap();
        let inode = GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        };
        let error = Store::open(data.0.clone(), inode, inode, Arc::new(label))
            .err()
            .unwrap();
        assert!(!error.committed);
        assert!(error.message.contains("not a regular file"));
    }
    #[test]
    fn same_bytes_replacement_inode_is_not_the_exclusive_owner() {
        let data = Data::new();
        let bytes = b"<sessions/>";
        fs::write(data.main(), bytes).unwrap();
        let mut store = data.open(Arc::new(label));
        let old = fs::metadata(data.main()).unwrap().ino();
        let replacement = data.0.join("system/other");
        fs::write(&replacement, bytes).unwrap();
        fs::rename(replacement, data.main()).unwrap();
        assert_ne!(fs::metadata(data.main()).unwrap().ino(), old);
        let error = store.write(&[]).unwrap_err();
        assert!(!error.committed);
        assert!(error.message.contains("outside exclusive owner"));
        assert_eq!(fs::read(data.main()).unwrap(), bytes);
    }
    #[test]
    fn post_rename_inspection_failure_reports_actual_committed_main() {
        let data = Data::new();
        let mut store = data.open(Arc::new(|path, guest| {
            label(path, guest)?;
            fs::create_dir(path.with_file_name("install_sessions.xml.bak")).map_err(before)
        }));
        let error = store.write(&[]).unwrap_err();
        assert!(error.committed);
        assert!(error.message.contains("not a regular file"));
        let bytes = fs::read(data.main()).unwrap();
        assert_eq!(aim_android_xml::read(&bytes).unwrap().name, "sessions");
        assert!(
            !data
                .main()
                .with_file_name("install_sessions.xml.new")
                .exists()
        );
    }
    #[test]
    fn xml_denials_win_independent_of_child_order_and_legacy_grants_take_priority() {
        for (children, expected) in [
            ("<deny-permission name='p'/><grant-permission name='p'/>", 2),
            ("<grant-permission name='p'/><deny-permission name='p'/>", 2),
            (
                "<deny-permission name='p'/><granted-runtime-permission name='p'/>",
                1,
            ),
        ] {
            let data = Data::new();
            fs::write(data.main(),format!("<sessions><session sessionId='7' userId='0' installerUid='10100' createdMillis='1' mode='1' installFlags='16' installLocation='1' sizeBytes='-1' installRason='0' packageSource='0' prepared='false' sessionStageDir='/data/app/vmdl7.tmp'>{children}</session></sessions>")).unwrap();
            let store = data.open(Arc::new(label));
            let records = store.recovered().unwrap();
            assert_eq!(
                records[0].1.params.permission_states,
                vec![(Some("p".into()), Some(expected))]
            );
        }
    }
}

pub(crate) fn valid_filename(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['\0', '/'])
        && name.len() <= 255
}

fn record_inode(path: &Path, inode: &GuestInode) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = inode.mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))?;
    }
    guest_inode::record(path, *inode)
}
