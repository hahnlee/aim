//! Explicit C image boot configuration from mounted and retained native owners.
use super::{
    apps_filter::Config as FilterConfig, boot_image, info::flags::*, intent::Intent, model::State,
    parse::resources::Config, resolve::Resolution,
};
use crate::{
    NativeServices, system::package_boot_inputs::Cli, system::package_runtime::BootFacts,
    system_package_persistence_init,
};
use aim_storage::guest_inode::{self, GuestInode};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Typed original UM/Resources/kernel inputs. Implementations call retained
/// independent leaves; none of these are reconstructed from original PMS/feed.
pub trait OriginalInputs {
    fn users(&self) -> Result<Vec<u32>, String>;
    fn resources(&self) -> Result<Config, String>;
    fn density(&self) -> Result<i32, String>;
    fn factory_test(&self) -> Result<bool, String>;
    fn dependency_installer_enabled(&self) -> Result<bool, String>;
    fn fix_system_apps_first_install_time(&self) -> Result<bool, String>;
    /// Actual creation metadata from the native writable-file owner, used only
    /// when inspection proves the corresponding original file is absent.
    fn creation_inode(&self, guest_path: &str) -> Result<GuestInode, String>;
    fn check_attachment(&self) -> Result<(), String>;
}

/// Values owned by actual native scanning, not a default empty inventory.
pub struct ScanEffects {
    pub boot_apex_changed: bool,
}
pub struct Invocation {
    /// Exact caller invocation policy; factory-test is checked against the
    /// original SystemServer invocation argument retained by its typed leaf.
    pub scan: Cli,
    /// Parent provides this only after the native redirect entered without
    /// starting original PMS, or after its original writer was actually stopped.
    pub original_writer_stopped: bool,
}
pub struct Early {
    image: Arc<boot_image::Owner>,
    policy: Cli,
    resources: Config,
    persistence: system_package_persistence_init::Inputs,
    properties: crate::system_package_installer_init::Properties,
    density: i32,
    dependency_installer_enabled: bool,
    fix_system_apps_first_install_time: bool,
}
impl Early {
    pub(crate) fn density_dpi(&self) -> i32 { self.density }
    /// None is the default image's original-PMS path; no native package policy
    /// is configured unless the actual image service list explicitly names it.
    pub fn read(
        configured_services: &[String],
        image: &Path,
        data: &Path,
        original_roots: &[PathBuf],
        invocation: Invocation,
        original: &Source,
        properties: crate::system_package_installer_init::Properties,
    ) -> Result<Option<Self>, String> {
        if !configured_services.iter().any(|name| name == "package") {
            return Ok(None);
        }
        if !invocation.original_writer_stopped {
            return Err("native package C invocation has no stopped-writer evidence".into());
        }
        let original_roots = original_root_inventory(image, original_roots)?;
        original.check_attachment()?;
        let image = boot_image::Owner::open(image, data, &original_roots)?;
        // Read from the live original property source now as well as retaining
        // it for later requests; a missing source cannot become an empty map.
        properties().map_err(|error| error.message)?;
        if invocation.scan.factory_test != original.factory_test()? {
            return Err("native scan factory-test mode differs from original invocation".into());
        }
        let users = original.users()?;
        if users.is_empty()
            || users.iter().any(|user| *user > i32::MAX as u32)
            || users
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != users.len()
        {
            return Err("original UM boot inventory is absent or malformed".into());
        }
        let resources = original.resources()?;
        let density = original.density()?;
        if density <= 0 {
            return Err("original display density is unavailable".into());
        }
        let inode = |guest: &str| -> Result<GuestInode, String> {
            let host = image
                .host_path(guest)
                .ok_or("boot inode VFS mapping absent")?;
            let value = match std::fs::symlink_metadata(&host) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err("boot inode is a symlink".into());
                    }
                    guest_inode::read(&host)
                        .map_err(|error| error.to_string())?
                        .ok_or("boot inode guest metadata was not recorded")?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    original.creation_inode(guest)?
                }
                Err(error) => return Err(error.to_string()),
            };
            if value.uid.is_none() || value.gid.is_none() || value.mode.is_none() {
                return Err("boot inode creation owner is incomplete".into());
            }
            Ok(value)
        };
        let session_inode = inode("/data/system/install_sessions.xml")?;
        let stage_inode = original.creation_inode("/data/app/vmdl.session.tmp")?;
        if stage_inode.uid.is_none() || stage_inode.gid.is_none() || stage_inode.mode.is_none() {
            return Err("installer stage creation owner is incomplete".into());
        }
        let mut runtime_inodes = BTreeMap::new();
        for user in &users {
            runtime_inodes.insert(
                *user,
                inode(&format!(
                    "/data/misc_de/{user}/apexdata/com.android.permission/runtime-permissions.xml"
                ))?,
            );
        }
        original.check_attachment()?;
        let persistence = system_package_persistence_init::Inputs {
            data: image.data.clone(),
            original_roots: image.original_roots.clone(),
            original_writer_stopped: true,
            users,
            session_inode,
            stage_inode,
            runtime_inodes,
            controller_version: CONTROLLER_VERSION_DEFERRED,
        };
        Ok(Some(Self {
            image,
            policy: invocation.scan,
            resources,
            persistence,
            properties,
            density,
            dependency_installer_enabled: original.dependency_installer_enabled()?,
            fix_system_apps_first_install_time: original.fix_system_apps_first_install_time()?,
        }))
    }
    /// Configure only values required before BootSession.begin. The controller
    /// sentinel is a deferred phase marker and must never reach runtime storage.
    pub fn configure(self: &Arc<Self>, services: &NativeServices) -> Result<(), String> {
        self.configure_system(&services.system)
    }
    pub fn configure_system(self: &Arc<Self>, system: &crate::System) -> Result<(), String> {
        system
            .configure_native_package_image(
                &self.image.image,
                &self.image.data,
                &self.image.original_roots,
            )
            .map_err(|error| error.message)?;
        let bridge=system.package_bootstrap().map_err(|error|error.message)?;
        self.configure_system_for(system,&bridge)
    }
    fn configure_system_for(self:&Arc<Self>,system:&crate::System,bridge:&Arc<super::bootstrap::Bridge>)->Result<(),String>{
        system.configure_native_package_early_for(bridge,self.clone(),self.policy.clone(),self.resources,self.persistence.clone())
            .map_err(|error|error.message)
    }
    /// The root calls this only from the explicit native entry, which is the
    /// stopped-writer capability: original PMS main has not been invoked.
    pub fn from_native_entry(
        system: &crate::System,
        bridge:&Arc<super::bootstrap::Bridge>,
        source: &Source,
        properties: crate::system_package_installer_init::Properties,
    ) -> Result<Arc<Self>, String> {
        let image = system
            .native_package_image()
            .map_err(|error| error.message)?;
        let early = Self::read(
            &["package".into()],
            &image.image,
            &image.data,
            &image.original_roots,
            Invocation {
                scan: source.boot_cli()?,
                original_writer_stopped: true,
            },
            source,
            properties,
        )?
        .ok_or("explicit native package entry did not prepare C inputs")?;
        let early = Arc::new(early);
        early.configure_system_for(system,bridge)?;
        Ok(early)
    }
    pub fn after_scan(
        &self,
        accepted_factory_state: Arc<State>,
        effects: ScanEffects,
    ) -> Result<Late, String> {
        let controller = accepted_factory_state
            .system
            .permission_controller_package
            .as_ref()
            .ok_or("native factory permission-controller selection unavailable")?
            .as_deref()
            .ok_or("native factory permission-controller selection empty")?;
        let controller = accepted_factory_state
            .packages
            .get(controller)
            .filter(|package| package.is.system && package.is.privileged && package.pkg.is_some())
            .ok_or("native accepted permission-controller code absent")?;
        let controller_version = controller.version_code;
        let resolution = Resolution::new(
            accepted_factory_state.clone(),
            &FilterConfig {
                force_system_packages_queryable: accepted_factory_state
                    .system
                    .force_system_packages_queryable,
                force_queryable_packages: accepted_factory_state
                    .system
                    .force_queryable_packages
                    .clone(),
            },
        )
        .map_err(|error| format!("native storage manager resolution: {error:?}"))?;
        let matches = resolution
            .query_intent_activities(
                &Intent {
                    action: Some("android.os.storage.action.MANAGE_STORAGE".into()),
                    ..Default::default()
                },
                None,
                MATCH_SYSTEM_ONLY
                    | MATCH_DIRECT_BOOT_AWARE
                    | MATCH_DIRECT_BOOT_UNAWARE
                    | MATCH_DISABLED_COMPONENTS,
                0,
                1000,
            )
            .map_err(|error| format!("native storage manager selection: {error:?}"))?;
        let storage_manager_package =
            (matches.len() == 1).then(|| matches[0].component().0.to_owned());
        let facts = BootFacts {
            properties: self.properties.clone(),
            dependency_installer_enabled: self.dependency_installer_enabled,
            density: self.density,
            storage_manager_package,
            fix_system_apps_first_install_time: self.fix_system_apps_first_install_time,
            boot_apex_changed: effects.boot_apex_changed,
        };
        let mut persistence = self.persistence.clone();
        persistence.controller_version = controller_version;
        Ok(Late { persistence, facts })
    }
}

