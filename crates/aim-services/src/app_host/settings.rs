//! User 0's settings as SettingsProvider keeps them (`SettingsState` at
//! `android-16.0.0_r1`): read from the files it persisted
//! (`/data/system/users/0/settings_{system,secure,global,config}.xml`),
//! changed through the settings provider (`super::settings_provider`), and
//! written back as `doWriteState` writes them, in binary XML, a moment
//! after the first unwritten change (`scheduleWriteIfNeededLocked`) or at
//! once after a critical one (`persistSettingsLocked`). DeviceConfig's
//! flags (`settings_config.xml`) are written the same way, a write of an
//! aconfig flag staged (`staged/namespace*flag`) for the next boot as the
//! image's aconfig defaults (`super::device_config`) name them, and a
//! namespace's flags set at once (`setSettingsLocked`) unless the reset
//! that banned them is what they would be again (`isNewConfigBannedLocked`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use aim_android_xml::{Element, Node, Value};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::device_config::{AconfigDefaults, map_hash};
use crate::package::info::java_hash;

/// `SettingsState.SETTINGS_VERSION_NEW_ENCODING`, `VERSION_UNDEFINED`,
/// `MAX_LENGTH_PER_STRING`, `MAX_BYTES_PER_APP_PACKAGE_LIMITED`,
/// `WRITE_SETTINGS_DELAY_MILLIS`.
const SETTINGS_VERSION_NEW_ENCODING: i32 = 121;
const VERSION_UNDEFINED: i32 = -1;
const MAX_LENGTH_PER_STRING: usize = 32768;
const MAX_BYTES_PER_APP_PACKAGE_LIMITED: i32 = 40000;
const WRITE_SETTINGS_DELAY: Duration = Duration::from_millis(200);
/// `SettingsState.SYSTEM_PACKAGE_NAME`, `NULL_VALUE_OLD_STYLE`,
/// `CONFIG_STAGED_PREFIX`.
pub const SYSTEM_PACKAGE_NAME: &str = "android";
const NULL_VALUE_OLD_STYLE: &str = "null";
const CONFIG_STAGED_PREFIX: &str = "staged/";
/// `Settings.Global.TRANSIENT_SETTINGS`, `Settings.Global.SECURE_FRP_MODE`.
const TRANSIENT_SETTINGS: &[&str] = &["clockwork_home_ready"];
const SECURE_FRP_MODE: &str = "secure_frp_mode";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Table {
    System,
    Secure,
    Global,
    /// DeviceConfig's flags, named `namespace/flag`.
    Config,
}

impl Table {
    pub const ALL: [Table; 4] = [Table::System, Table::Secure, Table::Global, Table::Config];

    fn file(self) -> &'static str {
        match self {
            Table::System => "settings_system.xml",
            Table::Secure => "settings_secure.xml",
            Table::Global => "settings_global.xml",
            Table::Config => "settings_config.xml",
        }
    }

    /// `getMaxBytesPerPackageForType`: only the system settings cap what
    /// an app may keep in them.
    fn capped(self) -> bool {
        self == Table::System
    }
}

/// `Settings.RESET_MODE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    PackageDefaults = 1,
    UntrustedDefaults = 2,
    UntrustedChanges = 3,
    TrustedDefaults = 4,
}

/// `SettingsState.Setting`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setting {
    pub name: String,
    pub value: Option<String>,
    pub default_value: Option<String>,
    pub package: Option<String>,
    pub tag: Option<String>,
    pub default_from_system: bool,
    pub preserved_in_restore: bool,
    pub id: i64,
}

/// An insertion's value and its caller (`insertSettingLocked`'s
/// arguments).
pub struct Change<'a> {
    pub value: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub make_default: bool,
    pub package: Option<&'a str>,
    /// `Setting.update`'s `callerSystem`: the caller is the system or a
    /// system package.
    pub caller_system: bool,
    pub overrideable_by_restore: bool,
}

/// `Setting.internValue`: the string `null` is no value.
fn intern(value: Option<&str>) -> Option<String> {
    value
        .filter(|v| *v != NULL_VALUE_OLD_STYLE)
        .map(str::to_string)
}

impl Setting {
    /// `Setting.update`: whether anything changed.
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        value: Option<&str>,
        set_default: bool,
        package: Option<&str>,
        tag: Option<&str>,
        caller_system: bool,
        overrideable_by_restore: bool,
        reset_to_default: bool,
        next_id: &mut i64,
    ) -> bool {
        let mut tag = tag;
        // Settings set by the system are always defaults.
        let set_default = set_default || caller_system;
        let mut default_value = self.default_value.as_deref();
        let mut default_from_system = self.default_from_system;
        if set_default {
            if value != self.default_value.as_deref() && (!default_from_system || caller_system) {
                default_value = value;
                if default_value.is_none() {
                    tag = None;
                    default_from_system = false;
                }
            }
            if !default_from_system && value.is_some() && caller_system {
                default_from_system = true;
            }
        }
        // `shouldPreserveSetting`: once preserved, always preserved.
        let preserved = if reset_to_default
            || value.is_some()
                && value == self.value.as_deref()
                && package == Some(SYSTEM_PACKAGE_NAME)
        {
            false
        } else {
            self.preserved_in_restore || !overrideable_by_restore
        };
        if value == self.value.as_deref()
            && default_value == self.default_value.as_deref()
            && package == self.package.as_deref()
            && tag == self.tag.as_deref()
            && default_from_system == self.default_from_system
            && preserved == self.preserved_in_restore
        {
            return false;
        }
        // `init`, which interns the values.
        self.default_value = intern(default_value);
        self.value = intern(value);
        self.tag = tag.map(str::to_string);
        self.package = package.map(str::to_string);
        self.default_from_system = default_from_system;
        self.preserved_in_restore = preserved;
        self.id = *next_id;
        *next_id += 1;
        true
    }
}

/// One table's `SettingsState`: its settings in their `ArrayMap`'s order.
struct State {
    version: i32,
    settings: Vec<Setting>,
    namespace_hashes: Vec<(Option<String>, Option<String>)>,
    next_id: i64,
    /// `mPackageToMemoryUsage`, in UTF-16 bytes, of a capped table.
    memory: HashMap<Option<String>, i32>,
    dirty: bool,
    /// When the next write is due.
    write_at: Option<Instant>,
}

/// `isBinary`: a string KXmlSerializer cannot write, kept in Base64.
fn is_binary(s: &str) -> bool {
    !s.chars()
        .all(|c| matches!(c as u32, 0x20..=0xd7ff | 0xe000..=0xfffd))
}

/// `String.length()`.
fn length(s: &str) -> usize {
    s.encode_utf16().count()
}

fn option_length(s: Option<&str>) -> i32 {
    s.map_or(0, length) as i32
}

impl State {
    fn new() -> State {
        State {
            version: VERSION_UNDEFINED,
            settings: Vec::new(),
            namespace_hashes: Vec::new(),
            next_id: 0,
            memory: HashMap::new(),
            dirty: false,
            write_at: None,
        }
    }

