//! Boot-time property initialization (property_service.cpp `PropertyInit`,
//! `CreateSerializedPropertyInfo`, `PropertyLoadBootDefaults`,
//! `LoadProperties`, the derived defaults) against an image.

use std::collections::BTreeMap;

use super::area::{AreaMemory, PROP_VALUE_MAX};
use super::info::{
    DEFAULT_CONTEXT, DEFAULT_TYPE, PROPERTY_CONTEXTS_FILES, PROPERTY_CONTEXTS_FILES_RAMDISK,
    PropertyInfoEntry, build_trie, parse_property_info_file,
};
use super::service::{
    INIT_CONTEXT, PropertyService, RESTORECON_PROPERTY, Ucred, VENDOR_INIT_CONTEXT,
};
use crate::PropertyLookup;
use crate::diag::Diagnostic;
use crate::image::ImageRoot;
use crate::libbase::{is_c_space, parse_bool, parse_int};
use crate::rc::{ANDROID_API_FUTURE, ANDROID_API_P, ANDROID_API_R, expand_props};

/// `kSecondStageRes` + `kBootImageRamdiskProp`.
pub const SECOND_STAGE_RAMDISK_PROP: &str = "/second_stage_resources/system/etc/ramdisk/build.prop";
/// `kDebugRamdiskProp`.
pub const DEBUG_RAMDISK_PROP: &str = "/debug_ramdisk/adb_debug.prop";
/// `__ANDROID_VENDOR_API_MAX__`.
const ANDROID_VENDOR_API_MAX: i64 = 1_000_000;
/// `__ANDROID_API_V__`.
const ANDROID_API_V: i64 = 35;

/// `CreateSerializedPropertyInfo`: reads the image's `*_property_contexts`
/// in init's order and serializes them. Returns the `property_info` bytes.
pub fn create_serialized_property_info(
    image: &ImageRoot,
    vendor_api_level: u32,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<u8>, String> {
    let require_prefix_or_exact = vendor_api_level >= ANDROID_API_R;
    let files = if image.exists(PROPERTY_CONTEXTS_FILES[0]) {
        PROPERTY_CONTEXTS_FILES
    } else {
        PROPERTY_CONTEXTS_FILES_RAMDISK
    };
    let mut entries: Vec<PropertyInfoEntry> = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let contents = match image.read(file) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => {
                if index == 0 {
                    return Err(format!("Could not read properties from '{file}': {error}"));
                }
                continue;
            }
        };
        for error in parse_property_info_file(&contents, require_prefix_or_exact, &mut entries) {
            diagnostics.push(Diagnostic::error(
                *file,
                0,
                format!("Could not read line from '{file}': {error}"),
            ));
        }
    }
    build_trie(&entries, DEFAULT_CONTEXT, DEFAULT_TYPE)
        .map_err(|error| format!("Unable to serialize property contexts: {error}"))
}

/// Inputs init takes from the kernel and device before loading `.prop`
/// files. The daemon supplies them; nothing here is invented.
#[derive(Clone, Debug, Default)]
pub struct KernelBootProperties {
    /// `androidboot.*` from the device tree (`ro.boot.*` names, DT values
    /// with ',' replaced by '.'), applied first.
    pub device_tree: Vec<(String, String)>,
    /// `androidboot.<key>=<value>` from the kernel command line.
    pub cmdline: Vec<(String, String)>,
    /// `androidboot.<key>=<value>` from bootconfig.
    pub bootconfig: Vec<(String, String)>,
}

/// Options `PropertyInit` derives from the boot environment.
#[derive(Clone, Debug)]
pub struct PropertyInitOptions {
    pub kernel: KernelBootProperties,
    /// `IsRecoveryMode()`: also loads `/prop.default` first.
    pub recovery: bool,
    /// `getpagesize()` for `ro.boot.hardware.cpu.pagesize`.
    pub page_size: u32,
    /// `SelinuxGetVendorAndroidVersion()`.
    pub vendor_api_level: u32,
}

