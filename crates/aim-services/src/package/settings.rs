//! PackageManager's settings, `/data/system/packages.xml`: every package
//! with its code path, flags, app id, install source and signatures, the
//! updated system packages' originals, shared users, the key sets, the
//! legacy permission definitions and the settings' versions, as
//! `Settings.readSettingsLPw` reads them (`writeLPr` writes them).
//!
//! Legacy `<perms>` restore lives in owner::legacy_permissions.
//! Not modelled yet: what only older platforms write (per-package
//! `<enabled-components>`, `<disabled-components>`,
//! the single-user preferred activities), which
//! the original reads only to migrate.

use aim_android_xml::Element;

use super::domain_verification;
use super::{children, string};

mod signatures;
mod key_sets;
mod verifier;
pub use super::owner::recovery::ReadError;
pub use signatures::SignatureReader;

/// `ApplicationInfo.FLAG_SYSTEM`.
pub const FLAG_SYSTEM: i32 = 1 << 0;
/// `ApplicationInfo.PRIVATE_FLAG_PRIVILEGED`.
pub const PRIVATE_FLAG_PRIVILEGED: i32 = 1 << 3;
/// `Process.INVALID_UID`.
const INVALID_UID: i32 = -1;
/// `ApplicationInfo.CATEGORY_UNDEFINED`.
const CATEGORY_UNDEFINED: i32 = -1;
/// `SigningDetails.SignatureSchemeVersion.UNKNOWN`.
const SCHEME_UNKNOWN: i32 = 0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// `mVersion`, by volume (`None`: internal storage).
    pub versions: Vec<Version>,
    /// The package verifier's device identity, once one was asked for.
    pub verifier: Option<String>,
    pub permission_trees: Vec<Permission>,
    pub permissions: Vec<Permission>,
    pub packages: Vec<Package>,
    /// `mDisabledSysPackages`: the system packages an update replaces.
    pub disabled_system_packages: Vec<Package>,
    pub shared_users: Vec<SharedUser>,
    /// `mRenamedPackages`: new name, old name.
    pub renamed_packages: Vec<(String, String)>,
    pub key_sets: KeySets,
    pub domain_verification: domain_verification::State,
    /// Imported IntentFilterVerificationInfo statuses, keyed by the Settings owner
    /// name (not the child XML packageName). Only used for legacy migration.
    pub legacy_domain_info: std::collections::BTreeMap<String, i32>,
}

/// `Settings.VersionInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Version {
    pub volume_uuid: Option<String>,
    pub sdk_version: i32,
    pub database_version: i32,
    pub build_fingerprint: Option<String>,
    pub fingerprint: Option<String>,
}

/// A permission definition (`LegacyPermission`; `AccessCheckingService`
/// keeps its own in `access.abx`).
#[derive(Clone, Debug, PartialEq)]
pub struct Permission {
    pub name: String,
    pub package: String,
    pub owner: PermissionOwner,
    /// `PermissionInfo.protectionLevel`, as `fixProtectionLevel` leaves it.
    pub protection_level: i32,
    /// Dynamic XML updates icon/label even on a retained configured owner.
    pub dynamic: Option<(i32, Option<String>)>,
}

/// LegacyPermission's immutable type and configured UID/GIDs.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionOwner {
    Manifest,
    Config { uid: i32, gids: Vec<i32> },
    Dynamic,
}

/// A `PackageSetting`: a `<package>`, or an `<updated-package>` (the
/// system package an update replaced), whose flags the reader derives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Package {
    pub transient: super::owner::transient::State,
    pub name: String,
    pub real_name: Option<String>,
    pub code_path: String,
    pub legacy_native_library_path: Option<String>,
    pub primary_cpu_abi: Option<String>,
    pub secondary_cpu_abi: Option<String>,
    pub cpu_abi_override: Option<String>,
    /// `ApplicationInfo.flags` and `privateFlags` of the package.
    pub flags: i32,
    pub private_flags: i32,
    /// `ft`, the code's modification time, and `ut`, in ms.
    pub last_modified_time: i64,
    pub last_update_time: i64,
    /// `it`, the first install time older platforms kept per package; the
    /// fallback for a user state without its own.
    pub legacy_first_install_time: i64,
    pub version_code: i64,
    pub target_sdk_version: i32,
    pub restrict_update_hash: Option<Vec<u8>>,
    pub scanned_as_stopped_system_app: bool,
    pub app_id: i32,
    /// Whether the setting owns a shared-user relationship.
    pub shared_user: bool,
    /// Separate runtime group ID when it differs from the persisted app ID.
    pub shared_user_app_id: Option<i32>,
    pub is_sdk_library: bool,
    pub install_source: InstallSource,
    pub volume_uuid: Option<String>,
    pub category_hint: i32,
    pub update_available: bool,
    pub force_queryable: bool,
    pub pending_restore: bool,
    pub debuggable: bool,
    /// Current PackageSetting bit; None means an imported owner is unresolved.
    /// This is not persisted in packages.xml or inferred from UID membership.
    pub leaving_shared_user: Option<bool>,
    /// Runtime LinkedHashSet<File>; null and allocated-empty are distinct.
    pub old_paths: Option<Vec<Option<String>>>,
    pub base_revision_code: i32,
    pub page_size_compat: i32,
    pub loading_progress: f32,
    pub loading_completed_time: i64,
    /// The domain verification set id (a UUID); `None` for an updated
    /// system package, which has the disabled id.
    pub domain_set_id: Option<String>,
    pub app_metadata_file_path: Option<String>,
    pub app_metadata_source: i32,
    pub uses_sdk_libraries: Vec<UsesSdkLibrary>,
    /// Static shared libraries used: name and version.
    pub uses_static_libraries: Vec<(String, i64)>,
    pub signatures: Option<Signatures>,
    pub key_set_data: KeySetData,
    /// MIME groups and their types.
    pub mime_groups: Vec<(Option<String>, Vec<Option<String>>)>,
    /// Split names and revision codes, kept for a package without code
    /// (archived, or deleted keeping its data).
    pub split_versions: Vec<(String, i32)>,
}

impl Package {
    /// packages.xml uses one sharedUserId for both IDs; initial APEX scans
    /// later change only appId to INVALID_UID, leaving the group relationship.
    pub fn shared_app_id(&self) -> Option<i32> {
        self.shared_user
            .then(|| self.shared_user_app_id.unwrap_or(self.app_id))
    }

    pub fn uid_owner_id(&self) -> i32 {
        self.shared_app_id().unwrap_or(self.app_id)
    }

    /// PackageSetting.addMimeTypes: existing groups accumulate distinct types.
    pub fn add_mime_types(&mut self, name: String, values: impl IntoIterator<Item = String>) {
        self.add_nullable_mime_types(Some(name), values.into_iter().map(Some));
    }

    pub fn add_nullable_mime_types(
        &mut self,
        name: Option<String>,
        values: impl IntoIterator<Item = Option<String>>,
    ) {
        let at = match self
            .mime_groups
            .iter()
            .position(|(group, _)| group == &name)
        {
            Some(at) => at,
            None => {
                self.mime_groups.push((name, Vec::new()));
                self.mime_groups.len() - 1
            }
        };
        let types = &mut self.mime_groups[at].1;
        for value in values {
            if !types.contains(&value) {
                types.push(value);
            }
        }
        types.sort_by_key(|value| value.as_deref().map_or(0, string_hash));
        self.mime_groups
            .sort_by_key(|(name, _)| name.as_deref().map_or(0, string_hash));
    }

    /// Consume an already registered package's body without rolling back earlier
    /// mutations. The caller supplies the legacy permission/domain owners (#914).
    pub fn read_children(
        &mut self,
        reader: &mut aim_android_xml::pull::Reader<'_>,
        signatures: &mut SignatureReader,
        key_set_refs: &mut std::collections::BTreeMap<i64, i32>,
        mut read_owner: impl FnMut(
            &mut Self,
            &mut aim_android_xml::pull::Reader<'_>,
            &Element,
        ) -> Result<bool, ReadError>,
    ) -> Result<(), ReadError> {
        use aim_android_xml::pull::Event;
        let outer = reader.depth();
        loop {
            match reader.next()? {
                Event::Start(child) => {
                    if !self.read_child(reader, &child, signatures, key_set_refs)?
                        && !read_owner(self, reader, &child)?
                    {
                        signatures::skip(reader)?;
                    }
                }
                Event::End(_) if reader.depth() <= outer => return Ok(()),
                Event::EndDocument => return Ok(()),
                _ => {}
            }
        }
    }