    /// `DatabaseHelper.loadGlobalSettings` / `loadSecureSettings`: the
    /// app host's vendor defaults stand for `def_device_provisioned=true`
    /// and `def_user_setup_complete=true`, since the Mac is already set up.
    /// Provision's `DefaultActivity` at android-16.0.0_r1 sets only these two.
    fn defaults(table: Table) -> Option<State> {
        let name = match table {
            Table::Global => "device_provisioned",
            Table::Secure => "user_setup_complete",
            _ => return None,
        };
        let mut state = State::new();
        // `SettingsProvider.UpgradeController.SETTINGS_VERSION` at the pin.
        state.version = 229;
        let mut setting = Setting {
            name: name.into(),
            ..Default::default()
        };
        setting.update(
            Some("1"),
            true,
            Some(SYSTEM_PACKAGE_NAME),
            None,
            true,
            true,
            false,
            &mut state.next_id,
        );
        state.put(setting);
        Some(state)
    }

    /// `ArrayMap.indexOf`: where `name` is, or where it would be added.
    fn index(&self, name: &str) -> Result<usize, usize> {
        let hash = java_hash(name);
        let mut i = self.settings.partition_point(|s| java_hash(&s.name) < hash);
        while i < self.settings.len() && java_hash(&self.settings[i].name) == hash {
            if self.settings[i].name == name {
                return Ok(i);
            }
            i += 1;
        }
        Err(i)
    }

    fn get(&self, name: &str) -> Option<&Setting> {
        self.index(name).ok().map(|i| &self.settings[i])
    }

    /// `ArrayMap.put`.
    fn put(&mut self, setting: Setting) {
        match self.index(&setting.name) {
            Ok(i) => self.settings[i] = setting,
            Err(i) => self.settings.insert(i, setting),
        }
    }

    /// `parseStateLocked`.
    fn parse(table: Table, bytes: &[u8]) -> Result<State, String> {
        let mut state = State::new();
        for root in aim_android_xml::read_sequence(bytes)? {
            match root.name.as_str() {
                "settings" => state.parse_settings(table, &root)?,
                "namespaceHashes" => state.namespace_hashes.extend(
                    root.children()
                        .filter(|e| e.name == "namespaceHash")
                        .map(|e| {
                            (
                                e.string("namespace").map(Into::into),
                                e.string("bannedHash").map(Into::into),
                            )
                        }),
                ),
                _ => {}
            }
        }
        Ok(state)
    }

    /// `parseSettingsLocked`.
    fn parse_settings(&mut self, table: Table, root: &Element) -> Result<(), String> {
        self.version = root.int("version")?.ok_or("<settings> without a version")?;
        let mut staged_applied = Vec::new();
        for setting in root.children().filter(|e| e.name == "setting") {
            let id = setting
                .string("id")
                .and_then(|id| id.parse::<i64>().ok())
                .ok_or("setting without an id")?;
            let mut name = setting
                .string("name")
                .ok_or("setting without a name")?
                .into_owned();
            let value = self.value_attribute(setting, "value", "valueBase64");
            let package = setting.string("package").map(Into::into);
            let default_value = self.value_attribute(setting, "defaultValue", "defaultValueBase64");
            let preserved_in_restore = setting.bool("preserve_in_restore")?.unwrap_or(false);
            let (mut tag, mut default_from_system) = (None, false);
            if default_value.is_some() {
                default_from_system = setting.bool("defaultSysSet")?.unwrap_or(false);
                tag = self.value_attribute(setting, "tag", "tagBase64");
            }
            if table == Table::Config {
                if staged_applied.contains(&name) {
                    continue;
                }
                if name.starts_with(CONFIG_STAGED_PREFIX) {
                    name = real_flag_name(&name);
                    staged_applied.push(name.clone());
                }
            }
            self.next_id = self.next_id.max(id + 1);
            self.put(Setting {
                name,
                value: intern(value.as_deref()),
                default_value: intern(default_value.as_deref()),
                package,
                tag,
                default_from_system,
                preserved_in_restore,
                id,
            });
        }
        Ok(())
    }

    /// `getValueAttribute`.
    fn value_attribute(&self, e: &Element, attr: &str, base64: &str) -> Option<String> {
        if self.version >= SETTINGS_VERSION_NEW_ENCODING {
            if let Some(value) = e.string(attr) {
                return Some(value.into_owned());
            }
            return e.string(base64).map(|b| base64_decode(&b));
        }
        e.string(attr)
            .filter(|v| v != NULL_VALUE_OLD_STYLE)
            .map(Into::into)
    }

    /// The documents `doWriteState` writes: the settings, then the
    /// namespaces' banned hashes.
    fn document(&self, table: Table) -> Vec<Element> {
        let mut settings = Vec::new();
        for s in &self.settings {
            // `isTransient`.
            if table == Table::Global && TRANSIENT_SETTINGS.contains(&s.name.as_str()) {
                continue;
            }
            // `writeSingleSetting` leaves out what it cannot write.
            let id = s.id.to_string();
            let Some(package) = s
                .package
                .as_deref()
                .filter(|p| !is_binary(&id) && !is_binary(&s.name) && !is_binary(p))
            else {
                continue;
            };
            let mut attrs = vec![string("id", &id), string("name", &s.name)];
            self.value_attribute_of(&mut attrs, "value", "valueBase64", s.value.as_deref());
            attrs.push(string("package", package));
            if let Some(default_value) = &s.default_value {
                self.value_attribute_of(
                    &mut attrs,
                    "defaultValue",
                    "defaultValueBase64",
                    Some(default_value),
                );
                attrs.push(("defaultSysSet".into(), Value::Bool(s.default_from_system)));
                self.value_attribute_of(&mut attrs, "tag", "tagBase64", s.tag.as_deref());
            }
            if s.preserved_in_restore {
                attrs.push(("preserve_in_restore".into(), Value::Bool(true)));
            }
            settings.push(Node::Element(element("setting", attrs, Vec::new())));
        }
        let hashes = self
            .namespace_hashes
            .iter()
            .filter_map(|(namespace, hash)| {
                Some(Node::Element(element(
                    "namespaceHash",
                    vec![
                        string("namespace", namespace.as_deref()?),
                        string("bannedHash", hash.as_deref()?),
                    ],
                    Vec::new(),
                )))
            })
            .collect();
        vec![
            element(
                "settings",
                vec![("version".into(), Value::Int(self.version))],
                settings,
            ),
            element("namespaceHashes", Vec::new(), hashes),
        ]
    }

    /// `setValueAttribute`.
    fn value_attribute_of(
        &self,
        attrs: &mut Vec<(String, Value)>,
        attr: &str,
        base64: &str,
        value: Option<&str>,
    ) {
        if self.version >= SETTINGS_VERSION_NEW_ENCODING {
            match value {
                None => {}
                Some(v) if is_binary(v) => attrs.push(string(base64, &base64_encode(v))),
                Some(v) => attrs.push(string(attr, v)),
            }
        } else {
            attrs.push(string(attr, value.unwrap_or(NULL_VALUE_OLD_STYLE)));
        }
    }