impl Default for PropertyInitOptions {
    fn default() -> Self {
        Self {
            kernel: KernelBootProperties::default(),
            recovery: false,
            page_size: 16384,
            vendor_api_level: ANDROID_API_FUTURE,
        }
    }
}

/// Collects diagnostics and applies `InitPropertySet` / `PropertySetNoSocket`.
struct Loader<'a, M: AreaMemory> {
    service: &'a mut PropertyService<M>,
    image: &'a ImageRoot,
    vendor_api_level: u32,
    diagnostics: Vec<Diagnostic>,
}

impl<M: AreaMemory> Loader<'_, M> {
    fn init_set(&mut self, name: &str, value: &str) {
        let outcome = self.service.init_set(name, value);
        if !outcome.is_success() {
            self.diagnostics.push(Diagnostic::error(
                "<init>",
                0,
                format!(
                    "Init cannot set '{name}' to '{value}': {}",
                    outcome.error.unwrap_or_default()
                ),
            ));
        }
    }

    fn set_no_socket(&mut self, name: &str, value: &str, what: &str) {
        let outcome = self.service.set_no_socket(name, value);
        if !outcome.is_success() {
            self.diagnostics.push(Diagnostic::error(
                "<init>",
                0,
                format!(
                    "Could not set '{name}' to '{value}' {what}: {}",
                    outcome.error.unwrap_or_default()
                ),
            ));
        }
    }

    fn get(&self, name: &str, default: &str) -> String {
        self.service.property_or(name, default)
    }

    /// `load_properties_from_file`.
    fn load_file(
        &mut self,
        filename: &str,
        filter: Option<&str>,
        properties: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        let mut contents = self
            .image
            .read(filename)
            .map_err(|error| format!("Couldn't load property file '{filename}': {error}"))?;
        contents.push(b'\n');
        self.load_properties(&contents, filter, filename, properties);
        Ok(())
    }

    /// `LoadProperties`: `key=value` lines, `#` comments, and (only in
    /// unfiltered files) `import <file> [<filter>]`, where a filter
    /// `prefix.*` loads names with that prefix and anything else one name.
    fn load_properties(
        &mut self,
        data: &[u8],
        filter: Option<&str>,
        filename: &str,
        properties: &mut BTreeMap<String, String>,
    ) {
        let context = if self.vendor_api_level >= ANDROID_API_P
            && ["/vendor", "/odm", "/vendor_dlkm", "/odm_dlkm"]
                .iter()
                .any(|prefix| filename.starts_with(prefix))
        {
            VENDOR_INIT_CONTEXT
        } else {
            INIT_CONTEXT
        };
        let flen = filter.map_or(0, str::len);

        // Every '\n'-terminated line (the file had '\n' appended).
        let mut lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
        lines.pop();
        for raw in lines {
            // A NUL inside a line ends it for strchr/strncmp purposes.
            let raw = &raw[..raw.iter().position(|b| *b == 0).unwrap_or(raw.len())];
            let start = raw.iter().take_while(|b| is_c_space(**b)).count();
            let mut line = &raw[start..];
            if line.first() == Some(&b'#') {
                continue;
            }
            // Trailing whitespace is trimmed, but never the first byte.
            while line.len() > 1 && is_c_space(line[line.len() - 1]) {
                line = &line[..line.len() - 1];
            }
            let line = String::from_utf8_lossy(line).into_owned();

            if line.starts_with("import ") && flen == 0 {
                let rest =
                    line[7..].trim_start_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
                let (file, filter) = match rest.find(' ') {
                    Some(space) => {
                        let filter = rest[space + 1..]
                            .trim_start_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
                        (&rest[..space], Some(filter))
                    }
                    None => (rest, None),
                };
                let expanded = match expand_props(file, &*self.service, self.vendor_api_level) {
                    Ok(expanded) => expanded,
                    Err(error) => {
                        self.diagnostics.push(Diagnostic::error(
                            filename,
                            0,
                            format!("Could not expand filename ': {error}"),
                        ));
                        continue;
                    }
                };
                let filter = filter.filter(|f| !f.is_empty());
                if let Err(error) = self.load_file(&expanded, filter, properties) {
                    self.diagnostics
                        .push(Diagnostic::warning(filename, 0, error));
                }
                continue;
            }

            let Some(equal) = line.find('=') else {
                continue;
            };
            let mut key = &line[..equal];
            while key.len() > 1 && is_c_space(key.as_bytes()[key.len() - 1]) {
                key = &key[..key.len() - 1];
            }
            let value =
                line[equal + 1..].trim_start_matches(|c: char| c.is_ascii() && is_c_space(c as u8));

            if let Some(filter) = filter {
                if let Some(prefix) = filter.strip_suffix('*') {
                    if !key.starts_with(prefix) {
                        continue;
                    }
                } else if key != filter {
                    continue;
                }
            }

            if key.starts_with("ctl.") || key == "sys.powerctl" || key == RESTORECON_PROPERTY {
                self.diagnostics.push(Diagnostic::error(
                    filename,
                    0,
                    format!("Ignoring disallowed property '{key}' with special meaning in prop file '{filename}'"),
                ));
                continue;
            }

            match self
                .service
                .check_permissions(key, value, context, &Ucred::INIT)
            {
                Ok(()) => match properties.get_mut(key) {
                    None => {
                        properties.insert(key.to_string(), value.to_string());
                    }
                    Some(existing) if existing != value => {
                        self.diagnostics.push(Diagnostic::warning(
                            filename,
                            0,
                            format!(
                                "Overriding previous property '{key}':'{existing}' with new value '{value}'"
                            ),
                        ));
                        *existing = value.to_string();
                    }
                    Some(_) => {}
                },
                Err((_, error)) => self.diagnostics.push(Diagnostic::error(
                    filename,
                    0,
                    format!(
                        "Do not have permissions to set '{key}' to '{value}' in property file '{filename}': {error}"
                    ),
                )),
            }
        }
    }

    /// `load_properties_from_partition`.
    fn load_partition(
        &mut self,
        partition: &str,
        support_legacy_path_until: i64,
        properties: &mut BTreeMap<String, String>,
    ) {
        let path = format!("/{partition}/etc/build.prop");
        if self.load_file(&path, None, properties).is_ok() {
            return;
        }
        let legacy1 = format!("/{partition}/default.prop");
        let legacy2 = format!("/{partition}/build.prop");
        let mut temp = BTreeMap::new();
        let _ = self.load_file(&legacy1, None, &mut temp);
        let _ = self.load_file(&legacy2, None, &mut temp);
        let version_prop = format!("ro.{partition}.build.version.sdk");
        let support_legacy = match temp.get(&version_prop) {
            None => true,
            Some(value) => parse_int(value, i32::MIN as i64, i32::MAX as i64)
                .is_some_and(|v| v <= support_legacy_path_until),
        };
        if support_legacy {
            let _ = self.load_file(&legacy1, None, properties);
            let _ = self.load_file(&legacy2, None, properties);
        } else {
            // init LOG(FATAL)s here.
            self.diagnostics.push(Diagnostic::error(
                &legacy1,
                0,
                format!(
                    "{legacy1} and {legacy2} were not loaded because {version_prop}({}) is newer than {support_legacy_path_until}",
                    temp[&version_prop]
                ),
            ));
        }
    }

    /// `PropertyLoadBootDefaults`.
    fn load_boot_defaults(&mut self, recovery: bool) {
        let mut properties = BTreeMap::new();
        if recovery && let Err(error) = self.load_file("/prop.default", None, &mut properties) {
            self.diagnostics
                .push(Diagnostic::error("/prop.default", 0, error));
        }
        if self.image.exists(SECOND_STAGE_RAMDISK_PROP)
            && let Err(error) = self.load_file(SECOND_STAGE_RAMDISK_PROP, None, &mut properties)
        {
            self.diagnostics
                .push(Diagnostic::warning(SECOND_STAGE_RAMDISK_PROP, 0, error));
        }
        if let Err(error) = self.load_file("/system/build.prop", None, &mut properties) {
            self.diagnostics
                .push(Diagnostic::warning("/system/build.prop", 0, error));
        }
        self.load_partition("system_ext", 30, &mut properties);
        let _ = self.load_file("/system_dlkm/etc/build.prop", None, &mut properties);
        let _ = self.load_file("/vendor/default.prop", None, &mut properties);
        let _ = self.load_file("/vendor/build.prop", None, &mut properties);
        let _ = self.load_file("/vendor_dlkm/etc/build.prop", None, &mut properties);
        let _ = self.load_file("/odm_dlkm/etc/build.prop", None, &mut properties);
        self.load_partition("odm", 28, &mut properties);
        self.load_partition("product", 30, &mut properties);
        if self.image.exists(DEBUG_RAMDISK_PROP)
            && let Err(error) = self.load_file(DEBUG_RAMDISK_PROP, None, &mut properties)
        {
            self.diagnostics
                .push(Diagnostic::warning(DEBUG_RAMDISK_PROP, 0, error));
        }

        for (name, value) in &properties {
            self.set_no_socket(name, value, "while loading .prop files");
        }

        self.initialize_ro_product_props();
        self.initialize_build_id();
        self.derive_build_fingerprint();
        self.derive_legacy_build_fingerprint();
        self.initialize_ro_cpu_abilist();
        self.initialize_ro_vendor_api_level();
        self.update_sys_usb_config();
    }

    /// `property_initialize_ro_product_props`.
    fn initialize_ro_product_props(&mut self) {
        const PROPS: [&str; 5] = ["brand", "device", "manufacturer", "model", "name"];
        const ALLOWED: [&str; 5] = ["odm", "product", "system_ext", "system", "vendor"];
        const DEFAULT_ORDER: &str = "product,odm,vendor,system_ext,system";
        let mut order = self.get("ro.product.property_source_order", "");
        if !order.is_empty() {
            if order.split(',').any(|source| !ALLOWED.contains(&source)) {
                self.diagnostics.push(Diagnostic::error(
                    "<init>",
                    0,
                    "Found unexpected source in ro.product.property_source_order; using the default property source order",
                ));
                order = DEFAULT_ORDER.to_string();
            }
        } else {
            order = DEFAULT_ORDER.to_string();
        }
        for prop in PROPS {
            let base = format!("ro.product.{prop}");
            if !self.get(&base, "").is_empty() {
                continue;
            }
            for source in order.split(',') {
                let target = format!("ro.product.{source}.{prop}");
                let value = self.get(&target, "");
                if !value.is_empty() {
                    self.set_no_socket(&base, &value, "(product property)");
                    break;
                }
            }
        }
    }

    /// `property_initialize_build_id`.
    fn initialize_build_id(&mut self) {
        if !self.get("ro.build.id", "").is_empty() {
            return;
        }
        let legacy = self.get("ro.build.legacy.id", "");
        let digest = self.get("ro.boot.vbmeta.digest", "");
        let build_id = if digest.len() < 8 {
            self.diagnostics.push(Diagnostic::error(
                "<init>",
                0,
                format!("vbmeta digest size too small {digest}"),
            ));
            legacy
        } else {
            // substr(0, 8) on bytes; a hex digest is ASCII.
            let prefix = String::from_utf8_lossy(&digest.as_bytes()[..8]).into_owned();
            format!("{legacy}.{prefix}")
        };
        self.set_no_socket("ro.build.id", &build_id, "(build id)");
    }

    /// `ConstructBuildFingerprint`.
    fn construct_fingerprint(&self, legacy: bool) -> String {
        let unknown = "unknown";
        let mut fingerprint = self.get("ro.product.brand", unknown);
        fingerprint.push('/');
        fingerprint.push_str(&self.get("ro.product.name", unknown));
        // The 16 KiB dev-option suffix needs getpagesize() == 16384 at run
        // time; the guest page size is 4 KiB unless the daemon says otherwise.
        fingerprint.push('/');
        fingerprint.push_str(&self.get("ro.product.device", unknown));
        fingerprint.push(':');
        fingerprint.push_str(&self.get("ro.build.version.release_or_codename", unknown));
        fingerprint.push('/');
        let id_prop = if legacy {
            "ro.build.legacy.id"
        } else {
            "ro.build.id"
        };
        fingerprint.push_str(&self.get(id_prop, unknown));
        fingerprint.push('/');
        fingerprint.push_str(&self.get("ro.build.version.incremental", unknown));
        fingerprint.push(':');
        fingerprint.push_str(&self.get("ro.build.type", unknown));
        fingerprint.push('/');
        fingerprint.push_str(&self.get("ro.build.tags", unknown));
        fingerprint
    }

    fn derive_build_fingerprint(&mut self) {
        if !self.get("ro.build.fingerprint", "").is_empty() {
            return;
        }
        let fingerprint = self.construct_fingerprint(false);
        self.set_no_socket("ro.build.fingerprint", &fingerprint, "(fingerprint)");
    }

    fn derive_legacy_build_fingerprint(&mut self) {
        if !self.get("ro.build.legacy.fingerprint", "").is_empty() {
            return;
        }
        if self.get("ro.build.legacy.id", "").is_empty() {
            return;
        }
        let fingerprint = self.construct_fingerprint(true);
        self.set_no_socket(
            "ro.build.legacy.fingerprint",
            &fingerprint,
            "(legacy fingerprint)",
        );
    }

    /// `property_initialize_ro_cpu_abilist`.
    fn initialize_ro_cpu_abilist(&mut self) {
        if !self.get("ro.product.cpu.abilist", "").is_empty() {
            return;
        }
        let mut abilist32 = String::new();
        let mut abilist64 = String::new();
        for source in ["product", "odm", "vendor", "system"] {
            abilist32 = self.get(&format!("ro.{source}.product.cpu.abilist32"), "");
            abilist64 = self.get(&format!("ro.{source}.product.cpu.abilist64"), "");
            if !abilist32.is_empty() || !abilist64.is_empty() {
                break;
            }
        }
        let mut abilist = abilist64.clone();
        if !abilist32.is_empty() {
            if !abilist.is_empty() {
                abilist.push(',');
            }
            abilist.push_str(&abilist32);
        }
        self.set_no_socket("ro.product.cpu.abilist", &abilist, "(abilist)");
        self.set_no_socket("ro.product.cpu.abilist32", &abilist32, "(abilist)");
        self.set_no_socket("ro.product.cpu.abilist64", &abilist64, "(abilist)");
    }

    fn int_property(&self, name: &str, default: i64) -> i64 {
        parse_int(&self.get(name, ""), i32::MIN as i64, i32::MAX as i64).unwrap_or(default)
    }

    /// `property_initialize_ro_vendor_api_level`.
    fn initialize_ro_vendor_api_level(&mut self) {
        if self.service.areas().contains("ro.vendor.api_level") {
            return;
        }
        let mut vendor_api_level =
            self.int_property("ro.board.first_api_level", ANDROID_VENDOR_API_MAX);
        if vendor_api_level != ANDROID_VENDOR_API_MAX {
            vendor_api_level = self.int_property("ro.board.api_level", vendor_api_level);
        }
        let mut product_first =
            self.int_property("ro.product.first_api_level", ANDROID_API_FUTURE as i64);
        if product_first == ANDROID_API_FUTURE as i64 {
            product_first = self.int_property("ro.build.version.sdk", ANDROID_API_FUTURE as i64);
        }
        // AVendorSupport_getVendorApiLevelOf.
        let of_product = if product_first < ANDROID_API_V {
            product_first
        } else if product_first < ANDROID_API_FUTURE as i64 {
            202404 + (product_first - ANDROID_API_V) * 100
        } else {
            -1
        };
        vendor_api_level = of_product.min(vendor_api_level);
        if vendor_api_level < 0 {
            self.diagnostics.push(Diagnostic::error(
                "<init>",
                0,
                "Unexpected vendor api level for ro.vendor.api_level. Check ro.product.first_api_level and ro.build.version.sdk.",
            ));
            vendor_api_level = ANDROID_VENDOR_API_MAX;
        }
        self.set_no_socket(
            "ro.vendor.api_level",
            &vendor_api_level.to_string(),
            "(vendor api level)",
        );
    }

    /// `update_sys_usb_config`.
    fn update_sys_usb_config(&mut self) {
        let debuggable = parse_bool(&self.get("ro.debuggable", "")).unwrap_or(false);
        let config = self.get("persist.sys.usb.config", "");
        if config.is_empty() || config == "none" {
            self.init_set(
                "persist.sys.usb.config",
                if debuggable { "adb" } else { "none" },
            );
        } else if debuggable && !config.contains("adb") && config.len() + 4 < PROP_VALUE_MAX {
            self.init_set("persist.sys.usb.config", &format!("{config},adb"));
        }
    }

    /// `ExportKernelBootProps`.
    fn export_kernel_boot_props(&mut self) {
        const MAP: [(&str, &str, &str); 6] = [
            ("ro.boot.serialno", "ro.serialno", ""),
            ("ro.boot.mode", "ro.bootmode", "unknown"),
            ("ro.boot.baseband", "ro.baseband", "unknown"),
            ("ro.boot.bootloader", "ro.bootloader", "unknown"),
            ("ro.boot.hardware", "ro.hardware", "unknown"),
            ("ro.boot.revision", "ro.revision", "0"),
        ];
        for (source, target, default) in MAP {
            let value = self.get(source, default);
            if !value.is_empty() {
                self.init_set(target, &value);
            }
        }
    }
}

