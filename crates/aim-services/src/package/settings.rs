//! PackageManager's settings, `/data/system/packages.xml`: every package
//! with its code path, flags, app id, install source and signatures, the
//! updated system packages' originals, shared users, the key sets, the
//! legacy permission definitions and the settings' versions, as
//! `Settings.readSettingsLPw` reads them (`writeLPr` writes them).
//!
//! Not modelled yet: what only older platforms write (per-package
//! `<perms>`, `<enabled-components>`, `<disabled-components>`,
//! `<domain-verification>`, the single-user
//! preferred activities, `last-platform-version`, the pre-M `flags`), which
//! the original reads only to migrate.

use aim_android_xml::Element;

use super::domain_verification;
use super::{children, string};

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
    /// `PermissionInfo.protectionLevel`, as `fixProtectionLevel` leaves it.
    pub protection_level: i32,
    /// A dynamic permission's icon and label; `None` for a manifest one.
    pub dynamic: Option<(i32, Option<String>)>,
}

/// A `PackageSetting`: a `<package>`, or an `<updated-package>` (the
/// system package an update replaced), whose flags the reader derives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Package {
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
    /// Whether `app_id` is a shared user's.
    pub shared_user: bool,
    pub is_sdk_library: bool,
    pub install_source: InstallSource,
    pub volume_uuid: Option<String>,
    pub category_hint: i32,
    pub update_available: bool,
    pub force_queryable: bool,
    pub pending_restore: bool,
    pub debuggable: bool,
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
    pub mime_groups: Vec<(String, Vec<String>)>,
    /// Split names and revision codes, kept for a package without code
    /// (archived, or deleted keeping its data).
    pub split_versions: Vec<(String, i32)>,
}

impl Package {
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
    /// Java-serialized public keys from the feed or native verified SPKI.
    pub public_keys: Option<Vec<Option<super::pkg::Serialized>>>,
    pub past_signatures: Option<Vec<(Vec<u8>, i32)>>,
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
    pub version: Option<i32>,
    pub public_keys: Vec<(i64, Vec<u8>)>,
    pub key_sets: Vec<(i64, Vec<i64>)>,
    pub last_issued_key_id: i64,
    pub last_issued_key_set_id: i64,
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
type Certificates = Vec<Option<Vec<u8>>>;

impl Settings {
    /// The settings in the document whose root is `root`.
    pub fn parse(root: &Element) -> Result<Settings, String> {
        let mut s = Settings::default();
        let mut certificates = Certificates::new();
        for e in root.children() {
            match e.name.as_str() {
                "package" => {
                    if let Some(p) = package(e, &mut certificates)? {
                        s.packages.push(p);
                    }
                }
                "updated-package" => s.disabled_system_packages.push(updated_package(e)?),
                "shared-user" => {
                    if let Some(u) = shared_user(e, &mut certificates)? {
                        s.shared_users.push(u);
                    }
                }
                "permissions" => s.permissions = permissions(e)?,
                "permission-trees" => s.permission_trees = permissions(e)?,
                "renamed-package" => {
                    if let (Some(new), Some(old)) = (string(e, "new"), string(e, "old")) {
                        s.renamed_packages.push((new, old));
                    }
                }
                "verifier" => s.verifier = string(e, "device"),
                "keyset-settings" => {
                    s.key_sets = key_sets(e)?;
                    if s.key_sets.version.is_none() {
                        for package in &mut s.packages {
                            package.key_set_data = KeySetData::default();
                        }
                    }
                }
                "domain-verifications" => s.domain_verification.read(e)?,
                "domain-verifications-legacy" => s.domain_verification.read_legacy(e)?,
                "version" => {
                    let volume_uuid = string(e, "volumeUuid");
                    s.versions.retain(|v| v.volume_uuid != volume_uuid);
                    s.versions.push(Version {
                        volume_uuid,
                        sdk_version: required(e, "sdkVersion", e.int("sdkVersion")?)?,
                        database_version: required(
                            e,
                            "databaseVersion",
                            e.int("databaseVersion")?,
                        )?,
                        build_fingerprint: string(e, "buildFingerprint"),
                        fingerprint: string(e, "fingerprint"),
                    });
                }
                _ => {}
            }
        }
        // `readLPw`: a package of an unknown shared user is dropped, an
        // updated system package of a shared user's app id belongs to it.
        let shared: Vec<i32> = s.shared_users.iter().map(|u| u.app_id).collect();
        s.packages
            .retain(|p| !p.shared_user || shared.contains(&p.app_id));
        for p in &mut s.disabled_system_packages {
            p.shared_user |= shared.contains(&p.app_id);
        }
        Ok(s)
    }
}

/// `getAttributeInt(null, name)` without a default: the attribute must be
/// there.
fn required<T>(e: &Element, name: &str, v: Option<T>) -> Result<T, String> {
    v.ok_or_else(|| format!("<{}> without {name}", e.name))
}

fn permissions(e: &Element) -> Result<Vec<Permission>, String> {
    let mut out: Vec<Permission> = Vec::new();
    for item in children(e, "item") {
        let (Some(name), Some(package)) = (string(item, "name"), string(item, "package")) else {
            continue;
        };
        let dynamic = string(item, "type").as_deref() == Some("dynamic");
        let permission = Permission {
            protection_level: fix_protection_level(item.int("protection")?.unwrap_or(0)),
            dynamic: dynamic
                .then(|| Ok::<_, String>((item.int("icon")?.unwrap_or(0), string(item, "label"))))
                .transpose()?,
            name,
            package,
        };
        out.retain(|p| p.name != permission.name);
        out.push(permission);
    }
    Ok(out)
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
        version_code: e.long("version")?.unwrap_or(0),
        target_sdk_version: e.int("targetSdkVersion")?.unwrap_or(0),
        restrict_update_hash: e.bytes_base64("restrictUpdateHash")?,
        scanned_as_stopped_system_app: e.bool("scannedAsStoppedSystemApp")?.unwrap_or(false),
        last_modified_time: match e.long_hex("ft")?.unwrap_or(0) {
            0 => e.long("ts")?.unwrap_or(0),
            ft => ft,
        },
        last_update_time: e.long_hex("ut")?.unwrap_or(0),
        app_metadata_file_path: string(e, "appMetadataFilePath"),
        app_metadata_source: e.int("appMetadataSource")?.unwrap_or(0),
        category_hint: CATEGORY_UNDEFINED,
        ..Package::default()
    };
    // `userId` and `sharedUserId` are the app id's historical names.
    p.app_id = e.int("userId")?.unwrap_or(0);
    if p.app_id <= 0 {
        p.app_id = e.int("sharedUserId")?.unwrap_or(0);
        p.shared_user = p.app_id > 0;
    }
    Ok(Some(p))
}