    /// `getNewMemoryUsagePerPackageLocked`.
    #[allow(clippy::too_many_arguments)]
    fn memory_usage(
        &self,
        capped: bool,
        package: Option<&str>,
        delta_key_length: i32,
        old_value: Option<&str>,
        new_value: Option<&str>,
        old_default: Option<&str>,
        new_default: Option<&str>,
    ) -> i32 {
        if exempt(capped, package) {
            return 0;
        }
        let current = self
            .memory
            .get(&package.map(str::to_string))
            .copied()
            .unwrap_or(0);
        let delta = (delta_key_length + option_length(new_value) + option_length(new_default)
            - option_length(old_value)
            - option_length(old_default))
            * 2;
        (current + delta).max(0)
    }

    /// `checkNewMemoryUsagePerPackageLocked`.
    fn check_memory_usage(capped: bool, package: Option<&str>, size: i32) -> Result<(), Exception> {
        if exempt(capped, package) || size <= MAX_BYTES_PER_APP_PACKAGE_LIMITED {
            return Ok(());
        }
        Err(Exception::new(
            EX_ILLEGAL_STATE,
            format!(
                "You are adding too many system settings. You should stop using system settings for app specific data package: {}",
                package.unwrap_or("null")
            ),
        ))
    }

    /// `updateMemoryUsagePerPackageLocked`.
    fn set_memory_usage(&mut self, capped: bool, package: Option<&str>, size: i32) {
        if !exempt(capped, package) {
            self.memory.insert(package.map(str::to_string), size);
        }
    }

    /// `scheduleWriteIfNeededLocked`: a write is already due while dirty.
    fn schedule_write(&mut self) {
        if !self.dirty {
            self.dirty = true;
            self.write_at = Some(Instant::now() + WRITE_SETTINGS_DELAY);
        }
    }

    /// `persistSettingsLocked`: a write right away.
    fn persist(&mut self) {
        self.write_at = Some(Instant::now());
    }

    /// `SettingsState.resetSettingLocked`.
    fn reset(&mut self, capped: bool, name: &str) -> Result<bool, Exception> {
        let Ok(i) = self.index(name) else {
            return Ok(false);
        };
        let s = &self.settings[i];
        let package = s.package.clone();
        let size = self.memory_usage(
            capped,
            package.as_deref(),
            0,
            s.value.as_deref(),
            s.default_value.as_deref(),
            s.default_value.as_deref(),
            s.default_value.as_deref(),
        );
        State::check_memory_usage(capped, package.as_deref(), size)?;
        let s = &mut self.settings[i];
        let default_value = s.default_value.clone();
        if !s.update(
            default_value.as_deref(),
            false,
            package.as_deref(),
            None,
            false,
            true,
            true,
            &mut self.next_id,
        ) {
            return Ok(false);
        }
        self.set_memory_usage(capped, package.as_deref(), size);
        self.schedule_write();
        Ok(true)
    }

    /// `SettingsState.deleteSettingLocked`.
    fn delete(&mut self, capped: bool, name: &str) -> bool {
        let Ok(i) = self.index(name) else {
            return false;
        };
        let old = self.settings.remove(i);
        let size = self.memory_usage(
            capped,
            old.package.as_deref(),
            -(length(name) as i32),
            old.value.as_deref(),
            None,
            old.default_value.as_deref(),
            None,
        );
        self.set_memory_usage(capped, old.package.as_deref(), size);
        self.schedule_write();
        true
    }
}

impl State {
    /// `mNamespaceBannedHashes.get(prefix)`.
    fn banned_hash(&self, prefix: &str) -> Option<&str> {
        self.namespace_hashes
            .iter()
            .find(|(n, _)| n.as_deref() == Some(prefix))
            .and_then(|(_, h)| h.as_deref())
    }

    /// `banConfigurationLocked`'s `mNamespaceBannedHashes.put`, in its
    /// `ArrayMap`'s order; written with the changes that follow.
    fn ban(&mut self, prefix: &str, hash: String) {
        if let Some(entry) = self
            .namespace_hashes
            .iter_mut()
            .find(|(n, _)| n.as_deref() == Some(prefix))
        {
            entry.1 = Some(hash);
            return;
        }
        let key = java_hash(prefix);
        let at = self
            .namespace_hashes
            .partition_point(|(n, _)| n.as_deref().map_or(0, java_hash) <= key);
        self.namespace_hashes
            .insert(at, (Some(prefix.to_string()), Some(hash)));
    }
}

/// `isExemptFromMemoryUsageCap`.
fn exempt(capped: bool, package: Option<&str>) -> bool {
    !capped || package == Some(SYSTEM_PACKAGE_NAME)
}

/// `createRealFlagName`: `staged/namespace*flag` as `namespace/flag`.
fn real_flag_name(staged: &str) -> String {
    let real = staged.split_once('/').and_then(|(_, rest)| {
        let (namespace, flag) = rest.split_once('*')?;
        (!namespace.is_empty() && !flag.is_empty()).then(|| format!("{namespace}/{flag}"))
    });
    real.unwrap_or_else(|| staged.to_string())
}

fn string(name: &str, value: &str) -> (String, Value) {
    (name.into(), Value::String(value.into()))
}

fn element(name: &str, attrs: Vec<(String, Value)>, content: Vec<Node>) -> Element {
    Element {
        name: name.into(),
        attrs,
        content,
    }
}

/// `base64Encode`: the string's UTF-16 units, big-endian.
fn base64_encode(s: &str) -> String {
    let bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
    aim_android_xml::base64::encode(&bytes)
}

