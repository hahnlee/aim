//! The Mac's time zone, language, appearance and keyboard layout, which
//! the device follows (docs/mac-settings.md). guest-init reads them before init's first
//! action and publishes them as `vendor.aim.mac.*` properties, and
//! republishes the time zone and appearance when the Mac changes them.
//! `init.aim.rc` applies the values through Android's own services, as a
//! device's vendor init would. Nothing here changes the Mac's settings.

use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc::Sender;

/// An IANA zone name in the image's tzdata.
pub const TIME_ZONE_PROP: &str = "vendor.aim.mac.time_zone";
/// The primary language as an Android BCP-47 tag, with the Mac's region.
pub const LOCALE_PROP: &str = "vendor.aim.mac.locale";
/// `yes` in Dark appearance, else `no` (`cmd uimode night`).
pub const NIGHT_MODE_PROP: &str = "vendor.aim.mac.night_mode";
/// The built-in keyboard's layout: an InputDevices layout's resource name
/// (`aim-keyboard layout`).
pub const KEYBOARD_LAYOUT_PROP: &str = "vendor.aim.mac.keyboard_layout";

/// The Mac keyboard layouts (`com.apple.keylayout.*`) whose characters an
/// InputDevices layout has, key for key on the unmodified and Shift levels.
const KEYBOARD_LAYOUTS: &[(&str, &str)] = &[
    ("Colemak", "keyboard_layout_english_us_colemak"),
    ("DVORAK-QWERTYCMD", "keyboard_layout_english_us_dvorak"),
    ("Dvorak", "keyboard_layout_english_us_dvorak"),
    ("British-PC", "keyboard_layout_english_uk"),
    ("French-PC", "keyboard_layout_french"),
    ("German", "keyboard_layout_german"),
    ("Russian", "keyboard_layout_russian_mac"),
    ("RussianWin", "keyboard_layout_russian"),
    ("USInternational-PC", "keyboard_layout_english_us_intl"),
];

/// The InputDevices layout for the Mac's keyboard layout `id`
/// (`AppleCurrentKeyboardLayoutInputSourceID`). US English for the others:
/// `ABC` and `US` are, and so is the Latin level of the input methods'
/// layouts (`2SetHangul`), whose own characters are an input method's.
pub fn keyboard_layout(id: &str) -> &'static str {
    let name = id.strip_prefix("com.apple.keylayout.").unwrap_or_default();
    KEYBOARD_LAYOUTS
        .iter()
        .find(|(mac, _)| *mac == name)
        .map_or("keyboard_layout_english_us", |(_, android)| android)
}

/// The guest's tzdata: bionic and libcore resolve zone names in it.
pub const TZDATA: &str = "/apex/com.android.tzdata/etc/tz/tzdata";

/// The IANA name `/etc/localtime` links to
/// (`/var/db/timezone/zoneinfo/Asia/Seoul`).
pub fn zone_from_localtime(target: &str) -> Option<&str> {
    let (_, name) = target.split_once("/zoneinfo/")?;
    let valid = !name.is_empty()
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_+-.".contains(&b))
        });
    valid.then_some(name)
}

/// Whether bionic's tzdata file (`ZoneInfoDb`: a 12-byte version, the
/// index, data and final offsets, then 52-byte entries of a 40-byte name
/// and three integers) has a zone called `name`.
pub fn tzdata_has(tzdata: &[u8], name: &str) -> bool {
    let be = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(tzdata.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    if !tzdata.starts_with(b"tzdata") || name.len() >= 40 {
        return false;
    }
    let (Some(index), Some(data)) = (be(12), be(16)) else {
        return false;
    };
    let Some(entries) = tzdata.get(index..data.min(tzdata.len())) else {
        return false;
    };
    entries.chunks_exact(52).any(|entry| {
        let stored = &entry[..40];
        let end = stored.iter().position(|&b| b == 0).unwrap_or(40);
        &stored[..end] == name.as_bytes()
    })
}

/// A macOS language or locale identifier (`zh-Hans`, `sr_Latn_RS`,
/// `en_KR@rg=gbzzzz`) as a BCP-47 tag with canonical case, the region
/// `region` added when it has none. `None` for what is not a
/// language-script-region tag, such as the pre-10.4 names (`English`).
pub fn android_locale(tag: &str, region: Option<&str>) -> Option<String> {
    let tag = tag.split('@').next().unwrap_or_default();
    let mut parts = tag.split(['-', '_']);
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = language.to_ascii_lowercase();
    let mut has_region = false;
    for part in parts {
        let alpha = part.bytes().all(|b| b.is_ascii_alphabetic());
        let digits = part.bytes().all(|b| b.is_ascii_digit());
        out.push('-');
        match part.len() {
            4 if alpha && !has_region => {
                out.push_str(&part[..1].to_ascii_uppercase());
                out.push_str(&part[1..].to_ascii_lowercase());
            }
            2 if alpha => {
                has_region = true;
                out.push_str(&part.to_ascii_uppercase());
            }
            3 if digits => {
                has_region = true;
                out.push_str(part);
            }
            5..=8 if part.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                out.push_str(&part.to_ascii_lowercase())
            }
            _ => return None,
        }
    }
    if !has_region && let Some(region) = region {
        out.push('-');
        out.push_str(&region.to_ascii_uppercase());
    }
    Some(out)
}

