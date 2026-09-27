//! User and group names as init resolves them (`DecodeUid`), which on
//! Android is bionic's `getpwnam` (libc/bionic/grp_pwd.cpp).
//!
//! Lookup order, as bionic's `getpwnam_internal` / `getgrnam_internal`:
//! 1. the built-in AIDs (`generated_android_ids.h`, generated from
//!    `android_filesystem_config.h` by `fs_config_generator.py aidarray`);
//! 2. the partition passwd/group files, each requiring its partition prefix;
//! 3. `oem_NNNN` in the OEM ranges;
//! 4. app names (`u0_a12`, `u10_system`, `u0_i3`, and for groups `all_a5`,
//!    `u0_a12_cache`, `u0_a12_ext`, `u0_a12_ext_cache`).

use crate::ImageRoot;
use crate::PropertyLookup;

/// `generated_android_ids.h` for android-16.0.0_r1: every `AID_*` define in
/// libcutils/include/private/android_filesystem_config.h except `AID_APP*`,
/// `AID_USER*`, `AID_UNUSED*` and `*_START` / `*_END`, lower-cased with the
/// generator's `media_drm`/`media_ex`/`media_codec` fixups.
pub const ANDROID_IDS: &[(&str, u32)] = &[
    ("root", 0),
    ("daemon", 1),
    ("bin", 2),
    ("sys", 3),
    ("system", 1000),
    ("radio", 1001),
    ("bluetooth", 1002),
    ("graphics", 1003),
    ("input", 1004),
    ("audio", 1005),
    ("camera", 1006),
    ("log", 1007),
    ("compass", 1008),
    ("mount", 1009),
    ("wifi", 1010),
    ("adb", 1011),
    ("install", 1012),
    ("media", 1013),
    ("dhcp", 1014),
    ("sdcard_rw", 1015),
    ("vpn", 1016),
    ("keystore", 1017),
    ("usb", 1018),
    ("drm", 1019),
    ("mdnsr", 1020),
    ("gps", 1021),
    ("media_rw", 1023),
    ("mtp", 1024),
    ("drmrpc", 1026),
    ("nfc", 1027),
    ("sdcard_r", 1028),
    ("clat", 1029),
    ("loop_radio", 1030),
    ("mediadrm", 1031),
    ("package_info", 1032),
    ("sdcard_pics", 1033),
    ("sdcard_av", 1034),
    ("sdcard_all", 1035),
    ("logd", 1036),
    ("shared_relro", 1037),
    ("dbus", 1038),
    ("tlsdate", 1039),
    ("mediaex", 1040),
    ("audioserver", 1041),
    ("metrics_coll", 1042),
    ("metricsd", 1043),
    ("webserv", 1044),
    ("debuggerd", 1045),
    ("mediacodec", 1046),
    ("cameraserver", 1047),
    ("firewall", 1048),
    ("trunks", 1049),
    ("nvram", 1050),
    ("dns", 1051),
    ("dns_tether", 1052),
    ("webview_zygote", 1053),
    ("vehicle_network", 1054),
    ("media_audio", 1055),
    ("media_video", 1056),
    ("media_image", 1057),
    ("tombstoned", 1058),
    ("media_obb", 1059),
    ("ese", 1060),
    ("ota_update", 1061),
    ("automotive_evs", 1062),
    ("lowpan", 1063),
    ("hsm", 1064),
    ("reserved_disk", 1065),
    ("statsd", 1066),
    ("incidentd", 1067),
    ("secure_element", 1068),
    ("lmkd", 1069),
    ("llkd", 1070),
    ("iorapd", 1071),
    ("gpu_service", 1072),
    ("network_stack", 1073),
    ("gsid", 1074),
    ("fsverity_cert", 1075),
    ("credstore", 1076),
    ("external_storage", 1077),
    ("ext_data_rw", 1078),
    ("ext_obb_rw", 1079),
    ("context_hub", 1080),
    ("virtualizationservice", 1081),
    ("artd", 1082),
    ("uwb", 1083),
    ("thread_network", 1084),
    ("diced", 1085),
    ("dmesgd", 1086),
    ("jc_weaver", 1087),
    ("jc_strongbox", 1088),
    ("jc_identitycred", 1089),
    ("sdk_sandbox", 1090),
    ("security_log_writer", 1091),
    ("prng_seeder", 1092),
    ("uprobestats", 1093),
    ("cros_ec", 1094),
    ("mmd", 1095),
    ("update_engine_log", 1096),
    ("shell", 2000),
    ("cache", 2001),
    ("diag", 2002),
    ("net_bt_admin", 3001),
    ("net_bt", 3002),
    ("inet", 3003),
    ("net_raw", 3004),
    ("net_admin", 3005),
    ("net_bw_stats", 3006),
    ("net_bw_acct", 3007),
    ("readproc", 3009),
    ("wakelock", 3010),
    ("uhid", 3011),
    ("readtracefs", 3012),
    ("virtualmachine", 3013),
    ("everybody", 9997),
    ("misc", 9998),
    ("nobody", 9999),
    ("overflowuid", 65534),
];