pub const CONTROLLER_VERSION_DEFERRED: i64 = -1;
pub struct Late {
    pub persistence: system_package_persistence_init::Inputs,
    pub facts: BootFacts,
}

/// Concrete independent typed leaf adapter. Its resource configuration is the
/// actual applied resource-owner value passed by the parent boot callback.
pub struct Source {
    node: Arc<aim_binder_host::local::Strong>,
    data: PathBuf,
    configuration: Config,
    check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}
impl Source {
    pub fn boot_cli(&self) -> Result<Cli, String> {
        let cache = self.invoke(aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::GET_PARSER_CACHE_DIRECTORY,
            |_| {}, |reader| reader.read_string16())?;
        let parser_cache = cache
            .map(|guest| {
                let suffix = Path::new(&guest)
                    .strip_prefix("/data")
                    .map_err(|_| "original parser cache is outside writable data")?;
                if suffix
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err("original parser cache has traversal components");
                }
                Ok(self.data.join(suffix))
            })
            .transpose()
            .map_err(str::to_owned)?;
        // InitAppsHelper109–116: SCAN_BOOTING|SCAN_INITIAL (upgrade bit is
        // lifecycle-owned). ScanPackageUtils228: allowInstall=true;
        // InstallPackageHelper3862: USER_SYSTEM for ordinary APK directories.
        // No INSTANT/VIRTUAL_PRELOAD/STOPPED/UPDATE_TIME bits are in that base.
        Ok(Cli {
            factory_test: self.factory_test()?,
            install_user: Some(0),
            allow_install: true,
            instant_app: false,
            virtual_preload: false,
            stopped_system_app: false,
            compat_16kb_disabled: false,
            update_time: false,
            scan_user: 0,
            parser_cache,
        })
    }
    pub fn capture(
        node: Arc<aim_binder_host::local::Strong>,
        data: &Path,
        check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Result<Self, String> {
        check()?;
        let temporary = Rpc {
            node: node.clone(),
            check: check.clone(),
        };
        let bytes = temporary
            .invoke(
                aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::CAPTURE_RESOURCES,
                |_| {},
                aim_service_aidl::read_byte_array,
            )?
            .ok_or("original resource configuration record absent")?;
        let configuration = decode_resources(&bytes)?;
        Ok(Self {
            node,
            data: data.canonicalize().map_err(|error| error.to_string())?,
            configuration,
            check,
        })
    }
    fn invoke<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut aim_binder_host::parcel::Parcel),
        read: impl FnOnce(
            &mut aim_binder_host::parcel::Reader<'_>,
        ) -> aim_binder_host::parcel::Result<T>,
    ) -> Result<T, String> {
        Rpc {
            node: self.node.clone(),
            check: self.check.clone(),
        }
        .invoke(code, write, read)
    }
    pub fn get_live_properties(&self, keys: &[String]) -> Result<BTreeMap<String, String>, String> {
        if keys.is_empty()
            || keys.iter().any(String::is_empty)
            || keys.iter().collect::<std::collections::BTreeSet<_>>().len() != keys.len()
        {
            return Err("actual property inventory absent or duplicated".into());
        }
        let bytes = self
            .invoke(
                aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::GET_LIVE_PROPERTIES,
                |parcel| {
                    parcel.write_i32(keys.len() as i32);
                    for key in keys {
                        parcel.write_string16(Some(key));
                    }
                },
                aim_service_aidl::read_byte_array,
            )?
            .ok_or("live property record absent")?;
        let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        let bad = |error| format!("live property record: {error}");
        if reader.read_i32().map_err(bad)? != 1
            || reader.read_i32().map_err(bad)? != keys.len() as i32
        {
            return Err("live property record schema or count differs".into());
        }
        let mut values = BTreeMap::new();
        for expected in keys {
            let key = reader
                .read_string16()
                .map_err(bad)?
                .ok_or("live property key null")?;
            let value = reader
                .read_string16()
                .map_err(bad)?
                .ok_or("live property value null")?;
            if key != *expected || values.insert(key, value).is_some() {
                return Err("live property record key order differs or duplicate".into());
            }
        }
        if reader.remaining() != 0 {
            return Err("live property record trailing data".into());
        }
        Ok(values)
    }
    pub fn runtime_properties(
        self: &Arc<Self>,
        keys: Vec<String>,
    ) -> Result<crate::system_package_installer_init::Properties, String> {
        self.get_live_properties(&keys)?;
        let source = self.clone();
        Ok(Arc::new(move || {
            source.get_live_properties(&keys).map_err(|message| {
                aim_binder_host::parcel::Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    message,
                )
            })
        }))
    }
}
pub type OriginalSource = Source;
/// A derived image records original content identity, not the host path of a
/// mounted original. Verify the receipt and any actually supplied original
/// mounts; never relabel the derived root as an original input to fill a list.
fn original_root_inventory(image: &Path, supplied: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    use sha2::{Digest, Sha256};
    let image = image.canonicalize().map_err(|error| error.to_string())?;
    let receipt = match std::fs::read(image.join(".overlay-receipt")) {
        Ok(receipt) => Some(receipt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let expected = match receipt {
        Some(receipt) => {
            let identity = aim_android_image::identity::read_tree_identity(&image)?
                .ok_or("derived image receipt has no content identity")?;
            let hash: String = Sha256::digest(&receipt)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if identity != hash {
                return Err("derived image receipt content identity differs".into());
            }
            let text = std::str::from_utf8(&receipt).map_err(|error| error.to_string())?;
            let mut lines = text.lines();
            if lines.next() != Some("aim-android-image identity v1") {
                return Err("derived image receipt schema differs".into());
            }
            let original = lines
                .next()
                .and_then(|line| line.strip_prefix("original "))
                .ok_or("derived image original source identity is absent")?;
            Some(aim_android_image::identity::parse_hex_identity(original)?)
        }
        None => {
            if supplied.is_empty() {
                return Err(
                    "image has neither derived provenance nor original mount inventory".into(),
                );
            }
            None
        }
    };
    let mut roots = Vec::new();
    for root in supplied {
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        if expected.is_some() && root == image {
            return Err("derived image cannot be recorded as an original input".into());
        }
        let identity = aim_android_image::identity::original_identity(&root, None)?
            .ok_or("original mount has no recorded source identity")?;
        if expected
            .as_ref()
            .is_some_and(|expected| expected != &identity)
        {
            return Err("mounted original identity differs from derived source receipt".into());
        }
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}
fn decode_resources(bytes: &[u8]) -> Result<Config, String> {
    let mut reader = aim_binder_host::parcel::Reader::new(bytes, &[]);
    let bad = |error| format!("original resource configuration record: {error}");
    if reader.read_i32().map_err(bad)? != 1 || reader.read_i32().map_err(bad)? != 24 {
        return Err("original resource configuration schema/count mismatch".into());
    }
    let mut fields = [0i32; 24];
    for field in &mut fields {
        *field = reader.read_i32().map_err(bad)?;
    }
    if reader.remaining() != 0 {
        return Err("original resource configuration trailing data".into());
    }
    // JNI NativeSetConfiguration casts these exact Java values to uint8/uint16;
    // ResTable_config's minorVersion is initialized by its native memset(0).
    Ok(Config {
        mcc: fields[0] as u16,
        mnc: fields[1] as u16,
        language: [fields[2] as u8, fields[3] as u8],
        country: [fields[4] as u8, fields[5] as u8],
        orientation: fields[6] as u8,
        touchscreen: fields[7] as u8,
        density: fields[8] as u16,
        keyboard: fields[9] as u8,
        navigation: fields[10] as u8,
        input_flags: fields[11] as u8,
        grammatical_inflection: fields[12] as u8,
        screen_width: fields[13] as u16,
        screen_height: fields[14] as u16,
        sdk_version: fields[15] as u16,
        minor_version: fields[16] as u16,
        screen_layout: fields[17] as u8,
        ui_mode: fields[18] as u8,
        smallest_screen_width_dp: fields[19] as u16,
        screen_width_dp: fields[20] as u16,
        screen_height_dp: fields[21] as u16,
        screen_layout2: fields[22] as u8,
        color_mode: fields[23] as u8,
    })
}
struct Rpc {
    node: Arc<aim_binder_host::local::Strong>,
    check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}
impl Rpc {
    fn invoke<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut aim_binder_host::parcel::Parcel),
        read: impl FnOnce(
            &mut aim_binder_host::parcel::Reader<'_>,
        ) -> aim_binder_host::parcel::Result<T>,
    ) -> Result<T, String> {
        use aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf as api;
        (self.check)()?;
        let mut request = aim_binder_host::parcel::Parcel::new();
        request.write_interface_token(api::DESCRIPTOR);
        write(&mut request);
        let reply = self
            .node
            .transact(code, &request, false)
            .map_err(|error| format!("boot configuration leaf: {error}"))?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(|error| error.to_string())?
            .map_err(|error| error.message)?;
        let value = read(&mut reader).map_err(|error| error.to_string())?;
        if reader.remaining() != 0 {
            return Err("boot configuration leaf trailing data".into());
        }
        (self.check)()?;
        Ok(value)
    }
}
impl OriginalInputs for Source {
    fn users(&self) -> Result<Vec<u32>, String> {
        let users = self
            .invoke(
                aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::GET_USER_IDS,
                |_| {},
                aim_service_aidl::read_int_array,
            )?
            .ok_or("actual original UM returned no boot user inventory")?;
        let mut output = Vec::with_capacity(users.len());
        for user in users {
            if user < 0 || output.contains(&(user as u32)) {
                return Err("actual original UM returned malformed boot users".into());
            }
            output.push(user as u32);
        }
        if output.is_empty() {
            return Err("actual original UM returned an empty boot roster".into());
        }
        Ok(output)
    }
    fn resources(&self) -> Result<Config, String> {
        self.check_attachment()?;
        Ok(self.configuration)
    }
    fn density(&self) -> Result<i32, String> {
        self.invoke(
            aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::GET_DENSITY,
            |_| {},
            |reader| reader.read_i32(),
        )
    }
    fn factory_test(&self) -> Result<bool, String> {
        self.invoke(
            aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::IS_FACTORY_TEST,
            |_| {},
            |reader| reader.read_bool(),
        )
    }
    fn dependency_installer_enabled(&self) -> Result<bool, String> {
        self.invoke(aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::IS_DEPENDENCY_INSTALLER_ENABLED, |_| {}, |reader| reader.read_bool())
    }
    fn fix_system_apps_first_install_time(&self) -> Result<bool, String> {
        self.invoke(aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::FIX_SYSTEM_APPS_FIRST_INSTALL_TIME, |_| {}, |reader| reader.read_bool())
    }
    fn creation_inode(&self, guest_path: &str) -> Result<GuestInode, String> {
        let bytes = self
            .invoke(
                aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf::CREATION_INODE,
                |parcel| parcel.write_string16(Some(guest_path)),
                aim_service_aidl::read_byte_array,
            )?
            .ok_or("boot inode creation leaf returned null")?;
        let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        let uid = reader.read_i32().map_err(|error| error.to_string())?;
        let gid = reader.read_i32().map_err(|error| error.to_string())?;
        let mode = reader.read_i32().map_err(|error| error.to_string())?;
        if uid < 0 || gid < 0 || !(0..=0o7777).contains(&mode) || reader.remaining() != 0 {
            return Err("boot inode creation leaf malformed record".into());
        }
        Ok(GuestInode {
            uid: Some(uid as u32),
            gid: Some(gid as u32),
            mode: Some(mode as u32),
        })
    }
    fn check_attachment(&self) -> Result<(), String> {
        (self.check)()
    }
}


#[cfg(test)]
pub(crate) fn epoch_fixture_early(
    image:Arc<boot_image::Owner>,policy:Cli,resources:Config,
    persistence:system_package_persistence_init::Inputs,
)->Arc<Early>{
    Arc::new(Early{image,policy,resources,persistence,
        properties:Arc::new(||Ok([("ro.build.fingerprint".into(),"epoch-fixture".into())].into())),
        density:320,dependency_installer_enabled:false,fix_system_apps_first_install_time:false})
}