/// The region of an `AppleLocale` value (`ko_KR`, `en_US@rg=krzzzz`):
/// the `rg` override if any, else its region subtag.
pub fn locale_region(apple_locale: &str) -> Option<String> {
    let (locale, keywords) = apple_locale.split_once('@').unwrap_or((apple_locale, ""));
    let from_rg = keywords.split(';').find_map(|kv| {
        let rg = kv.strip_prefix("rg=")?;
        let region = rg.get(..2)?;
        region
            .bytes()
            .all(|b| b.is_ascii_alphabetic())
            .then(|| region.to_ascii_uppercase())
    });
    from_rg.or_else(|| {
        locale
            .split(['-', '_'])
            .skip(1)
            .find(|p| {
                (p.len() == 2 && p.bytes().all(|b| b.is_ascii_alphabetic()))
                    || (p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(str::to_ascii_uppercase)
    })
}

/// The device locale for the Mac's preferred languages and region: the
/// first language Android can name. Only one: `persist.sys.locale` holds a
/// single tag.
pub fn primary_locale(languages: &[String], apple_locale: Option<&str>) -> Option<String> {
    let region = apple_locale.and_then(locale_region);
    languages
        .iter()
        .find_map(|tag| android_locale(tag, region.as_deref()))
}

/// `cmd uimode night`'s argument for an `AppleInterfaceStyle` value.
pub fn night_mode(interface_style: Option<&str>) -> &'static str {
    if interface_style == Some("Dark") {
        "yes"
    } else {
        "no"
    }
}

/// The Mac's current values of the `vendor.aim.mac.*` properties. The time
/// zone is left out when the guest's tzdata (`tzdata`) lacks it.
pub fn properties(tzdata: &[u8]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Ok(target) = std::fs::read_link("/etc/localtime")
        && let Some(zone) = zone_from_localtime(&target.to_string_lossy())
        && tzdata_has(tzdata, zone)
    {
        out.push((TIME_ZONE_PROP, zone.to_string()));
    }
    // SAFETY: CoreFoundation calls on values this function owns.
    unsafe {
        CFPreferencesAppSynchronize(*kCFPreferencesAnyApplication());
        let languages = preference("AppleLanguages").map_or_else(Vec::new, |v| v.strings());
        let apple_locale = preference("AppleLocale").and_then(|v| v.string());
        if let Some(locale) = primary_locale(&languages, apple_locale.as_deref()) {
            out.push((LOCALE_PROP, locale));
        }
        let style = preference("AppleInterfaceStyle").and_then(|v| v.string());
        out.push((NIGHT_MODE_PROP, night_mode(style.as_deref()).to_string()));
        let toolbox = cfstring(c"com.apple.HIToolbox");
        CFPreferencesAppSynchronize(toolbox);
        let layout = app_preference("AppleCurrentKeyboardLayoutInputSourceID", toolbox)
            .and_then(|v| v.string());
        CFRelease(toolbox);
        out.push((
            KEYBOARD_LAYOUT_PROP,
            keyboard_layout(layout.as_deref().unwrap_or_default()).to_string(),
        ));
    }
    out
}

/// Sends the name and value of each property whose value changes
/// from `initial`, checking every [`POLL`]: a `readlink` and a preferences
/// read, with no run loop for the Mac's change notifications in
/// guest-init.
pub fn watch(
    events: Sender<(String, String)>,
    tzdata: Vec<u8>,
    initial: Vec<(&'static str, String)>,
) {
    std::thread::Builder::new()
        .name("mac-settings".into())
        .spawn(move || {
            let mut last = initial;
            loop {
                std::thread::sleep(POLL);
                let current = properties(&tzdata);
                for (name, value) in &current {
                    // The language is applied when Android starts
                    // (`persist.sys.locale`), so only a boot follows it.
                    if *name != LOCALE_PROP
                        && !last.iter().any(|(n, v)| n == name && v == value)
                        && events.send((name.to_string(), value.clone())).is_err()
                    {
                        return;
                    }
                }
                last = current;
            }
        })
        .expect("mac-settings thread");
}

/// How often [`watch`] reads the Mac's settings.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(2);

type CFTypeRef = *const c_void;

const UTF8: u32 = 0x0800_0100;

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        static kCFPreferencesAnyApplication: CFTypeRef;
        fn CFPreferencesAppSynchronize(app: CFTypeRef) -> bool;
        fn CFPreferencesCopyAppValue(key: CFTypeRef, app: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }
}

unsafe fn cfstring(s: &CStr) -> CFTypeRef {
    // SAFETY: a NUL-terminated UTF-8 string.
    unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) }
}