pub const AID_OEM_RESERVED_START: u32 = 2900;
pub const AID_OEM_RESERVED_END: u32 = 2999;
pub const AID_OEM_RESERVED_2_START: u32 = 5000;
pub const AID_OEM_RESERVED_2_END: u32 = 5999;
pub const AID_EVERYBODY: u32 = 9997;
pub const AID_APP_START: u32 = 10000;
pub const AID_CACHE_GID_START: u32 = 20000;
pub const AID_EXT_GID_START: u32 = 30000;
pub const AID_EXT_CACHE_GID_START: u32 = 40000;
pub const AID_SHARED_GID_START: u32 = 50000;
pub const AID_SHARED_GID_END: u32 = 59999;
pub const AID_ISOLATED_START: u32 = 90000;
pub const AID_USER_OFFSET: u32 = 100000;

/// `secondary_user_platform_ids` in grp_pwd.cpp.
const SECONDARY_USER_PLATFORM_IDS: &[u32] = &[1000, 1001, 1007, 1027, 1002, 2000, 1068, 1073];

/// bionic's `passwd_files` / `group_files` with their required prefixes.
pub const PASSWD_FILES: &[(&str, &str)] = &[
    ("/system/etc/passwd", "system_"),
    ("/vendor/etc/passwd", "vendor_"),
    ("/odm/etc/passwd", "odm_"),
    ("/product/etc/passwd", "product_"),
    ("/system_ext/etc/passwd", "system_ext_"),
];
pub const GROUP_FILES: &[(&str, &str)] = &[
    ("/system/etc/group", "system_"),
    ("/vendor/etc/group", "vendor_"),
    ("/odm/etc/group", "odm_"),
    ("/product/etc/group", "product_"),
    ("/system_ext/etc/group", "system_ext_"),
];

/// A `name:x:id:...` entry from a partition passwd/group file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DatabaseEntry {
    name: String,
    id: u32,
}

/// The id databases init's `DecodeUid` consults.
#[derive(Clone, Debug, Default)]
pub struct IdResolver {
    passwd: Vec<DatabaseEntry>,
    group: Vec<DatabaseEntry>,
    /// `ro.product.first_api_level` in (0, 29): the legacy OEM range applies.
    launched_before_api_29: bool,
}

impl IdResolver {
    /// Built-in AIDs only.
    pub fn builtin() -> Self {
        Self::default()
    }

    /// Built-in AIDs plus the image's partition passwd/group files.
    pub fn from_image(image: &ImageRoot, properties: &dyn PropertyLookup) -> Self {
        let first_api_level: i64 = properties
            .property_or("ro.product.first_api_level", "")
            .parse()
            .unwrap_or(0);
        Self {
            passwd: read_database(image, PASSWD_FILES),
            group: read_database(image, GROUP_FILES),
            launched_before_api_29: first_api_level != 0 && first_api_level < 29,
        }
    }

    /// `DecodeUid`: a name starting with an ASCII letter goes through
    /// `getpwnam`; anything else is `strtoul(name, 0, 0)`.
    ///
    /// init uses `getpwnam` for group names too, so groups resolve through
    /// the passwd database; [`IdResolver::decode_gid`] is the same call.
    pub fn decode_uid(&self, name: &str) -> Result<u32, String> {
        let first = name.as_bytes().first().copied().unwrap_or(0);
        if first.is_ascii_alphabetic() {
            return self
                .getpwnam(name)
                .ok_or_else(|| "getpwnam failed: No such file or directory".to_string());
        }
        strtoul(name).ok_or_else(|| "strtoul failed: Numerical result out of range".to_string())
    }