/// The elements both kinds of package hold.
fn libraries(p: &mut Package, child: &Element) -> Result<bool, String> {
    match child.name.as_str() {
        "uses-static-lib" => {
            let version = child.long("version")?.unwrap_or(-1);
            if let Some(name) = string(child, "name").filter(|_| version >= 0) {
                match p.uses_static_libraries.iter_mut().find(|(n, _)| *n == name) {
                    Some(lib) => lib.1 = version,
                    None => p.uses_static_libraries.push((name, version)),
                }
            }
        }
        "uses-sdk-lib" => {
            let version_major = child.long("version")?.unwrap_or(-1);
            let optional = child.bool("optional")?.unwrap_or(true);
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
fn package(e: &Element, certificates: &mut Certificates) -> Result<Option<Package>, String> {
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
    p.is_sdk_library = e.bool("isSdkLibrary")?.unwrap_or(false);
    p.flags = e.int("publicFlags")?.unwrap_or(FLAG_SYSTEM);
    p.private_flags = e.int("privateFlags")?.unwrap_or(0);
    p.legacy_first_install_time = e.long_hex("it")?.unwrap_or(0);
    p.install_source = InstallSource {
        installer: string(e, "installer"),
        installer_uid: e.int("installerUid")?.unwrap_or(INVALID_UID),
        update_owner: string(e, "updateOwner"),
        installer_attribution_tag: string(e, "installerAttributionTag"),
        package_source: e.int("packageSource")?.unwrap_or(0),
        is_orphaned: e.bool("isOrphaned")?.unwrap_or(false),
        initiating_package: string(e, "installInitiator"),
        initiating_package_uninstalled: e.bool("installInitiatorUninstalled")?.unwrap_or(false),
        initiating_package_signatures: None,
        originating_package: string(e, "installOriginator"),
    };
    p.install_source = p.install_source.normalized()?;
    p.volume_uuid = string(e, "volumeUuid");
    p.category_hint = e.int("categoryHint")?.unwrap_or(CATEGORY_UNDEFINED);
    p.update_available = e.bool("updateAvailable")?.unwrap_or(false);
    p.force_queryable = e.bool("forceQueryable")?.unwrap_or(false);
    p.pending_restore = e.bool("pendingRestore")?.unwrap_or(false);
    p.debuggable = e.bool("debuggable")?.unwrap_or(false);

    p.base_revision_code = e.int("baseRevisionCode")?.unwrap_or(0);
    p.set_page_size_compat(e.int("pageSizeCompat")?.unwrap_or(0))?;
    p.domain_set_id = string(e, "domainSetId").filter(|id| !id.is_empty());
    for child in e.children() {
        if libraries(&mut p, child)? {
            continue;
        }
        let data = &mut p.key_set_data;
        match child.name.as_str() {
            "sigs" => p.signatures = signatures(child, certificates)?,
            "install-initiator-sigs" => {
                p.install_source.initiating_package_signatures = signatures(child, certificates)?
            }
            "proper-signing-keyset" => {
                data.proper_signing_key_set = identifier(child)?;
            }
            "upgrade-keyset" => data.add_upgrade_key_set(identifier(child)?),
            "defined-keyset" => {
                let alias = string(child, "alias");
                data.add_defined_key_set(identifier(child)?, alias);
            }
            "mime-group" => {
                if let Some(group) = string(child, "name") {
                    let types = children(child, "mime-type")
                        .filter_map(|t| string(t, "value"))
                        .collect();
                    p.mime_groups.push((group, types));
                }
            }
            "split-version" => {
                let revision = child.int("version")?.unwrap_or(-1);
                if let Some(name) = string(child, "name").filter(|_| revision >= 0) {
                    match p.split_versions.iter_mut().find(|(n, _)| *n == name) {
                        Some(split) => split.1 = revision,
                        None => p.split_versions.push((name, revision)),
                    }
                }
            }
            _ => {}
        }
    }
    p.install_source = p.install_source.normalized()?;
    Ok(Some(p))
}

fn identifier(e: &Element) -> Result<i64, String> {
    required(e, "identifier", e.long("identifier")?)
}

/// `readSharedUserLPw`: `None` for an entry the original drops.
fn shared_user(e: &Element, certificates: &mut Certificates) -> Result<Option<SharedUser>, String> {
    let (Some(name), Some(app_id)) = (string(e, "name"), e.int("userId")?.filter(|id| *id != 0))
    else {
        return Ok(None);
    };
    let mut u = SharedUser {
        name,
        app_id,
        flags: if e.bool("system")?.unwrap_or(false) {
            FLAG_SYSTEM
        } else {
            0
        },
        signatures: None,
    };
    for sigs in children(e, "sigs") {
        u.signatures = signatures(sigs, certificates)?;
    }
    Ok(Some(u))
}

/// `PackageSignatures.readXml`: `None` without a count, as the original
/// skips such an element. A certificate whose key is missing or bad is
/// left out, as the original leaves it out.
fn signatures(e: &Element, certificates: &mut Certificates) -> Result<Option<Signatures>, String> {
    let Some(count) = e.int("count")? else {
        return Ok(None);
    };
    let mut s = Signatures {
        scheme_version: e.int("schemeVersion")?.unwrap_or(SCHEME_UNKNOWN),
        ..Signatures::default()
    };
    let mut read = 0;
    for child in e.children() {
        match child.name.as_str() {
            "cert" => {
                if read < count
                    && let Some(key) = certificate(child, certificates)?
                {
                    s.signatures.push(key);
                }
                read += 1;
            }
            "pastSigs" if s.past_signatures.is_none() => {
                let Some(count) = child.int("count")? else {
                    continue;
                };
                let mut past = Vec::new();
                for cert in children(child, "cert").take(count.max(0) as usize) {
                    let flags = cert.int("flags")?.unwrap_or(0);
                    if let Some(key) = certificate(cert, certificates)? {
                        past.push((key, flags));
                    }
                }
                s.past_signatures = Some(past);
            }
            _ => {}
        }
    }
    Ok(Some(s))
}

/// A `<cert>`'s key: its own, which takes its index in the document's
/// table, or the one an earlier `<cert>` gave its index.
fn certificate(e: &Element, certificates: &mut Certificates) -> Result<Option<Vec<u8>>, String> {
    let Some(index) = e.int("index")?.filter(|i| *i >= 0).map(|i| i as usize) else {
        return Ok(None);
    };
    Ok(match e.bytes_hex("key") {
        Ok(Some(key)) => {
            if certificates.len() <= index {
                certificates.resize(index + 1, None);
            }
            certificates[index] = Some(key.clone());
            Some(key)
        }
        Ok(None) => certificates.get(index).cloned().flatten(),
        Err(_) => None,
    })
}

/// `KeySetManagerService.readKeySetsLPw`. A document without a version
/// is from before key sets were versioned; the original then drops every
/// package's key set data.
fn key_sets(e: &Element) -> Result<KeySets, String> {
    let mut k = KeySets {
        version: e.int("version")?,
        ..KeySets::default()
    };
    if k.version.is_none() {
        return Ok(k);
    }
    for child in e.children() {
        match child.name.as_str() {
            "keys" => {
                for key in children(child, "public-key") {
                    if let Some(value) = key.bytes_base64("value")? {
                        k.public_keys.push((identifier(key)?, value));
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