    /// Apply a package child at its start event. The caller owns registration,
    /// legacy permission/domain state and unrecognized children (#914).
    /// Keyset tags leave nested events visible; libraries/splits consume theirs.
    pub fn read_child(
        &mut self,
        reader: &mut aim_android_xml::pull::Reader<'_>,
        child: &Element,
        signatures: &mut SignatureReader,
        key_set_refs: &mut std::collections::BTreeMap<i64, i32>,
    ) -> Result<bool, String> {
        if libraries(self, child)? {
            signatures::skip(reader)?;
            return Ok(true);
        }
        match child.name.as_str() {
            "sigs" => {
                signatures.read(reader, child, &mut self.signatures)?;
            }
            "install-initiator-sigs" => {
                let mut target = None;
                signatures.read(reader, child, &mut target)?;
                self.install_source.initiating_package_signatures = target;
            }
            "proper-signing-keyset" | "defined-keyset" => {
                let id = identifier(child)?;
                let count = key_set_refs.entry(id).or_default();
                *count = count.wrapping_add(1);
                if child.name == "proper-signing-keyset" {
                    self.key_set_data.proper_signing_key_set = id;
                } else {
                    self.key_set_data
                        .add_defined_key_set(id, string(child, "alias"));
                }
            }
            "upgrade-keyset" => self.key_set_data.add_upgrade_key_set(identifier(child)?),
            "signing-keyset" => {}
            "mime-group" => {
                if let Some(group) = string(child, "name") {
                    let types = read_mime_group(reader)?;
                    self.add_mime_types(group, types);
                } else {
                    signatures::skip(reader)?;
                }
            }
            "split-version" => {
                self.read_split_version(child);
                signatures::skip(reader)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn read_split_version(&mut self, child: &Element) {
        let revision = defaulted(child.int("version"), -1);
        if let Some(name) = string(child, "name").filter(|_| revision >= 0) {
            match self.split_versions.iter_mut().find(|(n, _)| *n == name) {
                Some(split) => split.1 = revision,
                None => self.split_versions.push((name, revision)),
            }
        }
    }

    pub fn is_loading(&self) -> bool {
        (1.0f32 - self.loading_progress).abs() >= 0.00000001f32
    }

    /// PackageSetting only accepts increases; NaN comparisons do not update.
    pub fn set_loading_progress(&mut self, progress: f32) {
        if self.loading_progress < progress {
            self.loading_progress = progress;
        }
    }

    pub fn add_old_path(&mut self, path: Option<&str>) {
        let path = path.map(file_path);
        let paths = self.old_paths.get_or_insert_with(Vec::new);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    pub fn remove_old_path(&mut self, path: Option<&str>) {
        if let Some(path) = path
            && let Some(paths) = &mut self.old_paths
        {
            let path = Some(file_path(path));
            paths.retain(|value| value != &path);
        }
    }

    /// PackageSetting.setPageSizeAppCompatFlags at android-16.0.0_r1 (#810).
    pub fn set_page_size_compat(&mut self, mode: i32) -> Result<(), String> {
        if !(0..128).contains(&mode) {
            return Err("Invalid page size compat mode specified".into());
        }
        self.page_size_compat |= mode;
        if mode == 8 {
            self.page_size_compat &= !16;
        } else if mode == 16 {
            self.page_size_compat &= !8;
        }
        Ok(())
    }
}

// java.io.File's Unix normalization retains dot segments and relative paths.
fn file_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c != '/' || !out.ends_with('/') {
            out.push(c);
        }
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// `InstallSource`.
#[derive(Clone, Debug, PartialEq)]
pub struct InstallSource {
    pub installer: Option<String>,
    pub installer_uid: i32,
    pub update_owner: Option<String>,
    pub installer_attribution_tag: Option<String>,
    /// `PackageInstaller.PACKAGE_SOURCE_*`.
    pub package_source: i32,
    pub is_orphaned: bool,
    pub initiating_package: Option<String>,
    pub initiating_package_uninstalled: bool,
    pub initiating_package_signatures: Option<Signatures>,
    pub originating_package: Option<String>,
}

impl InstallSource {
    /// Pinned InstallSource.createInternal's empty-owner normalization.
    pub fn normalized(self) -> Result<Self, String> {
        if self.initiating_package.is_none() && self.initiating_package_signatures.is_some() {
            return Err("install signing owner has no initiating package".into());
        }
        if self.initiating_package.is_none()
            && self.originating_package.is_none()
            && self.installer.is_none()
            && self.update_owner.is_none()
            && self.initiating_package_signatures.is_none()
            && !self.initiating_package_uninstalled
            && self.package_source == 0
        {
            return Ok(Self {
                is_orphaned: self.is_orphaned,
                ..Self::default()
            });
        }
        Ok(self)
    }
}

impl Default for InstallSource {
    fn default() -> Self {
        InstallSource {
            installer: None,
            installer_uid: INVALID_UID,
            update_owner: None,
            installer_attribution_tag: None,
            package_source: 0,
            is_orphaned: false,
            initiating_package: None,
            initiating_package_uninstalled: false,
            initiating_package_signatures: None,
            originating_package: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsesSdkLibrary {
    pub name: String,
    pub version_major: i64,
    pub optional: bool,
}

/// `PackageSignatures` (`SigningDetails`): the signing certificates, DER
/// encoded, and the signing lineage's past certificates with their
/// capability flags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signatures {
    pub scheme_version: i32,
    pub signatures: Vec<Vec<u8>>,
    /// Current Signature capability flags; an empty vector means all zero.
    pub current_flags: Vec<i32>,
    /// Java-serialized public keys from the feed or native verified SPKI.
    pub public_keys: Option<Vec<Option<super::pkg::Serialized>>>,
    pub past_signatures: Option<Vec<(Vec<u8>, i32)>>,
}

pub(in crate::package) fn current_flags(flags: Vec<i32>) -> Vec<i32> {
    if flags.iter().any(|flags| *flags != 0) {
        flags
    } else {
        Vec::new()
    }
}

/// `PackageKeySetData`.
#[derive(Clone, Debug, PartialEq)]
pub struct KeySetData {
    pub proper_signing_key_set: i64,
    pub upgrade_key_sets: Vec<i64>,
    /// Key sets the package defines: alias, id.
    pub defined_key_sets: Vec<(Option<String>, i64)>,
}

impl KeySetData {
    pub fn add_upgrade_key_set(&mut self, id: i64) {
        if !self.upgrade_key_sets.contains(&id) {
            self.upgrade_key_sets.push(id);
        }
    }
    pub fn add_defined_key_set(&mut self, id: i64, alias: Option<String>) {
        match self
            .defined_key_sets
            .iter_mut()
            .find(|(name, _)| name == &alias)
        {
            Some((_, value)) => *value = id,
            None => self.defined_key_sets.push((alias, id)),
        }
        // ArrayMap keeps signed UTF-16 String hashes; null hashes to zero.
        self.defined_key_sets.sort_by_key(|(name, _)| {
            name.as_ref().map_or(0, |name| {
                name.encode_utf16()
                    .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
            })
        });
    }
}

impl Default for KeySetData {
    fn default() -> Self {
        Self {
            proper_signing_key_set: -1,
            upgrade_key_sets: Vec::new(),
            defined_key_sets: Vec::new(),
        }
    }
}

/// `SharedUserSetting`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedUser {
    pub name: String,
    pub app_id: i32,
    pub flags: i32,
    pub signatures: Option<Signatures>,
}

/// `KeySetManagerService`'s state: public keys (X.509 encoded) and key
/// sets by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeySets {
    /// The original only tests whether the version attribute is present.
    pub versioned: bool,
    pub public_keys: Vec<(i64, Vec<u8>)>,
    pub key_sets: Vec<(i64, Vec<i64>)>,
    pub last_issued_key_id: i64,
    pub last_issued_key_set_id: i64,
    /// Runtime references; saved XML counts each role before alias replacement.
    pub reference_counts: Option<std::collections::BTreeMap<i64, i32>>,
}

/// `PermissionInfo.fixProtectionLevel`.
pub fn fix_protection_level(mut level: i32) -> i32 {
    const SIGNATURE_OR_SYSTEM: i32 = 3;
    const SIGNATURE: i32 = 2;
    const FLAG_PRIVILEGED: i32 = 0x10;
    const FLAG_VENDOR_PRIVILEGED: i32 = 0x8000;
    if level == SIGNATURE_OR_SYSTEM {
        level = SIGNATURE | FLAG_PRIVILEGED;
    }
    if level & FLAG_VENDOR_PRIVILEGED != 0 && level & FLAG_PRIVILEGED == 0 {
        level &= !FLAG_VENDOR_PRIVILEGED;
    }
    level
}

/// The signatures written so far, by index: a certificate's first
/// `<cert>` in the document carries its key, later ones only its index.
type Certificates = Vec<Option<(Vec<u8>, i32)>>;

/// Per-attempt transient state. Retry clears pending packages, certificate and
/// keyset tables and first-install timestamps while retaining registered owners.
#[derive(Default)]
pub struct PackageReadAttempt {
    pub signatures: SignatureReader,
    pub key_set_refs: std::collections::BTreeMap<i64, i32>,
    pub pending: Vec<Package>,
    pub first_install_times: std::collections::BTreeMap<String, i64>,
}

#[derive(Debug, PartialEq)]
pub enum PackageReadOutcome {
    Active(usize),
    Pending(usize),
    InvalidHeader,
    DuplicateId,
    RejectedId(super::owner::app_ids::Error),
}

#[derive(Debug, PartialEq)]
pub enum SharedReadOutcome {
    Active(usize),
    InvalidHeader,
    DuplicateId,
    RejectedId(super::owner::app_ids::Error),
}

#[derive(Debug, PartialEq)]
pub enum PendingOutcome {
    Attached(String),
    MissingOwner(String),
    WrongOwner(String),
}

impl PackageReadAttempt {
    /// Settings.readLPw resolves the last read attempt's pending records in order.
    /// The binding owner retains displaced objects and applies group/user effects
    /// before publication; missing or non-group UID owners reject that record.
    pub fn resolve_pending(
        &mut self,
        settings: &mut Settings,
        ids: &mut super::owner::app_ids::AppIds,
        mut attach: impl FnMut(
            &mut Package,
            &SharedUser,
            Option<&Package>,
            &mut super::owner::app_ids::AppIds,
        ) -> Result<(), String>,
    ) -> Result<Vec<PendingOutcome>, String> {
        use super::owner::app_ids::Owner;
        let mut outcomes = Vec::new();
        while let Some(package) = self.pending.first_mut() {
            let outcome = match ids.get(package.uid_owner_id()).cloned() {
                None => PendingOutcome::MissingOwner(package.name.clone()),
                Some(Owner::SharedUser(name)) => {
                    let group = settings
                        .shared_users
                        .iter()
                        .find(|g| g.name == name && g.app_id == package.uid_owner_id())
                        .ok_or("shared UID slot has no matching settings owner")?;
                    let previous = settings
                        .packages
                        .iter()
                        .position(|p| p.name == package.name);
                    package.app_id = group.app_id;
                    package.shared_user_app_id = Some(group.app_id);
                    attach(
                        package,
                        group,
                        previous.map(|index| &settings.packages[index]),
                        ids,
                    )?;
                    let name = package.name.clone();
                    if let Some(index) = previous {
                        settings.packages[index] = package.clone();
                    } else {
                        settings.packages.push(package.clone());
                    }
                    PendingOutcome::Attached(name)
                }
                Some(_) => PendingOutcome::WrongOwner(package.name.clone()),
            };
            self.pending.remove(0);
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}

impl Settings {
    /// Preserve the complete consumed persistence document while applying its
    /// incremental owners. Owner/input failures return before any document export.
    pub fn read_document(
        &mut self,
        bytes: &[u8],
        read_record: impl FnMut(
            &mut Self,
            &mut aim_android_xml::pull::Reader<'_>,
            &Element,
        ) -> Result<bool, ReadError>,
    ) -> Result<Option<Element>, ReadError> {
        let mut reader = aim_android_xml::pull::Reader::with_document(bytes)?;
        self.read_events(&mut reader, read_record)?;
        Ok(reader.take_document()?)
    }

    /// Dispatch one original readSettingsLPw event stream. The callback supplies
    /// package/shared/global owners; recognized owners cannot silently be skipped.
    /// Returns false only when there is no start tag. Boot/retry tables belong to
    /// the caller and are reset before each attempt (#914).
    pub fn read_events(
        &mut self,
        reader: &mut aim_android_xml::pull::Reader<'_>,
        mut read_record: impl FnMut(
            &mut Self,
            &mut aim_android_xml::pull::Reader<'_>,
            &Element,
        ) -> Result<bool, ReadError>,
    ) -> Result<bool, ReadError> {
        use aim_android_xml::pull::Event;
        loop {
            match reader.next()? {
                Event::Start(_) => break,
                Event::EndDocument => return Ok(false),
                _ => {}
            }
        }
        let outer = reader.depth();
        loop {
            match reader.next()? {
                Event::Start(start) => match start.name.as_str() {
                    "version" | "last-platform-version" | "database-version" => {
                        self.read_version(&start)?
                    }
                    "permissions" | "permission-trees" => {
                        self.read_permissions(reader, start.name == "permission-trees")?
                    }
                    "renamed-package" => {
                        if let (Some(new), Some(old)) =
                            (string(&start, "new"), string(&start, "old"))
                        {
                            match self
                                .renamed_packages
                                .iter_mut()
                                .find(|(name, _)| name == &new)
                            {
                                Some((_, value)) => *value = old,
                                None => self.renamed_packages.push((new, old)),
                            }
                        }
                    }
                    "verifier" => self.read_verifier(&start)?,
                    "preferred-packages" | "read-external-storage" => {}
                    _ => {
                        if !read_record(self, reader, &start)? {
                            if matches!(
                                start.name.as_str(),
                                "package"
                                    | "shared-user"
                                    | "updated-package"
                                    | "keyset-settings"
                                    | "domain-verifications"
                                    | "domain-verifications-legacy"
                                    | "preferred-activities"
                                    | "persistent-preferred-activities"
                                    | "crossProfile-intent-filters"
                                    | "default-browser"
                            ) {
                                return Err(ReadError::Owner(format!(
                                    "settings owner unavailable: {}",
                                    start.name
                                )));
                            }
                            signatures::skip(reader)?;
                        }
                    }
                },
                Event::End(_) if reader.depth() <= outer => return Ok(true),
                Event::EndDocument => return Ok(true),
                _ => {}
            }
        }
    }

    /// Read one package from its completed start. The caller resolves domain IDs
    /// and owns legacy permission/domain side effects before using this entry.
    /// Registered settings remain visible if a later body event fails (#914).
    pub fn read_package(
        &mut self,
        reader: &mut aim_android_xml::pull::Reader<'_>,
        start: &Element,
        ids: &mut super::owner::app_ids::AppIds,
        attempt: &mut PackageReadAttempt,
        read_owner: impl FnMut(
            &mut Package,
            &mut aim_android_xml::pull::Reader<'_>,
            &Element,
        ) -> Result<bool, ReadError>,
    ) -> Result<PackageReadOutcome, ReadError> {
        let Some(mut incoming) = package_header(start)? else {
            signatures::skip(reader)?;
            return Ok(PackageReadOutcome::InvalidHeader);
        };
        let first_install_time = incoming.legacy_first_install_time;
        incoming.legacy_first_install_time = 0;
        let (target, outcome) = if incoming.shared_user {
            incoming.shared_user_app_id = Some(incoming.app_id);
            incoming.app_id = 0;
            attempt.pending.push(incoming);
            let index = attempt.pending.len() - 1;
            (
                &mut attempt.pending[index],
                PackageReadOutcome::Pending(index),
            )
        } else if let Some(index) = self.packages.iter().position(|p| p.name == incoming.name) {
            let existing = &mut self.packages[index];
            if existing.app_id != incoming.app_id {
                signatures::skip(reader)?;
                return Ok(PackageReadOutcome::DuplicateId);
            }
            // addPackageLPw returns the existing object. Its construction fields
            // and accumulated child state survive; the reader updates metadata.
            incoming.real_name = existing.real_name.clone();
            incoming.code_path = existing.code_path.clone();
            incoming.flags = existing.flags;
            incoming.private_flags = existing.private_flags;
            incoming.domain_set_id = existing.domain_set_id.clone();
            incoming.signatures = existing.signatures.clone();
            incoming.key_set_data = existing.key_set_data.clone();
            incoming.mime_groups = existing.mime_groups.clone();
            incoming.split_versions = existing.split_versions.clone();
            incoming.uses_static_libraries = existing.uses_static_libraries.clone();
            incoming.uses_sdk_libraries = existing.uses_sdk_libraries.clone();
            incoming.is_sdk_library = existing.is_sdk_library;
            incoming.old_paths = existing.old_paths.clone();
            incoming.leaving_shared_user = existing.leaving_shared_user;
            incoming.shared_user_app_id = existing.shared_user_app_id;
            incoming.transient = existing.transient.clone();
            let progress = incoming.loading_progress;
            incoming.loading_progress = existing.loading_progress;
            incoming.set_loading_progress(progress);
            incoming.page_size_compat = existing.page_size_compat;
            *existing = incoming;
            (existing, PackageReadOutcome::Active(index))
        } else {
            if let Err(error) = ids.register_existing(
                incoming.app_id,
                super::owner::app_ids::Owner::Package(incoming.name.clone()),
            ) {
                signatures::skip(reader)?;
                return Ok(PackageReadOutcome::RejectedId(error));
            }
            self.packages.push(incoming);
            let index = self.packages.len() - 1;
            (&mut self.packages[index], PackageReadOutcome::Active(index))
        };
        target.set_page_size_compat(defaulted(start.int("pageSizeCompat"), 0))?;
        target.read_children(
            reader,
            &mut attempt.signatures,
            &mut attempt.key_set_refs,
            read_owner,
        )?;
        if first_install_time != 0 {
            attempt
                .first_install_times
                .insert(target.name.clone(), first_install_time);
        }
        Ok(outcome)
    }

    /// Read a shared UID from its start event. Registration precedes signatures
    /// and legacy permission mutations; prior registered groups survive retry.
    pub fn read_shared_user(
        &mut self,
        reader: &mut aim_android_xml::pull::Reader<'_>,
        start: &Element,
        ids: &mut super::owner::app_ids::AppIds,
        attempt: &mut PackageReadAttempt,
        mut read_owner: impl FnMut(
            &mut SharedUser,
            &mut aim_android_xml::pull::Reader<'_>,
            &Element,
        ) -> Result<bool, ReadError>,
    ) -> Result<SharedReadOutcome, ReadError> {
        use aim_android_xml::pull::Event;
        let (Some(name), app_id) = (string(start, "name"), defaulted(start.int("userId"), 0))
        else {
            signatures::skip(reader)?;
            return Ok(SharedReadOutcome::InvalidHeader);
        };
        if app_id == 0 {
            signatures::skip(reader)?;
            return Ok(SharedReadOutcome::InvalidHeader);
        }
        let index = if let Some(index) = self.shared_users.iter().position(|g| g.name == name) {
            if self.shared_users[index].app_id != app_id {
                signatures::skip(reader)?;
                return Ok(SharedReadOutcome::DuplicateId);
            }
            index
        } else {
            if let Err(error) = ids.register_existing(
                app_id,
                super::owner::app_ids::Owner::SharedUser(name.clone()),
            ) {
                signatures::skip(reader)?;
                return Ok(SharedReadOutcome::RejectedId(error));
            }
            self.shared_users.push(SharedUser {
                name,
                app_id,
                flags: if defaulted(start.bool("system"), false) {
                    FLAG_SYSTEM
                } else {
                    0
                },
                signatures: None,
            });
            self.shared_users.len() - 1
        };
        let group = &mut self.shared_users[index];
        let outer = reader.depth();
        loop {
            match reader.next()? {
                Event::Start(child) if child.name == "sigs" => {
                    attempt
                        .signatures
                        .read(reader, &child, &mut group.signatures)?;
                }
                Event::Start(child) => {
                    if !read_owner(group, reader, &child)? {
                        signatures::skip(reader)?;
                    }
                }
                Event::End(_) if reader.depth() <= outer => {
                    return Ok(SharedReadOutcome::Active(index));
                }
                Event::EndDocument => return Ok(SharedReadOutcome::Active(index)),
                _ => {}
            }
        }
    }

    /// The settings in the document whose root is `root`.
    pub fn parse(root: &Element) -> Result<Settings, String> {
        Self::parse_with_config(root, &Default::default())
    }

    /// Settings.findOrCreateVersion retains one owner per volume.
    pub fn find_or_create_version(&mut self, uuid: Option<String>) -> &mut Version {
        let index = self
            .versions
            .iter()
            .position(|v| v.volume_uuid == uuid)
            .unwrap_or_else(|| {
                self.versions.push(Version {
                    volume_uuid: uuid,
                    ..Default::default()
                });
                self.versions.len() - 1
            });
        &mut self.versions[index]
    }

    /// Apply the version event before consuming its subtree. The original
    /// creates its owner and assigns each field before reading the next one;
    /// later attribute failures retain those earlier assignments.
    pub fn read_version(&mut self, element: &Element) -> Result<(), String> {
        match element.name.as_str() {
            "version" => {
                let version = self.find_or_create_version(string(element, "volumeUuid"));
                version.sdk_version = required(element, "sdkVersion", element.int("sdkVersion")?)?;
                version.database_version =
                    required(element, "databaseVersion", element.int("databaseVersion")?)?;
                version.build_fingerprint = string(element, "buildFingerprint");
                version.fingerprint = string(element, "fingerprint");
            }
            "last-platform-version" | "database-version" => {
                // UUID_PRIVATE_INTERNAL is null; UUID_PRIMARY_PHYSICAL is this string.
                self.find_or_create_version(None);
                self.find_or_create_version(Some("primary_physical".into()));
                // The default-value getters return the default for malformed
                // values too, unlike the required getters on <version>.
                if element.name == "last-platform-version" {
                    self.find_or_create_version(None).sdk_version =
                        element.int("internal").ok().flatten().unwrap_or(0);
                    self.find_or_create_version(Some("primary_physical".into()))
                        .sdk_version = element.int("external").ok().flatten().unwrap_or(0);
                    let build = string(element, "buildFingerprint");
                    let fingerprint = string(element, "fingerprint");
                    for uuid in [None, Some("primary_physical".into())] {
                        let version = self.find_or_create_version(uuid);
                        version.build_fingerprint = build.clone();
                        version.fingerprint = fingerprint.clone();
                    }
                } else {
                    self.find_or_create_version(None).database_version =
                        element.int("internal").ok().flatten().unwrap_or(0);
                    self.find_or_create_version(Some("primary_physical".into()))
                        .database_version = element.int("external").ok().flatten().unwrap_or(0);
                }
            }
            _ => return Err(format!("not a settings version event: {}", element.name)),
        }
        Ok(())
    }

    /// Consume a permission-definition container from its completed start event.
    /// Each item changes its owner before skipping its subtree, so a later XML
    /// failure leaves earlier definitions available to resilient-file retry.
    pub fn read_permissions(
        &mut self,
        reader: &mut aim_android_xml::pull::Reader<'_>,
        trees: bool,
    ) -> Result<(), String> {
        use aim_android_xml::pull::Event;
        let out = if trees {
            &mut self.permission_trees
        } else {
            &mut self.permissions
        };
        let outer = reader.depth();
        loop {
            match reader.next()? {
                Event::Start(item) => {
                    read_permission(out, &item);
                    let depth = reader.depth();
                    loop {
                        match reader.next()? {
                            Event::End(_) if reader.depth() <= depth => break,
                            Event::EndDocument => return Ok(()),
                            _ => {}
                        }
                    }
                }
                Event::End(_) if reader.depth() <= outer => return Ok(()),
                Event::EndDocument => return Ok(()),
                _ => {}
            }
        }
    }

    /// Settings already has platform/OEM shared UID owners before reading XML.
    pub fn parse_with_config(
        root: &Element,
        config: &super::system_config::SystemConfig,
    ) -> Result<Settings, String> {
        let mut s = Settings::default();
        let mut certificates = Certificates::new();
        let mut key_set_refs = std::collections::BTreeMap::<i64, i32>::new();
        for e in records(root) {
            match e.name.as_str() {
                "package" => {
                    if let Some(p) = package(e, &mut certificates)? {
                        for child in key_set_entries(e).into_iter().filter(|child| {
                            matches!(
                                child.name.as_str(),
                                "proper-signing-keyset" | "defined-keyset"
                            )
                        }) {
                            let count = key_set_refs.entry(identifier(child)?).or_default();
                            *count = count.wrapping_add(1);
                        }
                        for child in children(e, "domain-verification") {
                            s.legacy_domain_info
                                .insert(p.name.clone(), defaulted(child.int("status"), -1));
                        }
                        s.packages.push(p);
                    }
                }
                "updated-package" => s.disabled_system_packages.push(updated_package(e)?),
                "shared-user" => {
                    if let Some(u) = shared_user(e, &mut certificates)? {
                        s.shared_users.push(u);
                    }
                }
                "permissions" => {
                    for item in e.children() {
                        read_permission(&mut s.permissions, item);
                    }
                }
                "permission-trees" => {
                    for item in e.children() {
                        read_permission(&mut s.permission_trees, item);
                    }
                }
                "renamed-package" => {
                    if let (Some(new), Some(old)) = (string(e, "new"), string(e, "old")) {
                        s.renamed_packages.push((new, old));
                    }
                }
                "verifier" => s.read_verifier(e).map_err(|error| error.to_string())?,
                "keyset-settings" => {
                    s.key_sets = key_sets(e)?;
                    if s.key_sets.versioned {
                        s.key_sets.reference_counts = Some(
                            s.key_sets
                                .key_sets
                                .iter()
                                .map(|(id, _)| (*id, key_set_refs.get(id).copied().unwrap_or(0)))
                                .collect(),
                        );
                    }
                    if !s.key_sets.versioned {
                        for package in &mut s.packages {
                            package.key_set_data = KeySetData::default();
                        }
                    }
                }
                "domain-verifications" => s.domain_verification.read(e)?,
                "domain-verifications-legacy" => s.domain_verification.read_legacy(e)?,
                "version" | "last-platform-version" | "database-version" => s.read_version(e)?,
                _ => {}
            }
        }
        // `readLPw`: a package of an unknown shared user is dropped, an
        // updated system package of a shared user's app id belongs to it.
        let seeded = super::owner::shared_users::Bootstrap::new(config);
        let shared: Vec<i32> = s
            .shared_users
            .iter()
            .map(|u| u.app_id)
            .chain(seeded.shared_users.values().map(|group| group.app_id))
            .collect();
        s.packages
            .retain(|p| !p.shared_user || shared.contains(&p.app_id));
        for p in &mut s.disabled_system_packages {
            p.shared_user |= shared.contains(&p.app_id);
        }
        Ok(s)
    }
}

/// The original event loop leaves attribute-only/deprecated tags unconsumed.
/// Helper-owned and unknown subtrees are skipped as a whole.
pub(in crate::package) fn records(root: &Element) -> Vec<&Element> {
    let mut out = Vec::new();
    for entry in root.children() {
        out.push(entry);
        if matches!(
            entry.name.as_str(),
            "version"
                | "renamed-package"
                | "verifier"
                | "last-platform-version"
                | "database-version"
                | "preferred-packages"
                | "read-external-storage"
        ) {
            out.extend(records(entry));
        }
    }
    out
}

/// `getAttributeInt(null, name)` without a default: the attribute must be
/// there.
fn required<T>(e: &Element, name: &str, v: Option<T>) -> Result<T, String> {
    v.ok_or_else(|| format!("<{}> without {name}", e.name))
}

fn read_permission(out: &mut Vec<Permission>, item: &Element) {
    if item.name != "item" {
        return;
    }
    let (Some(name), Some(package)) = (string(item, "name"), string(item, "package")) else {
        return;
    };
    let protection_level = fix_protection_level(defaulted(item.int("protection"), 0));
    let dynamic = (string(item, "type").as_deref() == Some("dynamic"))
        .then(|| (defaulted(item.int("icon"), 0), string(item, "label")));
    if let Some(old) = out.iter_mut().find(|p| p.name == name)
        && matches!(old.owner, PermissionOwner::Config { .. })
    {
        old.protection_level = protection_level;
        if dynamic.is_some() {
            old.dynamic = dynamic;
        }
        return;
    }
    let permission = Permission {
        protection_level,
        owner: if dynamic.is_some() {
            PermissionOwner::Dynamic
        } else {
            PermissionOwner::Manifest
        },
        dynamic,
        name,
        package,
    };
    match out.iter_mut().find(|p| p.name == permission.name) {
        Some(old) => *old = permission,
        None => out.push(permission),
    }
}

// TypedXmlPullParser overloads with a default catch conversion failures too.
// Only call this for attributes whose pinned owner supplies that default.
fn defaulted<T>(value: Result<Option<T>, String>, default: T) -> T {
    value.ok().flatten().unwrap_or(default)
}

/// The attributes `<package>` and `<updated-package>` share; `None`
/// without a name or code path.
fn package_attributes(e: &Element) -> Result<Option<Package>, String> {
    let (Some(name), Some(code_path)) = (string(e, "name"), string(e, "codePath")) else {
        return Ok(None);
    };
    let mut p = Package {
        name,
        real_name: string(e, "realName"),
        code_path,
        legacy_native_library_path: string(e, "nativeLibraryPath"),
        primary_cpu_abi: string(e, "primaryCpuAbi").or_else(|| string(e, "requiredCpuAbi")),
        secondary_cpu_abi: string(e, "secondaryCpuAbi"),
        cpu_abi_override: string(e, "cpuAbiOverride"),
        version_code: defaulted(e.long("version"), 0),
        target_sdk_version: defaulted(e.int("targetSdkVersion"), 0),
        restrict_update_hash: e.bytes_base64("restrictUpdateHash").ok().flatten(),
        scanned_as_stopped_system_app: defaulted(e.bool("scannedAsStoppedSystemApp"), false),
        last_modified_time: match defaulted(e.long_hex("ft"), 0) {
            0 => defaulted(e.long("ts"), 0),
            ft => ft,
        },
        last_update_time: defaulted(e.long_hex("ut"), 0),
        app_metadata_file_path: string(e, "appMetadataFilePath"),
        app_metadata_source: defaulted(e.int("appMetadataSource"), 0),
        category_hint: CATEGORY_UNDEFINED,
        // readPackageLPw/readDisabledSysPackageLPw construct a fresh setting.
        leaving_shared_user: Some(false),
        ..Package::default()
    };
    // `userId` and `sharedUserId` are the app id's historical names.
    p.app_id = defaulted(e.int("userId"), 0);
    if p.app_id <= 0 {
        p.app_id = defaulted(e.int("sharedUserId"), 0);
        p.shared_user = p.app_id > 0;
    }
    Ok(Some(p))
}

/// The elements both kinds of package hold.
fn libraries(p: &mut Package, child: &Element) -> Result<bool, String> {
    match child.name.as_str() {
        "uses-static-lib" => {
            let version = defaulted(child.long("version"), -1);
            if let Some(name) = string(child, "name").filter(|_| version >= 0) {
                match p.uses_static_libraries.iter_mut().find(|(n, _)| *n == name) {
                    Some(lib) => lib.1 = version,
                    None => p.uses_static_libraries.push((name, version)),
                }
            }
        }
        "uses-sdk-lib" => {
            let version_major = defaulted(child.long("version"), -1);
            let optional = defaulted(child.bool("optional"), true);
            if let Some(name) = string(child, "name").filter(|_| version_major >= 0) {
                let lib = UsesSdkLibrary {
                    name,
                    version_major,
                    optional,
                };
                match p.uses_sdk_libraries.iter_mut().find(|l| l.name == lib.name) {
                    Some(old) => *old = lib,
                    None => p.uses_sdk_libraries.push(lib),
                }
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `readDisabledSysPackageLPw`.
fn updated_package(e: &Element) -> Result<Package, String> {
    let mut p = package_attributes(e)?.ok_or("an updated package without a name or code path")?;
    p.flags = FLAG_SYSTEM;
    if p.code_path.contains("/priv-app/") {
        p.private_flags = PRIVATE_FLAG_PRIVILEGED;
    }
    for child in e.children() {
        libraries(&mut p, child)?;
    }
    Ok(p)
}

/// `readPackageLPw`: `None` for an entry the original drops (no name, code
/// path or app id).
fn package_header(e: &Element) -> Result<Option<Package>, String> {
    // The pinned Settings DEX compiles out disallowSdkLibsToBeApps:
    // SDK libraries still need a positive app/shared-user ID (#802).
    let Some(mut p) = package_attributes(e)?.filter(|p| p.app_id > 0) else {
        return Ok(None);
    };
    p.set_loading_progress(e.float("loadingProgress").ok().flatten().unwrap_or(0.0));
    p.loading_completed_time = e
        .long_hex("loadingCompletedTime")
        .ok()
        .flatten()
        .unwrap_or(0);
    p.is_sdk_library = defaulted(e.bool("isSdkLibrary"), false);
    if let Some(public) = string(e, "publicFlags") {
        // Settings parses these string attributes with Integer.parseInt,
        // including ABX getAttributeValue formatting, and catches bad numbers.
        p.flags = public.parse().unwrap_or(0);
        p.private_flags = string(e, "privateFlags")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
    } else if let Some(flags) = string(e, "flags") {
        // Pre-M stored these private bits in the public flags field.
        p.flags = flags.parse().unwrap_or(0);
        for (old, private) in [(1 << 27, 1 << 0), (1 << 28, 1 << 1), (1 << 30, 1 << 3)] {
            if p.flags & old != 0 {
                p.private_flags |= private;
            }
            p.flags &= !old;
        }
    } else {
        p.flags = match string(e, "system") {
            Some(system) if !system.eq_ignore_ascii_case("true") => 0,
            _ => FLAG_SYSTEM,
        };
    }
    p.legacy_first_install_time = defaulted(e.long_hex("it"), 0);
    p.install_source = InstallSource {
        installer: string(e, "installer"),
        installer_uid: defaulted(e.int("installerUid"), INVALID_UID),
        update_owner: string(e, "updateOwner"),
        installer_attribution_tag: string(e, "installerAttributionTag"),
        package_source: defaulted(e.int("packageSource"), 0),
        is_orphaned: defaulted(e.bool("isOrphaned"), false),
        initiating_package: string(e, "installInitiator"),
        initiating_package_uninstalled: defaulted(e.bool("installInitiatorUninstalled"), false),
        initiating_package_signatures: None,
        originating_package: string(e, "installOriginator"),
    };
    p.install_source = p.install_source.normalized()?;
    p.volume_uuid = string(e, "volumeUuid");
    p.category_hint = defaulted(e.int("categoryHint"), CATEGORY_UNDEFINED);
    p.update_available = defaulted(e.bool("updateAvailable"), false);
    p.force_queryable = defaulted(e.bool("forceQueryable"), false);
    p.pending_restore = defaulted(e.bool("pendingRestore"), false);
    p.debuggable = defaulted(e.bool("debuggable"), false);

    p.base_revision_code = defaulted(e.int("baseRevisionCode"), 0);
    p.domain_set_id = string(e, "domainSetId").filter(|id| !id.is_empty());
    Ok(Some(p))
}

fn package(e: &Element, certificates: &mut Certificates) -> Result<Option<Package>, String> {
    let Some(mut p) = package_header(e)? else {
        return Ok(None);
    };
    p.set_page_size_compat(defaulted(e.int("pageSizeCompat"), 0))?;
    for child in e.children() {
        if libraries(&mut p, child)? {
            continue;
        }
        match child.name.as_str() {
            "sigs" => {
                signatures(child, certificates, &mut p.signatures)?;
            }
            "install-initiator-sigs" => {
                let mut target = None;
                signatures(child, certificates, &mut target)?;
                p.install_source.initiating_package_signatures = target;
            }
            "mime-group" => {
                if let Some(group) = string(child, "name") {
                    let mut types = Vec::new();
                    read_mime_types(child, &mut types);
                    p.add_mime_types(group, types);
                }
            }
            "split-version" => p.read_split_version(child),
            _ => {}
        }
    }
    for child in key_set_entries(e) {
        match child.name.as_str() {
            "proper-signing-keyset" => p.key_set_data.proper_signing_key_set = identifier(child)?,
            "upgrade-keyset" => p.key_set_data.add_upgrade_key_set(identifier(child)?),
            "defined-keyset" => p
                .key_set_data
                .add_defined_key_set(identifier(child)?, string(child, "alias")),
            _ => {}
        }
    }
    p.install_source = p.install_source.normalized()?;
    Ok(Some(p))
}

// These Settings branches leave the parser inside the tag, so nested keyset
// entries are visited too. Unknown package children consume their subtree.
fn key_set_entries(e: &Element) -> Vec<&Element> {
    let mut entries = Vec::new();
    for child in e.children() {
        if matches!(
            child.name.as_str(),
            "proper-signing-keyset" | "defined-keyset" | "upgrade-keyset" | "signing-keyset"
        ) {
            entries.push(child);
            entries.extend(key_set_entries(child));
        }
    }
    entries
}

fn string_hash(value: &str) -> i32 {
    value.encode_utf16().fold(0i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

// The group publishes after the container completes; mime-type starts do not
// skip their own subtree, so nested type events remain visible.
fn read_mime_group(reader: &mut aim_android_xml::pull::Reader<'_>) -> Result<Vec<String>, String> {
    use aim_android_xml::pull::Event;
    let outer = reader.depth();
    let mut types = Vec::new();
    loop {
        match reader.next()? {
            Event::Start(child) if child.name == "mime-type" => {
                if let Some(value) = string(&child, "value") {
                    types.push(value);
                }
            }
            Event::Start(_) => signatures::skip(reader)?,
            Event::End(_) if reader.depth() <= outer => return Ok(types),
            Event::EndDocument => return Ok(types),
            _ => {}
        }
    }
}

fn read_mime_types(e: &Element, types: &mut Vec<String>) {
    for child in e.children().filter(|child| child.name == "mime-type") {
        if let Some(value) = string(child, "value") {
            types.push(value);
        }
        read_mime_types(child, types);
    }
}

fn identifier(e: &Element) -> Result<i64, String> {
    required(e, "identifier", e.long("identifier")?)
}

/// `readSharedUserLPw`: `None` for an entry the original drops.
fn shared_user(e: &Element, certificates: &mut Certificates) -> Result<Option<SharedUser>, String> {
    let (Some(name), Some(app_id)) = (
        string(e, "name"),
        Some(defaulted(e.int("userId"), 0)).filter(|id| *id != 0),
    ) else {
        return Ok(None);
    };
    let mut u = SharedUser {
        name,
        app_id,
        flags: if defaulted(e.bool("system"), false) {
            FLAG_SYSTEM
        } else {
            0
        },
        signatures: None,
    };
    for sigs in children(e, "sigs") {
        signatures(sigs, certificates, &mut u.signatures)?;
    }
    Ok(Some(u))
}

/// `PackageSignatures.readXml`: a missing count preserves the target;
/// invalid completed signing data clears it after certificate-table effects.
fn signatures(
    e: &Element,
    certificates: &mut Certificates,
    target: &mut Option<Signatures>,
) -> Result<(), String> {
    let count = defaulted(e.int("count"), -1);
    if count == -1 {
        return Ok(());
    }
    let mut s = Signatures {
        scheme_version: defaulted(e.int("schemeVersion"), SCHEME_UNKNOWN),
        ..Signatures::default()
    };
    let mut read = 0;
    for child in e.children() {
        match child.name.as_str() {
            "cert" => {
                if read < count
                    && let Some((key, flags, _)) = certificate(child, certificates)?
                {
                    s.signatures.push(key);
                    s.current_flags.push(flags);
                }
                read += 1;
            }
            "pastSigs" => {
                let count = defaulted(child.int("count"), -1);
                if count == -1 {
                    continue;
                }
                let mut past = Vec::new();
                for cert in children(child, "cert").take(count.max(0) as usize) {
                    if let Some((key, inherited, added)) = certificate(cert, certificates)? {
                        let flags = match defaulted(cert.int("flags"), -1) {
                            -1 => inherited,
                            flags => flags,
                        };
                        if let Some(index) = added {
                            certificates[index].as_mut().unwrap().1 = flags;
                        }
                        past.push((key, flags));
                    }
                }
                s.past_signatures = Some(past);
            }
            _ => {}
        }
    }
    *target = signatures::build(s)?;
    Ok(())
}

/// A `<cert>`'s key: its own, which takes its index in the document's
/// table, or the one an earlier `<cert>` gave its index.
fn certificate(
    e: &Element,
    certificates: &mut Certificates,
) -> Result<Option<(Vec<u8>, i32, Option<usize>)>, String> {
    let index = defaulted(e.int("index"), -1);
    if index == -1 {
        return Ok(None);
    }
    Ok(match e.bytes_hex("key").ok().flatten() {
        Some(key) => {
            // A new Signature is appended, even if its XML index was already
            // used. Negative indices other than -1 also reach this branch.
            if index > 0 && certificates.len() < index as usize {
                certificates.resize(index as usize, None);
            }
            let added = certificates.len();
            certificates.push(Some((key.clone(), 0)));
            Some((key, 0, Some(added)))
        }
        None if index >= 0 => certificates
            .get(index as usize)
            .cloned()
            .flatten()
            .map(|(key, flags)| (key, flags, None)),
        None => None,
    })
}

/// `KeySetManagerService.readKeySetsLPw`. A document without a version
/// is from before key sets were versioned; the original then drops every
/// package's key set data.
fn key_sets(e: &Element) -> Result<KeySets, String> {
    let mut k = KeySets {
        versioned: string(e, "version").is_some(),
        ..KeySets::default()
    };
    if !k.versioned {
        return Ok(k);
    }
    for child in e.children() {
        match child.name.as_str() {
            "keys" => {
                for key in children(child, "public-key") {
                    let id = identifier(key)?;
                    if let Some(value) = key.bytes_base64("value").ok().flatten()
                        && let Ok(mut canonical) = super::sign::canonical_public_keys(&[value])
                    {
                        k.public_keys.push((id, canonical.remove(0)));
                    }
                }
            }
            "keysets" => {
                for set in children(child, "keyset") {
                    let keys = children(set, "key-id")
                        .map(identifier)
                        .collect::<Result<_, _>>()?;
                    k.key_sets.push((identifier(set)?, keys));
                }
            }
            "lastIssuedKeyId" => {
                k.last_issued_key_id = required(child, "value", child.long("value")?)?
            }
            "lastIssuedKeySetId" => {
                k.last_issued_key_set_id = required(child, "value", child.long("value")?)?
            }
            _ => {}
        }
    }
    Ok(k)
}

#[cfg(test)]
mod install_source_tests {
    use super::*;
    #[test]
    fn empty_owner_normalizes_attributes_and_signing_requires_initiator() {
        for orphan in [false, true] {
            for input in [
                "installerUid='123' installerAttributionTag='tag' packageSource='0'",
                "",
            ] {
                let xml = format!(
                    "<packages><package name='p' codePath='/data/p' userId='10001' isOrphaned='{orphan}' {input}/></packages>"
                );
                let root = aim_android_xml::read(xml.as_bytes()).unwrap();
                for root in [
                    root.clone(),
                    aim_android_xml::read(&aim_android_xml::abx::write(&root).unwrap()).unwrap(),
                ] {
                    let p = Settings::parse(&root).unwrap().packages.remove(0);
                    assert_eq!(
                        p.install_source,
                        InstallSource {
                            is_orphaned: orphan,
                            ..Default::default()
                        }
                    );
                }
            }
        }
        let invalid = InstallSource {
            initiating_package_signatures: Some(Signatures::default()),
            ..Default::default()
        };
        assert!(invalid.normalized().is_err());
        let retained = InstallSource {
            initiating_package: Some("".into()),
            installer_uid: 123,
            installer_attribution_tag: Some("tag".into()),
            package_source: 3,
            ..Default::default()
        };
        assert_eq!(retained.clone().normalized().unwrap(), retained);
        let unspecified_names = InstallSource {
            installer_uid: 123,
            installer_attribution_tag: Some("tag".into()),
            package_source: 3,
            ..Default::default()
        };
        assert_eq!(
            unspecified_names.clone().normalized().unwrap(),
            unspecified_names
        );
    }
}

#[cfg(test)]
mod version_event_tests {
    use super::*;

    #[test]
    fn assignments_survive_later_attribute_errors_and_repeated_versions_keep_the_owner() {
        let mut settings = Settings::default();
        let read = |settings: &mut Settings, xml: &str| {
            settings.read_version(&aim_android_xml::read(xml.as_bytes()).unwrap())
        };
        read(&mut settings, "<version volumeUuid='v' sdkVersion='35' databaseVersion='7' buildFingerprint='old' fingerprint='old-partitions'/>").unwrap();
        read(
            &mut settings,
            "<version volumeUuid='other' sdkVersion='34' databaseVersion='6'/>",
        )
        .unwrap();
        assert!(read(&mut settings, "<version volumeUuid='v' sdkVersion='36' databaseVersion='broken' buildFingerprint='new'/>").is_err());
        assert_eq!(
            settings.versions[0],
            Version {
                volume_uuid: Some("v".into()),
                sdk_version: 36,
                database_version: 7,
                build_fingerprint: Some("old".into()),
                fingerprint: Some("old-partitions".into())
            }
        );
        assert_eq!(settings.versions[1].volume_uuid.as_deref(), Some("other"));
        assert!(
            read(
                &mut settings,
                "<version volumeUuid='new' databaseVersion='1'/>"
            )
            .is_err()
        );
        assert_eq!(
            settings.versions[2],
            Version {
                volume_uuid: Some("new".into()),
                ..Default::default()
            }
        );
        read(
            &mut settings,
            "<last-platform-version internal='35' external='bad'/>",
        )
        .unwrap();
        assert_eq!(settings.versions[3].sdk_version, 35);
        assert_eq!(settings.versions[4].sdk_version, 0);
        read(
            &mut settings,
            "<database-version internal='6' external='7'/>",
        )
        .unwrap();
        assert_eq!(settings.versions[3].database_version, 6);
        assert_eq!(settings.versions[4].database_version, 7);
    }
}

#[cfg(test)]
mod incremental_package_tests {
    use super::*;
    use aim_android_xml::pull::{Event, Reader};
    use std::collections::BTreeMap;

    fn read(
        bytes: &[u8],
        package: &mut Package,
        refs: &mut BTreeMap<i64, i32>,
    ) -> Result<(), String> {
        let mut reader = Reader::new(bytes)?;
        assert!(matches!(reader.next()?, Event::Start(_)));
        let mut signatures = SignatureReader::default();
        loop {
            match reader.next()? {
                Event::Start(child) => {
                    if !package.read_child(&mut reader, &child, &mut signatures, refs)? {
                        signatures::skip(&mut reader)?;
                    }
                }
                Event::End(_) if reader.depth() == 1 => return Ok(()),
                Event::EndDocument => return Ok(()),
                _ => {}
            }
        }
    }

    fn registered(
        bytes: &[u8],
        settings: &mut Settings,
        ids: &mut super::super::owner::app_ids::AppIds,
        attempt: &mut PackageReadAttempt,
    ) -> Result<PackageReadOutcome, String> {
        let mut reader = Reader::new(bytes)?;
        let Event::Start(start) = reader.next()? else {
            panic!()
        };
        settings
            .read_package(&mut reader, &start, ids, attempt, |_, _, _| Ok(false))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn registration_precedes_body_failure_and_retry_keeps_uid_slots() {
        use super::super::owner::app_ids::{AppIds, Owner};
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        assert!(registered(b"<package name='p' codePath='/p' userId='10001' it='7'><uses-static-lib name='l' version='3'/><", &mut settings, &mut ids, &mut attempt).is_err());
        assert_eq!(ids.get(10001), Some(&Owner::Package("p".into())));
        assert_eq!(
            settings.packages[0].uses_static_libraries,
            [("l".into(), 3)]
        );
        assert!(attempt.first_install_times.is_empty());
        attempt = PackageReadAttempt::default();
        let outcome = registered(
            b"<package name='q' codePath='/q' userId='10001'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        assert!(matches!(outcome, PackageReadOutcome::RejectedId(_)));
        assert_eq!(settings.packages.len(), 1);
        registered(
            b"<package name='p' codePath='/other' userId='10001' it='9' version='6'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        assert_eq!(settings.packages[0].code_path, "/p");
        assert_eq!(settings.packages[0].version_code, 6);
        assert_eq!(
            settings.packages[0].uses_static_libraries,
            [("l".into(), 3)]
        );
        assert_eq!(attempt.first_install_times["p"], 9);
    }

    #[test]
    fn invalid_page_mode_retains_registered_header_effects() {
        use super::super::owner::app_ids::{AppIds, Owner};
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        assert_eq!(registered(b"<package name='p' codePath='/p' userId='10001' pageSizeCompat='128' version='5'/>", &mut settings, &mut ids, &mut attempt).unwrap_err(), "Invalid page size compat mode specified");
        assert_eq!(settings.packages[0].version_code, 5);
        assert_eq!(settings.packages[0].page_size_compat, 0);
        assert_eq!(ids.get(10001), Some(&Owner::Package("p".into())));
    }

    #[test]
    fn shared_package_is_pending_before_failure_and_not_registered() {
        use super::super::owner::app_ids::AppIds;
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        assert!(registered(b"<package name='p' codePath='/p' sharedUserId='10001'><split-version name='x' version='2'/><", &mut settings, &mut ids, &mut attempt).is_err());
        assert!(settings.packages.is_empty());
        assert!(ids.get(10001).is_none());
        assert_eq!(attempt.pending[0].split_versions, [("x".into(), 2)]);
        attempt = PackageReadAttempt::default();
        assert!(attempt.pending.is_empty());
    }

    fn shared(
        bytes: &[u8],
        settings: &mut Settings,
        ids: &mut super::super::owner::app_ids::AppIds,
        attempt: &mut PackageReadAttempt,
    ) -> Result<SharedReadOutcome, String> {
        let mut reader = Reader::new(bytes)?;
        let Event::Start(start) = reader.next()? else {
            panic!()
        };
        settings
            .read_shared_user(&mut reader, &start, ids, attempt, |_, _, _| Ok(false))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn shared_registration_survives_failure_and_pending_resolves_after_group_read() {
        use super::super::owner::app_ids::{AppIds, Owner};
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        registered(
            b"<package name='p' codePath='/p' sharedUserId='10001'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        assert_eq!(attempt.pending[0].app_id, 0);
        assert_eq!(attempt.pending[0].shared_app_id(), Some(10001));
        assert!(
            shared(
                b"<shared-user name='g' userId='10001' system='true'><",
                &mut settings,
                &mut ids,
                &mut attempt
            )
            .is_err()
        );
        assert_eq!(ids.get(10001), Some(&Owner::SharedUser("g".into())));
        // A new read attempt drops pending p but preserves the registered group.
        attempt = PackageReadAttempt::default();
        shared(
            b"<shared-user name='g' userId='10001' system='false'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        assert_eq!(settings.shared_users[0].flags, FLAG_SYSTEM);
        registered(
            b"<package name='p' codePath='/p' sharedUserId='10001'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        let outcome = attempt
            .resolve_pending(&mut settings, &mut ids, |p, g, old, _| {
                assert_eq!(p.app_id, g.app_id);
                assert!(old.is_none());
                Ok(())
            })
            .unwrap();
        assert_eq!(outcome, [PendingOutcome::Attached("p".into())]);
        assert_eq!(settings.packages[0].shared_app_id(), Some(10001));
        assert!(attempt.pending.is_empty());
    }

    #[test]
    fn pending_rejects_missing_or_package_slots_and_binding_error_retains_records() {
        use super::super::owner::app_ids::AppIds;
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        registered(
            b"<package name='active' codePath='/a' userId='10002'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        for bytes in [
            b"<package name='missing' codePath='/p' sharedUserId='10001'/>".as_slice(),
            b"<package name='wrong' codePath='/p' sharedUserId='10002'/>",
        ] {
            registered(bytes, &mut settings, &mut ids, &mut attempt).unwrap();
        }
        assert_eq!(
            attempt
                .resolve_pending(&mut settings, &mut ids, |_, _, _, _| panic!(
                    "invalid group attached"
                ))
                .unwrap(),
            [
                PendingOutcome::MissingOwner("missing".into()),
                PendingOutcome::WrongOwner("wrong".into())
            ]
        );
        shared(
            b"<shared-user name='g' userId='10003'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        registered(
            b"<package name='pending' codePath='/p' sharedUserId='10003'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        assert_eq!(
            attempt
                .resolve_pending(&mut settings, &mut ids, |_, _, _, _| Err(
                    "binding owner unavailable".into()
                ))
                .unwrap_err(),
            "binding owner unavailable"
        );
        assert_eq!(attempt.pending.len(), 1);
        assert_eq!(settings.packages.len(), 1);
    }

    #[test]
    fn binding_owner_can_retain_displaced_uid_object_before_publication() {
        use super::super::owner::app_ids::{AppIds, DetachedSetting, Owner};
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        registered(
            b"<package name='p' codePath='/old' userId='10000'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        shared(
            b"<shared-user name='g' userId='10001'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        registered(
            b"<package name='p' codePath='/new' sharedUserId='10001'/>",
            &mut settings,
            &mut ids,
            &mut attempt,
        )
        .unwrap();
        attempt
            .resolve_pending(&mut settings, &mut ids, |_, _, old, ids| {
                ids.detach(DetachedSetting {
                    package: old.unwrap().clone(),
                    users: Default::default(),
                    user_aliases: Default::default(),
                    legacy: None,
                    install_fixed: None,
                    runtime: None,
                })
            })
            .unwrap();
        assert_eq!(settings.packages[0].app_id, 10001);
        assert_eq!(settings.packages[0].code_path, "/new");
        assert_eq!(ids.get(10000), Some(&Owner::DetachedPackage("p".into())));
        assert_eq!(
            ids.detached_setting(10000).unwrap().package.code_path,
            "/old"
        );
    }

    #[test]
    fn dispatcher_retains_attribute_events_and_skips_unknown_subtrees() {
        let mut settings = Settings::default();
        let mut reader = Reader::new(b"<packages><renamed-package new='n' old='a'/><renamed-package new='n' old='b'/><preferred-packages><version sdkVersion='36' databaseVersion='3'/></preferred-packages><unknown><version volumeUuid='hidden' sdkVersion='1' databaseVersion='1'/></unknown><permissions><item name='perm' package='p' protection='2'/></permissions></packages>broken").unwrap();
        assert!(
            settings
                .read_events(&mut reader, |_, _, _| Ok(false))
                .unwrap()
        );
        assert_eq!(settings.renamed_packages, [("n".into(), "b".into())]);
        assert_eq!(settings.versions.len(), 1);
        assert_eq!(settings.versions[0].sdk_version, 36);
        assert_eq!(settings.permissions[0].protection_level, 2);
    }

    #[test]
    fn dispatcher_missing_owner_keeps_prior_records_and_is_not_a_file_error() {
        let mut settings = Settings::default();
        let mut reader = Reader::new(b"<packages><version sdkVersion='36' databaseVersion='3'/><keyset-settings version='1'/></packages>").unwrap();
        assert_eq!(
            settings
                .read_events(&mut reader, |_, _, _| Ok(false))
                .unwrap_err(),
            ReadError::Owner("settings owner unavailable: keyset-settings".into())
        );
        assert_eq!(settings.versions[0].sdk_version, 36);
        assert!(
            !Settings::default()
                .read_events(&mut Reader::new(b" ").unwrap(), |_, _, _| Ok(false))
                .unwrap()
        );
    }

    #[test]
    fn document_capture_preserves_helper_subtrees_unknown_extensions_and_types() {
        let bytes = b"<packages custom='keep'><!--note--><permissions><item name='p' package='owner' protection='2'/></permissions><extension value='keep'><nested/></extension><version sdkVersion='36' databaseVersion='3'/></packages>";
        let root = aim_android_xml::read_next(bytes).unwrap();
        for bytes in [bytes.to_vec(), aim_android_xml::abx::write(&root).unwrap()] {
            let mut settings = Settings::default();
            assert_eq!(
                settings
                    .read_document(&bytes, |_, _, _| Ok(false))
                    .unwrap()
                    .unwrap(),
                aim_android_xml::read_next(&bytes).unwrap()
            );
            assert_eq!(settings.permissions.len(), 1);
            assert_eq!(settings.versions[0].sdk_version, 36);
        }
        let mut settings = Settings::default();
        assert!(matches!(settings.read_document(b"<packages><version sdkVersion='36' databaseVersion='3'/><keyset-settings/></packages>", |_,_,_| Ok(false)), Err(ReadError::Owner(_))));
        assert_eq!(settings.versions[0].sdk_version, 36);
    }

    #[test]
    fn package_body_retains_earlier_effects_and_propagates_external_owner_failure() {
        let mut reader =
            Reader::new(b"<package><uses-static-lib name='l' version='3'/><perms/></package>")
                .unwrap();
        reader.next().unwrap();
        let mut package = Package::default();
        let mut signatures = SignatureReader::default();
        let mut refs = BTreeMap::new();
        let error = package
            .read_children(&mut reader, &mut signatures, &mut refs, |p, _, child| {
                assert_eq!(child.name, "perms");
                assert_eq!(p.uses_static_libraries, [("l".into(), 3)]);
                Err(ReadError::Owner("legacy permission owner failed".into()))
            })
            .unwrap_err();
        assert_eq!(
            error,
            ReadError::Owner("legacy permission owner failed".into())
        );
        assert_eq!(package.uses_static_libraries, [("l".into(), 3)]);
    }

    #[test]
    fn mime_group_failure_retains_previous_completed_groups() {
        let mut package = Package::default();
        assert!(read(b"<package><mime-group name='g'><mime-type value='a'/></mime-group><mime-group name='g'><mime-type value='b'/><", &mut package, &mut BTreeMap::new()).is_err());
        assert_eq!(
            package.mime_groups,
            [(Some("g".into()), vec![Some("a".into())])]
        );
    }

    #[test]
    fn package_children_preserve_start_effects_when_subtree_read_fails() {
        for (tag, attributes) in [
            ("uses-static-lib", "name='lib' version='7'"),
            ("uses-sdk-lib", "name='lib' version='7' optional='false'"),
            ("split-version", "name='lib' version='7'"),
            ("proper-signing-keyset", "identifier='7'"),
            ("defined-keyset", "identifier='7' alias='lib'"),
            ("upgrade-keyset", "identifier='7'"),
        ] {
            let bytes = format!("<package><{tag} {attributes}><");
            let mut package = Package::default();
            let mut refs = BTreeMap::new();
            assert!(
                read(bytes.as_bytes(), &mut package, &mut refs).is_err(),
                "{tag}"
            );
            match tag {
                "uses-static-lib" => assert_eq!(package.uses_static_libraries, [("lib".into(), 7)]),
                "uses-sdk-lib" => {
                    assert_eq!(package.uses_sdk_libraries[0].version_major, 7);
                    assert!(!package.uses_sdk_libraries[0].optional);
                }
                "split-version" => assert_eq!(package.split_versions, [("lib".into(), 7)]),
                "proper-signing-keyset" => {
                    assert_eq!(package.key_set_data.proper_signing_key_set, 7);
                    assert_eq!(refs[&7], 1);
                }
                "defined-keyset" => assert_eq!(refs[&7], 1),
                "upgrade-keyset" => assert_eq!(package.key_set_data.upgrade_key_sets, [7]),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn package_child_skip_boundaries_and_reference_counts_match_tree_import() {
        let bytes = b"<package name='p' codePath='/p' userId='10001'><uses-static-lib name='a' version='1'><uses-static-lib name='hidden' version='2'/></uses-static-lib><uses-static-lib name='a' version='3'/><uses-sdk-lib name='b' version='4' optional='false'/><split-version name='x' version='1'/><split-version name='x' version='5'/><proper-signing-keyset identifier='8'><defined-keyset identifier='8' alias='alias'/></proper-signing-keyset><upgrade-keyset identifier='9'/></package>";
        for bytes in [
            bytes.to_vec(),
            aim_android_xml::abx::write(&aim_android_xml::read(bytes).unwrap()).unwrap(),
        ] {
            let root = aim_android_xml::read(&bytes).unwrap();
            let expected = package(&root, &mut Vec::new()).unwrap().unwrap();
            let mut actual = package(
                &Element {
                    content: Vec::new(),
                    ..root
                },
                &mut Vec::new(),
            )
            .unwrap()
            .unwrap();
            let mut refs = BTreeMap::from([(8, i32::MAX)]);
            read(&bytes, &mut actual, &mut refs).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(refs[&8], i32::MIN + 1);
        }
        let mut actual = Package::default();
        let mut refs = BTreeMap::new();
        assert!(read(b"<package><proper-signing-keyset identifier='2'/><defined-keyset identifier='bad'/></package>", &mut actual, &mut refs).is_err());
        assert_eq!(actual.key_set_data.proper_signing_key_set, 2);
        assert_eq!(refs, BTreeMap::from([(2, 1)]));
    }
}