    /// init decodes gids with the same `DecodeUid` (passwd lookup).
    pub fn decode_gid(&self, name: &str) -> Result<u32, String> {
        self.decode_uid(name)
    }

    /// bionic `getpwnam`.
    pub fn getpwnam(&self, login: &str) -> Option<u32> {
        if let Some(id) = builtin_id(login) {
            return Some(id);
        }
        if let Some(entry) = self.passwd.iter().find(|e| e.name == login) {
            return Some(entry.id);
        }
        if let Some(id) = self.oem_id_from_name(login) {
            return Some(id);
        }
        app_id_from_name(login, false)
    }

    /// bionic `getgrnam`.
    pub fn getgrnam(&self, name: &str) -> Option<u32> {
        if let Some(id) = builtin_id(name) {
            return Some(id);
        }
        if let Some(entry) = self.group.iter().find(|e| e.name == name) {
            return Some(entry.id);
        }
        if let Some(id) = self.oem_id_from_name(name) {
            return Some(id);
        }
        app_id_from_name(name, true)
    }

    fn is_oem_id(&self, id: u32) -> bool {
        if self.launched_before_api_29
            && (AID_OEM_RESERVED_START..AID_EVERYBODY).contains(&id)
            && !ANDROID_IDS.iter().any(|(_, aid)| *aid == id)
        {
            return true;
        }
        (AID_OEM_RESERVED_START..=AID_OEM_RESERVED_END).contains(&id)
            || (AID_OEM_RESERVED_2_START..=AID_OEM_RESERVED_2_END).contains(&id)
    }

    /// `sscanf(name, "oem_%u", &id)`: trailing text after the number is
    /// ignored, as sscanf does.
    fn oem_id_from_name(&self, name: &str) -> Option<u32> {
        let digits = name.strip_prefix("oem_")?;
        let (id, _) = scan_unsigned(digits)?;
        let id = u32::try_from(id).ok()?;
        if id == 0 || !self.is_oem_id(id) {
            return None;
        }
        Some(id)
    }
}

fn builtin_id(name: &str) -> Option<u32> {
    ANDROID_IDS
        .iter()
        .find(|(aid_name, _)| *aid_name == name)
        .map(|(_, id)| *id)
}

fn read_database(image: &ImageRoot, files: &[(&str, &str)]) -> Vec<DatabaseEntry> {
    let mut entries = Vec::new();
    for (path, prefix) in files {
        let Ok(bytes) = image.read(path) else {
            continue;
        };
        // bionic refuses a file whose last byte is not '\n'.
        if bytes.last() != Some(&b'\n') {
            continue;
        }
        for line in String::from_utf8_lossy(&bytes).lines() {
            let fields: Vec<&str> = line.split(':').collect();
            let name = fields[0];
            if !name.starts_with(prefix) {
                continue;
            }
            if let Some(id) = fields.get(2).and_then(|f| f.parse::<u32>().ok()) {
                entries.push(DatabaseEntry {
                    name: name.to_string(),
                    id,
                });
            }
        }
    }
    entries
}

/// `strtoul(s, &end, 10)` returning the value and the unparsed rest; `None`
/// when no digits were consumed.
fn scan_unsigned(s: &str) -> Option<(u64, &str)> {
    let end = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if end == 0 {
        return None;
    }
    let value = s[..end].parse::<u64>().unwrap_or(u64::MAX);
    Some((value, &s[end..]))
}

/// `strtoul(name, 0, 0)` as `DecodeUid` calls it: leading blanks and sign,
/// `0x` hex, leading-`0` octal, stops at the first invalid byte, and only
/// overflow sets errno. The result is truncated to `uid_t`.
fn strtoul(name: &str) -> Option<u32> {
    let s = name.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if (s.starts_with("0x") || s.starts_with("0X"))
        && s.as_bytes().get(2).is_some_and(|b| b.is_ascii_hexdigit())
    {
        (16, &s[2..])
    } else if s.starts_with('0') {
        (8, s)
    } else {
        (10, s)
    };
    let mut value: u64 = 0;
    for byte in digits.bytes() {
        let digit = match (byte as char).to_digit(radix) {
            Some(digit) => digit as u64,
            None => break,
        };
        value = value.checked_mul(radix as u64)?.checked_add(digit)?;
    }
    let value = if negative {
        value.wrapping_neg()
    } else {
        value
    };
    Some(value as u32)
}