/// An owned CoreFoundation value.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: a +1 reference this value owns.
        unsafe { CFRelease(self.0) }
    }
}

impl Cf {
    fn string(&self) -> Option<String> {
        // SAFETY: a valid CF value.
        unsafe { to_string(self.0) }
    }

    fn strings(&self) -> Vec<String> {
        // SAFETY: a valid CF value; array elements are borrowed.
        unsafe {
            if CFGetTypeID(self.0) != CFArrayGetTypeID() {
                return Vec::new();
            }
            (0..CFArrayGetCount(self.0))
                .filter_map(|i| to_string(CFArrayGetValueAtIndex(self.0, i)))
                .collect()
        }
    }
}

/// The global-domain value of `key`, as `defaults read -g` shows it.
unsafe fn preference(key: &str) -> Option<Cf> {
    // SAFETY: the any-application domain.
    unsafe { app_preference(key, *kCFPreferencesAnyApplication()) }
}

/// Application `app`'s value of `key`, as `defaults read APP KEY` shows
/// it.
unsafe fn app_preference(key: &str, app: CFTypeRef) -> Option<Cf> {
    let key = std::ffi::CString::new(key).ok()?;
    // SAFETY: CF calls on a string this function creates and releases.
    unsafe {
        let key = cfstring(&key);
        let value = CFPreferencesCopyAppValue(key, app);
        CFRelease(key);
        (!value.is_null()).then(|| Cf(value))
    }
}