/// `PropertyInit` after `__system_property_area_init`: kernel-provided
/// `ro.boot.*`, `ExportKernelBootProps`, `PropertyLoadBootDefaults` and
/// `PropertyLoadDerivedDefaults`. Returns what init would log.
pub fn property_init<M: AreaMemory>(
    service: &mut PropertyService<M>,
    image: &ImageRoot,
    options: &PropertyInitOptions,
) -> Vec<Diagnostic> {
    let mut loader = Loader {
        service,
        image,
        vendor_api_level: options.vendor_api_level,
        diagnostics: Vec::new(),
    };
    for (name, value) in &options.kernel.device_tree {
        loader.init_set(name, value);
    }
    for source in [&options.kernel.cmdline, &options.kernel.bootconfig] {
        for (key, value) in source {
            if let Some(rest) = key.strip_prefix("androidboot.") {
                loader.init_set(&format!("ro.boot.{rest}"), value);
            }
        }
    }
    loader.export_kernel_boot_props();
    loader.load_boot_defaults(options.recovery);
    // PropertyLoadDerivedDefaults.
    if loader.get("ro.boot.hardware.cpu.pagesize", "").is_empty() {
        let page_size = options.page_size.to_string();
        loader.set_no_socket("ro.boot.hardware.cpu.pagesize", &page_size, "(page size)");
    }
    loader.diagnostics
}

/// `StartPropertyService`'s property: tells bionic to speak protocol 2.
pub fn start_property_service<M: AreaMemory>(service: &mut PropertyService<M>) -> Vec<Diagnostic> {
    let outcome = service.init_set("ro.property_service.version", "2");
    let mut diagnostics = Vec::new();
    if !outcome.is_success() {
        diagnostics.push(Diagnostic::error(
            "<init>",
            0,
            format!(
                "Init cannot set 'ro.property_service.version': {}",
                outcome.error.unwrap_or_default()
            ),
        ));
    }
    diagnostics
}

/// Parses `.prop` files into the ordered map `PropertyLoadBootDefaults`
/// builds, without a property service (for inspection and tests). Types and
/// permissions are not checked.
pub fn read_prop_file(contents: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in contents.split('\n') {
        let line = raw.trim_start_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
        if line.starts_with('#') || line.starts_with("import ") {
            continue;
        }
        let line = line.trim_end_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim_end_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
            let value = value.trim_start_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
            out.push((key.to_string(), value.to_string()));
        }
    }
    out
}