/// bionic `app_id_from_name`.
fn app_id_from_name(name: &str, is_group: bool) -> Option<u32> {
    let bytes = name.as_bytes();
    let (userid, is_shared_gid, end): (u64, bool, &str) = if is_group && name.starts_with("all") {
        (0, true, &name[3..])
    } else if bytes.first() == Some(&b'u') && bytes.get(1).is_some_and(u8::is_ascii_digit) {
        let (userid, rest) = scan_unsigned(&name[1..])?;
        (userid, false, rest)
    } else {
        return None;
    };

    let tail = end.strip_prefix('_')?;
    if tail.is_empty() {
        return None;
    }

    let tail_bytes = tail.as_bytes();
    let appid: u64;
    let mut rest: &str;
    if tail_bytes[0] == b'a' && tail_bytes.get(1).is_some_and(u8::is_ascii_digit) {
        let (number, after) = scan_unsigned(&tail[1..])?;
        rest = after;
        if is_shared_gid {
            appid = number + AID_SHARED_GID_START as u64;
            if appid > AID_SHARED_GID_END as u64 {
                return None;
            }
        } else if is_group {
            if rest == "_ext_cache" {
                rest = "";
                appid = number + AID_EXT_CACHE_GID_START as u64;
            } else if rest == "_ext" {
                rest = "";
                appid = number + AID_EXT_GID_START as u64;
            } else if rest == "_cache" {
                rest = "";
                appid = number + AID_CACHE_GID_START as u64;
            } else {
                appid = number + AID_APP_START as u64;
            }
        } else {
            appid = number + AID_APP_START as u64;
        }
    } else if tail_bytes[0] == b'i' && tail_bytes.get(1).is_some_and(u8::is_ascii_digit) {
        let (number, after) = scan_unsigned(&tail[1..])?;
        rest = after;
        appid = number + AID_ISOLATED_START as u64;
    } else if let Some(id) = builtin_id(tail) {
        if !SECONDARY_USER_PLATFORM_IDS.contains(&id) {
            return None;
        }
        appid = id as u64;
        rest = "";
    } else {
        // None of the three cases consumed the name: bionic's `end` still
        // points at the '_' and the "entire string consumed" check fails.
        return None;
    }

    if !rest.is_empty() || userid > 1000 || appid >= AID_USER_OFFSET as u64 {
        return None;
    }
    Some((appid + userid * AID_USER_OFFSET as u64) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_like_bionic() {
        let ids = IdResolver::builtin();
        assert_eq!(ids.decode_uid("system"), Ok(1000));
        assert_eq!(ids.decode_uid("mediacodec"), Ok(1046));
        assert_eq!(ids.decode_uid("audioserver"), Ok(1041));
        assert_eq!(ids.decode_uid("root"), Ok(0));
        assert_eq!(ids.decode_uid("1000"), Ok(1000));
        assert_eq!(ids.decode_uid("0x10"), Ok(16));
        assert_eq!(ids.decode_uid("010"), Ok(8));
        assert!(ids.decode_uid("no_such_user").is_err());
        assert_eq!(ids.getpwnam("oem_2901"), Some(2901));
        assert_eq!(ids.getpwnam("oem_3000"), None);
        assert_eq!(ids.getpwnam("u0_a12"), Some(10012));
        assert_eq!(ids.getpwnam("u10_system"), Some(1_001_000));
        assert_eq!(ids.getpwnam("u10_media"), None);
        assert_eq!(ids.getpwnam("u0_i3"), Some(90003));
        assert_eq!(ids.getgrnam("u0_a12_cache"), Some(20012));
        assert_eq!(ids.getgrnam("all_a5"), Some(50005));
        assert_eq!(ids.getpwnam("u0_a12_cache"), None);
    }
}