unsafe fn to_string(value: CFTypeRef) -> Option<String> {
    // SAFETY: a valid CF value, checked to be a string.
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8)
            .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_is_the_path_below_zoneinfo() {
        let zone = |t| zone_from_localtime(t);
        assert_eq!(
            zone("/var/db/timezone/zoneinfo/Asia/Seoul"),
            Some("Asia/Seoul")
        );
        assert_eq!(
            zone("/usr/share/zoneinfo/America/Argentina/Buenos_Aires"),
            Some("America/Argentina/Buenos_Aires")
        );
        assert_eq!(
            zone("/var/db/timezone/zoneinfo/Etc/GMT+9"),
            Some("Etc/GMT+9")
        );
        assert_eq!(zone("/var/db/timezone/zoneinfo/UTC"), Some("UTC"));
        assert_eq!(zone("/var/db/timezone/zoneinfo/"), None);
        assert_eq!(zone("/var/db/timezone/zoneinfo/../../etc"), None);
        assert_eq!(zone("/etc/foo"), None);
    }

    fn tzdata(names: &[&str]) -> Vec<u8> {
        let mut out = b"tzdata2025a\0".to_vec();
        let index = 24u32;
        let data = index + 52 * names.len() as u32;
        for offset in [index, data, data] {
            out.extend(offset.to_be_bytes());
        }
        for name in names {
            let mut entry = [0u8; 52];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            out.extend(entry);
        }
        out
    }

    #[test]
    fn tzdata_index_lookup() {
        let data = tzdata(&[
            "Africa/Abidjan",
            "Asia/Seoul",
            "America/Argentina/ComodRivadavia",
        ]);
        assert!(tzdata_has(&data, "Asia/Seoul"));
        assert!(tzdata_has(&data, "America/Argentina/ComodRivadavia"));
        assert!(!tzdata_has(&data, "Asia/Seo"));
        assert!(!tzdata_has(&data, "Mars/Olympus"));
        assert!(!tzdata_has(b"nottzdata", "Asia/Seoul"));
        assert!(!tzdata_has(&data[..30], "Asia/Seoul"));
    }

    #[test]
    fn macos_language_tags_as_android_locales() {
        let l = |tag, region| android_locale(tag, region);
        assert_eq!(l("ko-KR", None).as_deref(), Some("ko-KR"));
        assert_eq!(l("ko", Some("KR")).as_deref(), Some("ko-KR"));
        assert_eq!(l("en-US", Some("KR")).as_deref(), Some("en-US"));
        assert_eq!(l("zh-Hans", Some("CN")).as_deref(), Some("zh-Hans-CN"));
        assert_eq!(l("zh-Hant-TW", Some("KR")).as_deref(), Some("zh-Hant-TW"));
        assert_eq!(l("zh_Hant_HK", None).as_deref(), Some("zh-Hant-HK"));
        assert_eq!(l("sr-Latn", None).as_deref(), Some("sr-Latn"));
        assert_eq!(l("sr-Latn-RS", None).as_deref(), Some("sr-Latn-RS"));
        assert_eq!(l("es-419", Some("MX")).as_deref(), Some("es-419"));
        assert_eq!(l("yue-Hant", Some("HK")).as_deref(), Some("yue-Hant-HK"));
        assert_eq!(l("ca-ES-valencia", None).as_deref(), Some("ca-ES-valencia"));
        assert_eq!(l("EN_gb", None).as_deref(), Some("en-GB"));
        assert_eq!(l("en_US@rg=krzzzz", None).as_deref(), Some("en-US"));
        assert_eq!(l("English", Some("US")), None);
        assert_eq!(l("", None), None);
        assert_eq!(l("en--US", None), None);
    }

    #[test]
    fn region_of_apple_locale() {
        assert_eq!(locale_region("ko_KR").as_deref(), Some("KR"));
        assert_eq!(locale_region("zh-Hans_CN").as_deref(), Some("CN"));
        assert_eq!(locale_region("en_US@rg=gbzzzz").as_deref(), Some("GB"));
        assert_eq!(locale_region("es_419").as_deref(), Some("419"));
        assert_eq!(locale_region("en"), None);
    }

    #[test]
    fn primary_locale_is_the_first_nameable_language() {
        let langs = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            primary_locale(&langs(&["ko-KR", "en-US"]), Some("ko_KR")).as_deref(),
            Some("ko-KR")
        );
        assert_eq!(
            primary_locale(&langs(&["English", "ja"]), Some("ja_JP")).as_deref(),
            Some("ja-JP")
        );
        assert_eq!(
            primary_locale(&langs(&["en"]), Some("en_KR@rg=krzzzz")).as_deref(),
            Some("en-KR")
        );
        assert_eq!(primary_locale(&[], Some("ko_KR")), None);
    }

    #[test]
    fn dark_appearance_is_night_mode() {
        assert_eq!(night_mode(Some("Dark")), "yes");
        assert_eq!(night_mode(None), "no");
        assert_eq!(night_mode(Some("Light")), "no");
    }

    #[test]
    fn mac_keyboard_layouts_as_android_layouts() {
        let l = keyboard_layout;
        assert_eq!(
            l("com.apple.keylayout.Dvorak"),
            "keyboard_layout_english_us_dvorak"
        );
        assert_eq!(
            l("com.apple.keylayout.RussianWin"),
            "keyboard_layout_russian"
        );
        assert_eq!(l("com.apple.keylayout.ABC"), "keyboard_layout_english_us");
        assert_eq!(
            l("com.apple.keylayout.2SetHangul"),
            "keyboard_layout_english_us"
        );
        assert_eq!(l(""), "keyboard_layout_english_us");
    }

    #[test]
    fn reads_the_macs_settings() {
        // No tzdata: the zone is left out. The appearance is always named.
        let props = properties(&[]);
        assert!(!props.iter().any(|(n, _)| *n == TIME_ZONE_PROP));
        let night = props.iter().find(|(n, _)| *n == NIGHT_MODE_PROP).unwrap();
        assert!(night.1 == "yes" || night.1 == "no");
        let layout = props
            .iter()
            .find(|(n, _)| *n == KEYBOARD_LAYOUT_PROP)
            .unwrap();
        assert!(layout.1.starts_with("keyboard_layout_"), "{}", layout.1);
    }
}
