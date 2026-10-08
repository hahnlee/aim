//! Concrete image/VFS inputs for native live-install metadata completion.
//! ABI and seInfo follow PackageAbiHelper/SELinuxMMAC at android-16.0.0_r1.
use super::environment;
use crate::package::{
    bootstrap::Bridge, owner::seinfo, scan, system_config::Properties, write::Apks,
};
use aim_storage::guest_inode::{self, GuestInode};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Validates the retained bootstrap attachment before and after original
/// PlatformCompat calls; it must reject an owner replacement.
pub type Attachment = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

pub struct ImageInputs {
    data: PathBuf,
    apks: Arc<Apks>,
    abi: scan::AbiPolicy,
    preferred_abi: String,
    app_lib32_dir: String,
    seinfo: seinfo::Policy,
    compatibility: Arc<dyn scan::SeInfoCompatibility + Send + Sync>,
    directory_inode: GuestInode,
    file_inode: GuestInode,
}
impl ImageInputs {
    pub fn load(
        image: &Path,
        data: &Path,
        apks: Arc<Apks>,
        properties: &Properties,
        bridge: Arc<Bridge>,
        attachment: Attachment,
    ) -> Result<Self, String> {
        attachment()?;
        let list = |name: &str| -> Result<Vec<String>, String> {
            let value =
                properties(name).ok_or_else(|| format!("missing image ABI property: {name}"))?;
            if value.is_empty() {
                return Ok(Vec::new());
            }
            let list: Vec<_> = value.split(',').map(str::to_owned).collect();
            if list.iter().any(|abi| abi.is_empty() || abi.trim() != abi) {
                return Err(format!("invalid image ABI list: {name}"));
            }
            Ok(list)
        };
        let all = list("ro.product.cpu.abilist")?;
        let bit32 = list("ro.product.cpu.abilist32")?;
        let bit64 = list("ro.product.cpu.abilist64")?;
        let preferred_abi = all
            .first()
            .cloned()
            .ok_or("image supports no application ABI")?;
        for abi in &all {
            if !bit32.contains(abi) && !bit64.contains(abi) {
                return Err(format!("image ABI has no supported bitness owner: {abi}"));
            }
        }
        for abi in bit32.iter().chain(&bit64) {
            if !all.contains(abi) {
                return Err(format!("bitness ABI absent from image ABI order: {abi}"));
            }
        }
        if bit32.iter().any(|abi| bit64.contains(abi)) {
            return Err("image ABI occurs in both bitness lists".into());
        }
        let supported = scan::SupportedAbis {
            bit32: &bit32,
            bit64: &bit64,
        };
        let abi =
            scan::AbiPolicy::from_platform(&apks.platform, &all, &supported, properties.as_ref())?;
        let seinfo = seinfo::Policy::load(image)?;
        let data = std::fs::canonicalize(data).map_err(|error| error.to_string())?;
        let code_root = data.join("app");
        if !std::fs::symlink_metadata(&code_root)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err("native /data/app directory owner unavailable".into());
        }
        let owner = guest_inode::read(&code_root)
            .map_err(|error| error.to_string())?
            .ok_or("native /data/app guest inode is not recorded")?;
        // The original installer runs under SYSTEM_UID/SYSTEM_GID. Confirm
        // the writable VFS root belongs to that owner instead of consulting
        // the host process uid or stat's Darwin ownership.
        if owner.uid != Some(1000) || owner.gid != Some(1000) || owner.mode.is_none() {
            return Err(
                "native /data/app guest ownership differs from installer system owner".into(),
            );
        }
        // PackageManagerServiceUtils.copyPackage/makeDirRecursive and
        // DEFAULT_FILE_ACCESS_MODE set these code permissions explicitly.
        let directory_inode = GuestInode {
            uid: owner.uid,
            gid: owner.gid,
            mode: Some(0o755),
        };
        let file_inode = GuestInode {
            uid: owner.uid,
            gid: owner.gid,
            mode: Some(0o644),
        };
        let check = attachment.clone();
        let compatibility: Arc<dyn scan::SeInfoCompatibility + Send + Sync> =
            Arc::new(move |package: &crate::package::pkg::AndroidPackage| {
                check()?;
                let target = bridge
                    .seinfo_target_sdk(package)
                    .map_err(|error| format!("original install seInfo compatibility: {error:?}"))?;
                check()?;
                Ok(target)
            });
        attachment()?;
        Ok(Self {
            data,
            apks,
            abi,
            preferred_abi,
            app_lib32_dir: "/data/app-lib".into(),
            seinfo,
            compatibility,
            directory_inode,
            file_inode,
        })
    }

    pub fn configuration(self, services: Services) -> environment::Config {
        environment::Config {
            data: self.data,
            apks: self.apks,
            snapshots: services.snapshots,
            users: services.users,
            build_debuggable: services.build_debuggable,
            cross_user_suspensions: services.cross_user_suspensions,
            factory_test: services.factory_test,
            app_data_flags: services.app_data_flags,
            directory_inode: self.directory_inode,
            file_inode: self.file_inode,
            abi: self.abi,
            library_compatibility: services.library_compatibility,
            vendor_sdk: services.vendor_sdk,
            remove_test_base: services.remove_test_base,
            preferred_abi: self.preferred_abi,
            app_lib32_dir: self.app_lib32_dir,
            seinfo: self.seinfo,
            compatibility: self.compatibility,
            labeler: services.labeler,
            metadata: services.metadata,
            install_source: services.install_source,
            library_policy: services.library_policy,
            zip_clock: services.zip_clock,
            app_data: services.app_data,
            rollback_app_data: services.rollback_app_data,
            commit_app_data: services.commit_app_data,
            clear_code_cache: services.clear_code_cache,
            post_install_users: services.post_install_users,
            permissions: services.permissions,
            release_permissions: services.release_permissions,
            effects: services.effects,
            code_resources: services.code_resources,
            publish: services.publish,
            completion: services.completion,
        }
    }
}

/// Existing service-owned install operations; no policy default replaces one.
pub struct Services {
    pub snapshots: Arc<crate::package::scan_snapshot::Store>,
    pub users: Vec<scan::User>,
    pub build_debuggable: bool,
    pub cross_user_suspensions: bool,
    pub factory_test: bool,
    pub library_compatibility: Arc<scan::LibraryCompatibility>,
    pub vendor_sdk: i32,
    pub remove_test_base: Arc<dyn Fn(&crate::package::pkg::AndroidPackage, bool) -> Result<Option<bool>, String> + Send + Sync>,
    pub app_data_flags: environment::AppDataFlags,
    pub labeler: environment::Labeler,
    pub metadata: environment::Metadata,
    pub install_source: environment::InstallSource,
    pub library_policy: environment::LibraryPolicy,
    pub zip_clock: environment::ZipClock,
    pub app_data: environment::AppDataCreate,
    pub rollback_app_data: environment::AppDataRollback,
    pub commit_app_data: environment::AppDataCommit,
    pub clear_code_cache: environment::CodeCacheClear,
    pub post_install_users: environment::PostInstallUsers,
    pub permissions: environment::RuntimePrepare,
    pub release_permissions: environment::ReservationRelease,
    pub effects: Arc<crate::package::effects::Owner>,
    pub code_resources: Arc<crate::package::owner::resources::CodeResources>,
    pub publish: environment::QueryPublication,
    pub completion: environment::Completion,
}