/// `base64Decode`.
fn base64_decode(s: &str) -> String {
    let bytes = aim_android_xml::base64::decode(s).unwrap_or_default();
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

#[derive(Default)]
struct Shared {
    /// The guest's `/data`.
    data: PathBuf,
    tables: Mutex<HashMap<Table, State>>,
    /// Signalled when a write is scheduled.
    scheduled: Condvar,
}

#[derive(Default)]
pub struct Settings {
    shared: Arc<Shared>,
    writer: OnceLock<()>,
    /// The config table's aconfig defaults.
    aconfig: AconfigDefaults,
}

impl Settings {
    /// User 0's settings below the guest's `/data` at `data`, with the
    /// aconfig defaults of the image at `root`. A new secure or global
    /// table starts with the vendor defaults; existing XML is kept as-is,
    /// as the original has no provisioning backfill for SettingsState XML.
    pub fn load(data: &Path, root: &Path) -> Result<Settings, String> {
        let mut tables = HashMap::new();
        for table in Table::ALL {
            let path = data.join("system/users/0").join(table.file());
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if let Some(state) = State::defaults(table) {
                        std::fs::create_dir_all(data.join("system/users/0"))
                            .and_then(|()| write(data, table, &state.document(table)))
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        tables.insert(table, state);
                    }
                    continue;
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            tables.insert(
                table,
                State::parse(table, &bytes).map_err(|e| format!("{}: {e}", path.display()))?,
            );
        }
        Ok(Settings {
            shared: Arc::new(Shared {
                data: data.to_path_buf(),
                tables: Mutex::new(tables),
                scheduled: Condvar::new(),
            }),
            writer: OnceLock::new(),
            aconfig: AconfigDefaults::load(root),
        })
    }

    /// `mAconfigDefaultFlags`' flag `package.flag`.
    pub fn aconfig_flag(&self, name: &str) -> Option<&super::device_config::FlagInfo> {
        self.aconfig.flags.get(name)
    }

    /// `insertSettingLocked`'s staging of an aconfig flag of the config
    /// table: `staged/namespace*flag` for `namespace/flag` when the
    /// namespace's defaults name it.
    fn staged_name(&self, table: Table, name: &str) -> Option<String> {
        let slash = name
            .find('/')
            .filter(|&i| i != 0 && table == Table::Config)?;
        let namespace = &name[..slash];
        self.aconfig
            .namespaces
            .get(namespace)
            .is_some_and(|defaults| defaults.contains_key(name))
            .then(|| format!("{CONFIG_STAGED_PREFIX}{namespace}*{}", &name[slash + 1..]))
    }

    /// `SettingsProvider.getAllConfigFlags`: the aconfig defaults of the
    /// prefix's namespace (all of them without one), then the config
    /// settings it starts, but for read-only aconfig flags
    /// (`ignoreXmlForReadOnlyFlags`). A prefix must end its namespace.
    pub fn config_flags(
        &self,
        prefix: Option<&str>,
    ) -> Result<HashMap<String, Option<String>>, Exception> {
        let mut flags = HashMap::new();
        let defaults = |defaults: &HashMap<String, String>,
                        flags: &mut HashMap<String, Option<String>>| {
            flags.extend(
                defaults
                    .iter()
                    .map(|(name, value)| (name.clone(), Some(value.clone()))),
            );
        };
        match prefix {
            Some(prefix) => {
                // `prefix.substring(0, prefix.length() - 1)`.
                let namespace = prefix
                    .char_indices()
                    .last()
                    .map(|(i, _)| &prefix[..i])
                    .ok_or_else(|| {
                        Exception::new(
                            EX_ILLEGAL_STATE,
                            "StringIndexOutOfBoundsException: an empty prefix",
                        )
                    })?;
                if let Some(namespace) = self.aconfig.namespaces.get(namespace) {
                    defaults(namespace, &mut flags);
                }
            }
            None => self
                .aconfig
                .namespaces
                .values()
                .for_each(|namespace| defaults(namespace, &mut flags)),
        }
        let tables = self.shared.tables.lock().unwrap();
        for setting in tables
            .get(&Table::Config)
            .into_iter()
            .flat_map(|state| &state.settings)
        {
            if prefix.is_some_and(|p| !setting.name.starts_with(p)) {
                continue;
            }
            let flag = setting
                .name
                .find('/')
                .filter(|&i| i != 0)
                .map(|i| &setting.name[i + 1..]);
            if flag
                .and_then(|f| self.aconfig.flags.get(f))
                .is_some_and(|info| !info.read_write)
            {
                continue;
            }
            flags.insert(setting.name.clone(), setting.value.clone());
        }
        Ok(flags)
    }

    /// `getAllConfigPrefixesLocked`: each config setting's name up to its
    /// first `/`, with it.
    pub fn config_prefixes(&self) -> Vec<String> {
        let tables = self.shared.tables.lock().unwrap();
        let mut prefixes: Vec<String> = tables
            .get(&Table::Config)
            .into_iter()
            .flat_map(|state| &state.settings)
            .map(|s| format!("{}/", s.name.split('/').next().unwrap_or_default()))
            .collect();
        prefixes.sort();
        prefixes.dedup();
        prefixes
    }

    /// `banConfigurationLocked` of `prefix` with its flags `flags`: none
    /// for no flags.
    pub fn ban_config(&self, prefix: &str, flags: &HashMap<String, Option<String>>) {
        if flags.is_empty() {
            return;
        }
        let hash = map_hash(flags.iter().map(|(k, v)| (k.as_str(), v.as_deref())));
        self.shared
            .tables
            .lock()
            .unwrap()
            .entry(Table::Config)
            .or_insert_with(State::new)
            .ban(prefix, hash.to_string());
    }

    /// `setConfigSettingsLocked` of `prefix`, `key_values` in their map's
    /// order: false if the reset that banned the namespace's flags left
    /// them as they would be again (`isNewConfigBannedLocked`, `"null"`
    /// read as null, as it also does to the values set); else every
    /// namespace is unbanned if this one was, and the flags are set
    /// (`setSettingsLocked`). The names changed come with it; a value of
    /// a flag there is is always a change unless both are null, as the
    /// original compares them by reference. `caller_system` is a new
    /// setting's (`Setting.update`'s).
    pub fn set_config(
        &self,
        prefix: &str,
        key_values: &mut [(String, Option<String>)],
        package: Option<&str>,
        caller_system: bool,
    ) -> Option<Vec<String>> {
        for (_, value) in key_values.iter_mut() {
            if value.as_deref() == Some(NULL_VALUE_OLD_STYLE) {
                *value = None;
            }
        }
        let (changed, unbanned) = {
            let mut tables = self.shared.tables.lock().unwrap();
            let state = tables.entry(Table::Config).or_insert_with(State::new);
            let hash = map_hash(key_values.iter().map(|(k, v)| (k.as_str(), v.as_deref())));
            if state.banned_hash(prefix) == Some(hash.to_string().as_str()) {
                return None;
            }
            // `unbanAllConfigIfBannedConfigUpdatedLocked`.
            let unbanned = state.banned_hash(prefix).is_some();
            if unbanned {
                state.namespace_hashes.clear();
                state.schedule_write();
            }
            let namespace = prefix.rfind('/').map_or("", |i| &prefix[..i]);
            let trunk = self.aconfig.namespaces.get(namespace);
            let mut changed = Vec::new();
            // Old flags of the prefix not set again go, but aconfig ones.
            state.settings.retain(|s| {
                let gone = trunk.is_none_or(|t| !t.contains_key(&s.name))
                    && s.name.starts_with(prefix)
                    && !key_values.iter().any(|(k, _)| *k == s.name);
                if gone {
                    changed.push(s.name.clone());
                }
                !gone
            });
            for (key, value) in key_values.iter() {
                let staged = key
                    .find('/')
                    .filter(|&i| i > 0 && trunk.is_some_and(|t| t.contains_key(key)))
                    .map(|i| format!("{CONFIG_STAGED_PREFIX}{namespace}*{}", &key[i + 1..]));
                let name = staged.unwrap_or_else(|| key.clone());
                match state.index(&name) {
                    Err(_) => {
                        let mut setting = Setting {
                            name: name.clone(),
                            ..Default::default()
                        };
                        setting.update(
                            value.as_deref(),
                            false,
                            package,
                            None,
                            caller_system,
                            true,
                            false,
                            &mut state.next_id,
                        );
                        state.put(setting);
                    }
                    Ok(i) => {
                        if state.settings[i].value.is_none() && value.is_none() {
                            continue;
                        }
                        state.settings[i].update(
                            value.as_deref(),
                            false,
                            package,
                            None,
                            false,
                            false,
                            false,
                            &mut state.next_id,
                        );
                    }
                }
                changed.push(name);
            }
            if !changed.is_empty() {
                state.schedule_write();
            }
            (changed, unbanned)
        };
        if unbanned || !changed.is_empty() {
            self.writes_scheduled();
        }
        Some(changed)
    }

    #[cfg(test)]
    pub(super) fn read_for_test(&mut self, table: Table, bytes: &[u8]) {
        self.shared
            .tables
            .lock()
            .unwrap()
            .insert(table, State::parse(table, bytes).unwrap());
    }

    /// `getSettingLocked`: none for a setting that is absent.
    pub fn setting(&self, table: Table, name: &str) -> Option<Setting> {
        self.shared
            .tables
            .lock()
            .unwrap()
            .get(&table)?
            .get(name)
            .cloned()
    }

    /// `Settings.*.getStringForUser`: `None` for a setting that is absent
    /// or null.
    pub fn get(&self, table: Table, name: &str) -> Option<String> {
        self.setting(table, name)?.value
    }

    /// The settings of `table` whose names start with `prefix`
    /// (`SettingsState.getSettingNamesLocked` filtered as `LIST_config`
    /// filters them).
    pub fn list(&self, table: Table, prefix: &str) -> Vec<(String, Option<String>)> {
        let tables = self.shared.tables.lock().unwrap();
        let mut list: Vec<_> = tables
            .get(&table)
            .into_iter()
            .flat_map(|state| &state.settings)
            .filter(|s| s.name.starts_with(prefix))
            .map(|s| (s.name.clone(), s.value.clone()))
            .collect();
        list.sort();
        list
    }

    /// The settings of `table` in `getSettingNamesLocked`'s order (its
    /// `ArrayMap`'s).
    pub fn all(&self, table: Table) -> Vec<Setting> {
        let tables = self.shared.tables.lock().unwrap();
        let settings = tables
            .get(&table)
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        crate::package::info::array_order(settings, |s| &s.name)
    }

    /// `DeviceConfig.getProperty(namespace, name)`.
    pub fn device_config(&self, namespace: &str, name: &str) -> Option<String> {
        self.get(Table::Config, &format!("{namespace}/{name}"))
    }

    /// `SettingsState.insertSettingLocked`, and `persistSettingsLocked`
    /// after a change of a `critical` setting: whether it changed.
    pub fn insert(
        &self,
        table: Table,
        name: &str,
        change: &Change,
        critical: bool,
    ) -> Result<bool, Exception> {
        if name.is_empty() {
            return Ok(false);
        }
        let staged = self.staged_name(table, name);
        let name = staged.as_deref().unwrap_or(name);
        let name_too_long = length(name) > MAX_LENGTH_PER_STRING;
        if name_too_long
            || change
                .value
                .is_some_and(|v| length(v) > MAX_LENGTH_PER_STRING)
        {
            let shown: String = name.chars().take(20).collect();
            return Err(Exception::illegal_argument(format!(
                "The {} of your setting [{}{}] is too long. The max length allowed for the string is {MAX_LENGTH_PER_STRING}.",
                if name_too_long { "name" } else { "value" },
                shown,
                if length(name) > 20 { "..." } else { "" },
            )));
        }
        let capped = table.capped();
        let changed = {
            let mut tables = self.shared.tables.lock().unwrap();
            let state = tables.entry(table).or_insert_with(State::new);
            let old = state.get(name).cloned();
            let previous_owner = old.as_ref().and_then(|s| s.package.clone());
            let owner_changed =
                previous_owner.is_some() && previous_owner.as_deref() != change.package;
            let old_value = old.as_ref().and_then(|s| s.value.clone());
            let old_default = old.as_ref().and_then(|s| s.default_value.clone());
            let new_default = if change.make_default {
                change.value.map(str::to_string)
            } else {
                old_default.clone()
            };
            let size = state.memory_usage(
                capped,
                change.package,
                if old.is_none() || owner_changed {
                    length(name) as i32
                } else {
                    0
                },
                if owner_changed {
                    None
                } else {
                    old_value.as_deref()
                },
                change.value,
                if owner_changed {
                    None
                } else {
                    old_default.as_deref()
                },
                new_default.as_deref(),
            );
            State::check_memory_usage(capped, change.package, size)?;
            let mut setting = old.unwrap_or_else(|| Setting {
                name: name.to_string(),
                ..Default::default()
            });
            let updated = setting.update(
                change.value,
                change.make_default,
                change.package,
                change.tag,
                change.caller_system,
                change.overrideable_by_restore,
                false,
                &mut state.next_id,
            );
            // A new setting is added even when its update changed nothing
            // (`new Setting(...)`).
            let added = state.index(name).is_err();
            if updated || added {
                state.put(setting);
                state.set_memory_usage(capped, change.package, size);
                if owner_changed {
                    let previous = state.memory_usage(
                        capped,
                        previous_owner.as_deref(),
                        -(length(name) as i32),
                        old_value.as_deref(),
                        None,
                        old_default.as_deref(),
                        None,
                    );
                    state.set_memory_usage(capped, previous_owner.as_deref(), previous);
                }
                state.schedule_write();
                if critical {
                    state.persist();
                }
            }
            updated || added
        };
        if changed {
            self.writes_scheduled();
        }
        Ok(changed)
    }

    /// `SettingsState.deleteSettingLocked`, and `persistSettingsLocked`
    /// after deleting a `critical` setting: whether there was one.
    pub fn delete(&self, table: Table, name: &str, critical: bool) -> bool {
        let deleted = {
            let mut tables = self.shared.tables.lock().unwrap();
            let state = tables.entry(table).or_insert_with(State::new);
            let deleted = state.delete(table.capped(), name);
            if deleted && critical {
                state.persist();
            }
            deleted
        };
        if deleted {
            self.writes_scheduled();
        }
        deleted
    }

    /// `SettingsRegistry.resetSettingsLocked` of the settings of `table`
    /// in `mode`, for `package`, of those `prefix` starts if given: the
    /// names changed, in order. `system_package` is
    /// `SettingsState.isSystemPackage`.
    pub fn reset(
        &self,
        table: Table,
        mode: ResetMode,
        package: Option<&str>,
        tag: Option<&str>,
        prefix: Option<&str>,
        system_package: &dyn Fn(Option<&str>) -> Result<bool, Exception>,
    ) -> Result<Vec<String>, Exception> {
        let capped = table.capped();
        let mut changed = Vec::new();
        {
            let mut tables = self.shared.tables.lock().unwrap();
            let state = tables.entry(table).or_insert_with(State::new);
            let settings = state.settings.clone();
            for setting in settings {
                // `shouldExcludeSettingFromReset`.
                let excluded = prefix.is_some_and(|p| !setting.name.starts_with(p))
                    || setting.name == SECURE_FRP_MODE;
                let name = setting.name.as_str();
                let reset = match mode {
                    ResetMode::PackageDefaults => {
                        if setting.package.as_deref() != package
                            || tag.is_some() && tag != setting.tag.as_deref()
                            || excluded
                        {
                            continue;
                        }
                        state.reset(capped, name)?
                    }
                    ResetMode::UntrustedDefaults => {
                        if system_package(setting.package.as_deref())? || excluded {
                            continue;
                        }
                        state.reset(capped, name)?
                    }
                    ResetMode::UntrustedChanges | ResetMode::TrustedDefaults => {
                        if mode == ResetMode::UntrustedChanges
                            && system_package(setting.package.as_deref())?
                            || excluded
                        {
                            continue;
                        }
                        if setting.default_from_system {
                            state.reset(capped, name)?
                        } else {
                            state.delete(capped, name)
                        }
                    }
                };
                if reset {
                    changed.push(setting.name.clone());
                }
            }
            if !changed.is_empty() {
                state.persist();
            }
        }
        if !changed.is_empty() {
            self.writes_scheduled();
        }
        Ok(changed)
    }

    /// Wakes the writer, started with the first change.
    fn writes_scheduled(&self) {
        self.writer.get_or_init(|| {
            let shared = self.shared.clone();
            if let Err(e) = std::thread::Builder::new()
                .name("settings writer".into())
                .spawn(move || shared.write_when_due())
            {
                eprintln!("app host: settings writer: {e}");
            }
        });
        self.shared.scheduled.notify_one();
    }
}

impl Shared {
    /// `MSG_PERSIST_SETTINGS`: each table's state written when its write
    /// is due (`doWriteState`), one at a time.
    fn write_when_due(&self) {
        let mut tables = self.tables.lock().unwrap();
        loop {
            let now = Instant::now();
            let mut due = Vec::new();
            for (table, state) in tables.iter_mut() {
                if state.write_at.is_some_and(|at| at <= now) {
                    state.dirty = false;
                    state.write_at = None;
                    due.push((*table, state.document(*table)));
                }
            }
            if due.is_empty() {
                tables = match tables.values().filter_map(|s| s.write_at).min() {
                    Some(at) => self.scheduled.wait_timeout(tables, at - now).unwrap().0,
                    None => self.scheduled.wait(tables).unwrap(),
                };
                continue;
            }
            drop(tables);
            for (table, document) in due {
                if let Err(e) = write(&self.data, table, &document) {
                    eprintln!("app host: failed to write settings {}: {e}", table.file());
                }
            }
            tables = self.tables.lock().unwrap();
        }
    }
}

/// `doWriteState`'s `AtomicFile` write of `document`, as system_server's
/// file.
fn write(data: &Path, table: Table, document: &[Element]) -> std::io::Result<()> {
    let bytes = aim_android_xml::abx::write_sequence(document).map_err(std::io::Error::other)?;
    let dir = data.join("system/users/0");
    let path = dir.join(table.file());
    let new = dir.join(format!("{}.new", table.file()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        },
    )?;
    std::fs::rename(&new, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change<'a>(value: Option<&'a str>, package: &'a str) -> Change<'a> {
        Change {
            value,
            tag: None,
            make_default: false,
            package: Some(package),
            caller_system: false,
            overrideable_by_restore: false,
        }
    }

    /// `DatabaseHelper`'s vendor provisioning defaults, migrated with
    /// `SettingsRegistry.migrateLegacySettingsLocked`'s system attribution.
    #[test]
    fn fresh_settings_are_provisioned_and_persisted() {
        let dir = std::env::temp_dir().join(format!("aim-settings-fresh-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let defaults = [
            (Table::Global, "device_provisioned"),
            (Table::Secure, "user_setup_complete"),
        ];
        let settings = Settings::load(&dir, &dir).unwrap();
        for (table, name) in defaults {
            let setting = settings.setting(table, name).unwrap();
            assert_eq!(settings.get(table, name).as_deref(), Some("1"));
            assert_eq!(setting.default_value.as_deref(), Some("1"));
            assert_eq!(setting.package.as_deref(), Some(SYSTEM_PACKAGE_NAME));
            assert!(setting.default_from_system);
            assert!(!setting.preserved_in_restore);
            let bytes = std::fs::read(dir.join("system/users/0").join(table.file())).unwrap();
            assert!(bytes.starts_with(aim_android_xml::abx::MAGIC));
            let state = State::parse(table, &bytes).unwrap();
            assert_eq!(state.version, 229);
            assert_eq!(state.get(name), Some(&setting));
        }
        assert_eq!(settings.get(Table::Secure, "tv_user_setup_complete"), None);
        let again = Settings::load(&dir, &dir).unwrap();
        for (table, name) in defaults {
            assert_eq!(again.get(table, name).as_deref(), Some("1"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The legacy SQLite 92->93 upgrade derives USER_SETUP_COMPLETE from
    /// DEVICE_PROVISIONED; it does not backfill existing SettingsState XML.
    #[test]
    fn existing_settings_do_not_reapply_provisioning_defaults() {
        let dir =
            std::env::temp_dir().join(format!("aim-settings-existing-{}", std::process::id()));
        let users = dir.join("system/users/0");
        std::fs::create_dir_all(&users).unwrap();
        for value in [None, Some("0"), Some("1")] {
            let mut documents = Vec::new();
            for (table, name) in [
                (Table::Global, "device_provisioned"),
                (Table::Secure, "user_setup_complete"),
            ] {
                let setting = value.map_or(String::new(), |value| {
                    format!(r#"<setting id="7" name="{name}" value="{value}" package="android" />"#)
                });
                let xml = format!(r#"<settings version="229">{setting}</settings>"#);
                std::fs::write(users.join(table.file()), &xml).unwrap();
                documents.push((table, name, xml));
            }
            let settings = Settings::load(&dir, &dir).unwrap();
            for (table, name, xml) in documents {
                assert_eq!(settings.get(table, name).as_deref(), value);
                assert_eq!(
                    std::fs::read_to_string(users.join(table.file())).unwrap(),
                    xml
                );
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_values_and_nulls() {
        let mut s = Settings::default();
        s.read_for_test(
            Table::Secure,
            br#"<settings version="226">
            <setting id="1" name="long_press_timeout" value="400" package="android" />
            <setting id="2" name="nothing" package="android" />
            <setting id="3" name="binary" valueBase64="AAkAYQ==" package="android" />
        </settings>"#,
        );
        s.read_for_test(Table::Config, br#"<settings version="1"><setting id="3" name="widget/line_slop_ratio" value="0.25" /></settings>"#);
        assert_eq!(
            s.get(Table::Secure, "long_press_timeout").as_deref(),
            Some("400")
        );
        assert_eq!(s.get(Table::Secure, "nothing"), None);
        assert_eq!(s.get(Table::Secure, "binary").as_deref(), Some("\ta"));
        assert_eq!(
            s.device_config("widget", "line_slop_ratio").as_deref(),
            Some("0.25")
        );
    }

    /// Settings with the aconfig flag `ns/com.example.trunk` (read-write,
    /// on) and `ns/com.example.fixed` (read-only), and the config flags
    /// `ns/a`, `ns/b` and `other/c`.
    fn config_settings() -> Settings {
        let mut s = Settings::default();
        s.aconfig.namespaces.insert(
            "ns".into(),
            HashMap::from([
                ("ns/com.example.trunk".into(), "true".into()),
                ("ns/com.example.fixed".into(), "false".into()),
            ]),
        );
        for (flag, read_write) in [("com.example.trunk", true), ("com.example.fixed", false)] {
            s.aconfig.flags.insert(
                flag.into(),
                super::super::device_config::FlagInfo {
                    namespace: "ns".into(),
                    read_write,
                },
            );
        }
        s.read_for_test(
            Table::Config,
            br#"<settings version="1">
            <setting id="1" name="ns/a" value="1" package="gms" />
            <setting id="2" name="ns/b" value="2" package="gms" />
            <setting id="3" name="other/c" value="3" package="gms" />
            <setting id="4" name="ns/com.example.fixed" value="true" package="gms" />
        </settings>"#,
        );
        s
    }

    fn names(s: &Settings) -> Vec<String> {
        s.list(Table::Config, "")
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    }

    /// `setSettingsLocked` (DeviceConfigApiTests `setProperties`): the
    /// namespace's flags not set again go, the others are set, an aconfig
    /// flag staged; a flag set to the value it has is a change all the
    /// same, but none to null from null.
    #[test]
    fn a_namespace_is_set_as_a_whole() {
        let s = config_settings();
        let mut flags = vec![
            ("ns/a".to_string(), Some("1".to_string())),
            ("ns/new".to_string(), Some("null".to_string())),
            (
                "ns/com.example.trunk".to_string(),
                Some("false".to_string()),
            ),
        ];
        let changed = s.set_config("ns/", &mut flags, Some("gms"), false).unwrap();
        assert_eq!(
            changed,
            ["ns/b", "ns/a", "ns/new", "staged/ns*com.example.trunk"]
        );
        assert_eq!(flags[1].1, None);
        assert_eq!(
            names(&s),
            [
                "ns/a",
                "ns/com.example.fixed",
                "ns/new",
                "other/c",
                "staged/ns*com.example.trunk"
            ]
        );
        assert_eq!(
            s.get(Table::Config, "staged/ns*com.example.trunk")
                .as_deref(),
            Some("false")
        );
        let changed = s.set_config("ns/", &mut flags, Some("gms"), false).unwrap();
        assert_eq!(changed, ["ns/a", "staged/ns*com.example.trunk"]);
    }

    /// `banConfigurationLocked` and `isNewConfigBannedLocked`
    /// (DeviceConfigApiTests `resetToDefault` then `setProperties`): the
    /// flags a reset banned cannot be set again as they were; others can,
    /// and unban every namespace.
    #[test]
    fn a_reset_bans_its_flags() {
        let s = config_settings();
        let flags = s.config_flags(Some("ns/")).unwrap();
        s.ban_config("ns/", &flags);
        s.ban_config("other/", &s.config_flags(Some("other/")).unwrap());
        let mut same: Vec<_> = flags.into_iter().collect();
        assert_eq!(s.set_config("ns/", &mut same, Some("gms"), false), None);
        let mut other = vec![("ns/a".to_string(), Some("9".to_string()))];
        assert!(
            s.set_config("ns/", &mut other, Some("gms"), false)
                .is_some()
        );
        let tables = s.shared.tables.lock().unwrap();
        assert!(tables[&Table::Config].namespace_hashes.is_empty());
    }

    /// `getAllConfigFlags`: the namespace's aconfig defaults, then its
    /// settings over them, but for a read-only aconfig flag's.
    #[test]
    fn config_flags_are_defaults_then_settings() {
        let s = config_settings();
        let flags = s.config_flags(Some("ns/")).unwrap();
        let mut flags: Vec<_> = flags.into_iter().collect();
        flags.sort();
        let some = |v: &str| Some(v.to_string());
        assert_eq!(
            flags,
            [
                ("ns/a".to_string(), some("1")),
                ("ns/b".to_string(), some("2")),
                ("ns/com.example.fixed".to_string(), some("false")),
                ("ns/com.example.trunk".to_string(), some("true")),
            ]
        );
        assert_eq!(s.config_flags(None).unwrap().len(), 5);
        assert!(s.config_flags(Some("")).is_err());
        assert_eq!(s.config_prefixes(), ["ns/", "other/"]);
    }

    /// `insertSettingLocked` stages a write of an aconfig flag for the
    /// next boot; any other flag is written as named.
    #[test]
    fn aconfig_flags_are_staged() {
        let s = config_settings();
        assert!(
            s.insert(
                Table::Config,
                "ns/com.example.trunk",
                &change(Some("false"), "gms"),
                false
            )
            .unwrap()
        );
        assert!(
            s.insert(Table::Config, "ns/plain", &change(Some("x"), "gms"), false)
                .unwrap()
        );
        assert_eq!(
            s.get(Table::Config, "staged/ns*com.example.trunk")
                .as_deref(),
            Some("false")
        );
        assert_eq!(s.get(Table::Config, "ns/com.example.trunk"), None);
        assert_eq!(s.get(Table::Config, "ns/plain").as_deref(), Some("x"));
    }

    /// A banned namespace's hash is written with the settings, in its
    /// `ArrayMap`'s order, and read back.
    #[test]
    fn banned_hashes_are_kept() {
        let mut state = State::new();
        state.version = 1;
        state.ban("ns/", "5".into());
        state.ban("a/", "6".into());
        state.ban("ns/", "7".into());
        let document = state.document(Table::Config);
        let hashes: Vec<_> = document[1]
            .children()
            .map(|e| {
                (
                    e.string("namespace").unwrap().into_owned(),
                    e.string("bannedHash").unwrap().into_owned(),
                )
            })
            .collect();
        assert_eq!(
            hashes,
            [
                ("a/".to_string(), "6".to_string()),
                ("ns/".to_string(), "7".to_string())
            ]
        );
        let read = State::parse(
            Table::Config,
            &aim_android_xml::abx::write_sequence(&document).unwrap(),
        )
        .unwrap();
        assert_eq!(read.banned_hash("ns/"), Some("7"));
    }

    /// `SettingsStateTest.testUpgrade`-style values: before version 121,
    /// the string `null` is no value.
    #[test]
    fn old_encoding_null() {
        let mut s = Settings::default();
        s.read_for_test(
            Table::System,
            br#"<settings version="120"><setting id="1" name="a" value="null" package="android" /></settings>"#,
        );
        assert_eq!(s.setting(Table::System, "a").unwrap().value, None);
    }

    /// `SettingsStateTest.testSetDefault*`: the system's value is the
    /// default, an app's default is replaced only while the system set
    /// none.
    #[test]
    fn defaults_follow_the_system() {
        let s = Settings::default();
        let system = Change {
            caller_system: true,
            ..change(Some("1"), "android")
        };
        assert!(s.insert(Table::Secure, "x", &system, false).unwrap());
        let x = s.setting(Table::Secure, "x").unwrap();
        assert_eq!(
            (x.default_value.as_deref(), x.default_from_system),
            (Some("1"), true)
        );
        let app = Change {
            make_default: true,
            ..change(Some("2"), "app")
        };
        assert!(s.insert(Table::Secure, "x", &app, false).unwrap());
        let x = s.setting(Table::Secure, "x").unwrap();
        assert_eq!(
            (x.value.as_deref(), x.default_value.as_deref()),
            (Some("2"), Some("1"))
        );
        // The same again changes nothing.
        assert!(!s.insert(Table::Secure, "x", &app, false).unwrap());
    }

    #[test]
    fn apps_system_settings_are_capped() {
        let s = Settings::default();
        let big = "v".repeat(10_000);
        assert!(
            s.insert(Table::System, "a", &change(Some(&big), "app"), false)
                .unwrap()
        );
        let e = s
            .insert(Table::System, "b", &change(Some(&big), "app"), false)
            .unwrap_err();
        assert_eq!(e.code, EX_ILLEGAL_STATE);
        // The system and other tables are not capped.
        assert!(
            s.insert(Table::System, "b", &change(Some(&big), "android"), false)
                .unwrap()
        );
        assert!(
            s.insert(Table::Secure, "b", &change(Some(&big), "app"), false)
                .unwrap()
        );
        // A deletion frees the app's bytes.
        assert!(s.delete(Table::System, "a", false));
        assert!(
            s.insert(Table::System, "c", &change(Some(&big), "app"), false)
                .unwrap()
        );
    }

    /// `SettingsProviderTest.testResetModePackageDefaults*`.
    #[test]
    fn reset_to_package_defaults() {
        let s = Settings::default();
        let set = |value, make_default| Change {
            make_default,
            ..change(Some(value), "app")
        };
        s.insert(Table::Secure, "a", &set("1", true), false)
            .unwrap();
        s.insert(Table::Secure, "a", &set("2", false), false)
            .unwrap();
        s.insert(Table::Secure, "b", &change(Some("3"), "other"), false)
            .unwrap();
        let none = |_: Option<&str>| Ok(false);
        let changed = s
            .reset(
                Table::Secure,
                ResetMode::PackageDefaults,
                Some("app"),
                None,
                None,
                &none,
            )
            .unwrap();
        assert_eq!(changed, ["a"]);
        assert_eq!(s.get(Table::Secure, "a").as_deref(), Some("1"));
        assert_eq!(s.get(Table::Secure, "b").as_deref(), Some("3"));
        // Untrusted changes without a system default are deleted.
        let changed = s
            .reset(
                Table::Secure,
                ResetMode::UntrustedChanges,
                None,
                None,
                None,
                &none,
            )
            .unwrap();
        assert_eq!(changed.len(), 2);
        assert_eq!(s.setting(Table::Secure, "b"), None);
    }

    /// A change is written as `doWriteState` writes it: binary XML, the
    /// version, each setting's attributes in its order, then the hashes.
    #[test]
    fn changes_are_written_as_settings_state_writes_them() {
        let dir = std::env::temp_dir().join(format!("aim-settings-{}", std::process::id()));
        let users = dir.join("system/users/0");
        std::fs::create_dir_all(&users).unwrap();
        std::fs::write(
            users.join("settings_secure.xml"),
            br#"<settings version="226"><setting id="7" name="a" value="1" package="android" defaultValue="1" defaultSysSet="true" /></settings>"#,
        )
        .unwrap();
        let s = Settings::load(&dir, &dir).unwrap();
        let tagged = Change {
            tag: Some("t"),
            make_default: true,
            ..change(Some("x\u{1}"), "app")
        };
        assert!(s.insert(Table::Secure, "b", &tagged, false).unwrap());
        assert!(
            s.insert(
                Table::Secure,
                "clockwork_home_ready",
                &change(Some("1"), "app"),
                false
            )
            .unwrap()
        );
        let started = Instant::now();
        let bytes = loop {
            let bytes = std::fs::read(users.join("settings_secure.xml")).unwrap();
            if bytes.starts_with(aim_android_xml::abx::MAGIC)
                || started.elapsed() > Duration::from_secs(5)
            {
                break bytes;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(started.elapsed() >= WRITE_SETTINGS_DELAY - Duration::from_millis(20));
        let roots = aim_android_xml::abx::read_sequence(&bytes).unwrap();
        assert_eq!(
            roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["settings", "namespaceHashes"]
        );
        assert_eq!(roots[0].attr("version"), Some(&Value::Int(226)));
        let settings: Vec<_> = roots[0].children().collect();
        // The transient setting is a secure one here, so it is kept.
        assert_eq!(settings.len(), 3);
        let b = settings
            .iter()
            .find(|e| e.string("name").as_deref() == Some("b"))
            .unwrap();
        assert_eq!(
            b.attrs,
            [
                string("id", "8"),
                string("name", "b"),
                string("valueBase64", &base64_encode("x\u{1}")),
                string("package", "app"),
                string("defaultValueBase64", &base64_encode("x\u{1}")),
                ("defaultSysSet".into(), Value::Bool(false)),
                string("tag", "t"),
                ("preserve_in_restore".into(), Value::Bool(true)),
            ]
        );
        assert!(!users.join("settings_secure.xml.new").exists());
        let again = Settings::load(&dir, &dir).unwrap();
        assert_eq!(again.get(Table::Secure, "b").as_deref(), Some("x\u{1}"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn transient_global_settings_are_not_written() {
        let mut state = State::new();
        state.version = 226;
        state.put(Setting {
            name: "clockwork_home_ready".into(),
            package: Some("android".into()),
            ..Default::default()
        });
        state.put(Setting {
            name: "a".into(),
            package: None,
            ..Default::default()
        });
        assert_eq!(state.document(Table::Global)[0].children().count(), 0);
        assert_eq!(state.document(Table::Secure)[0].children().count(), 1);
    }

    #[test]
    fn settings_keep_array_map_order() {
        let mut state = State::new();
        for name in ["b", "a", "Aa", "BB", "c"] {
            state.put(Setting {
                name: name.into(),
                ..Default::default()
            });
        }
        // "Aa" and "BB" share a hash: kept as added.
        let names: Vec<_> = state.settings.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "Aa", "BB"]);
        assert_eq!(state.index("BB"), Ok(4));
        assert_eq!(real_flag_name("staged/ns*flag"), "ns/flag");
        assert_eq!(real_flag_name("staged/nsflag"), "staged/nsflag");
    }
}
