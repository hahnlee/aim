//! The device's languages follow the Mac's preferred languages (#344,
//! docs/mac-settings.md). The Mac's list ([`mac`]) is handed to
//! system_server's bridge (`IBridge.updateLocales`, which applies it as
//! Settings' language page does) when the bridge attaches, at every boot
//! and after system_server restarted, and on each change of the Mac's
//! list. A choice made in Android holds until the Mac's list changes
//! again, as the appearance does. Android never changes the Mac's.

mod mac;

use std::sync::{Arc, mpsc};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::dev_aim_server_ibridge as ib;

pub use mac::preferred_locales;

use crate::system::System;

enum Event {
    /// system_server attached its bridge.
    Attached(Strong),
    /// The Mac's languages or region may have changed.
    MacChanged,
}

/// Follows the Mac's languages from now on, through each bridge `system`
/// is handed.
pub(crate) fn start(process: Arc<LocalProcess>, system: &System) {
    let (events, received) = mpsc::channel();
    let attached = events.clone();
    system.add_bridge_listener(Box::new(move |handle| {
        let _ = attached.send(Event::Attached(process.strong(handle)));
    }));
    mac::watch(Box::new(move || {
        let _ = events.send(Event::MacChanged);
    }));
    std::thread::Builder::new()
        .name("locale".into())
        .spawn(move || {
            let mut bridge = None;
            let mut last = Vec::new();
            for event in received {
                let attached = matches!(event, Event::Attached(_));
                if let Event::Attached(strong) = event {
                    bridge = Some(strong);
                }
                let Some(tags) = update(&mut last, preferred_locales(), attached) else {
                    continue;
                };
                if let Some(strong) = &bridge
                    && !apply(strong, &tags)
                {
                    bridge = None;
                }
            }
        })
        .expect("spawn the locale thread");
}

/// The languages to apply (`IBridge.updateLocales`' comma-separated tags)
/// for the Mac's list `now`: when a bridge attached, whatever it is; on a
/// change notification, only if it differs from `last`, the list seen
/// before, so that a notification of anything else keeps a choice made in
/// Android. Nothing for an empty list.
fn update(last: &mut Vec<String>, now: Vec<String>, attached: bool) -> Option<String> {
    if now.is_empty() || (!attached && now == *last) {
        return None;
    }
    let tags = now.join(",");
    *last = now;
    Some(tags)
}

/// `IBridge.updateLocales(tags)`; false if system_server is gone.
fn apply(bridge: &Strong, tags: &str) -> bool {
    let mut data = Parcel::new();
    ib::UpdateLocales {
        language_tags: Some(tags.to_string()),
    }
    .write(&mut data);
    let reply = bridge.transact(ib::UPDATE_LOCALES, &data, false);
    match reply.map(|r| ib::read_update_locales_reply(&mut r.reader())) {
        Ok(Ok(Ok(()))) => {
            eprintln!("locale: the Mac's languages {tags}");
            true
        }
        Ok(Ok(Err(e))) => {
            eprintln!("locale: updateLocales({tags}): {}", e.message);
            true
        }
        Ok(Err(s)) | Err(s) => {
            eprintln!("locale: updateLocales({tags}): status {s}");
            false
        }
    }
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

/// The device's locale list for the Mac's preferred languages and its
/// region (`AppleLocale`): each language Android can name, in order, once
/// (`LocaleList` refuses a repetition).
pub fn android_locales(languages: &[String], apple_locale: Option<&str>) -> Vec<String> {
    let region = apple_locale.and_then(locale_region);
    let mut out: Vec<String> = Vec::new();
    for tag in languages {
        if let Some(locale) = android_locale(tag, region.as_deref())
            && !out.contains(&locale)
        {
            out.push(locale);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
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
    fn the_list_keeps_the_macs_order_without_repetitions() {
        let list = |l: &[&str], locale| android_locales(&strings(l), locale);
        assert_eq!(
            list(&["ko-KR", "en-US", "ja-JP"], Some("ko_KR")),
            ["ko-KR", "en-US", "ja-JP"]
        );
        assert_eq!(list(&["English", "ja"], Some("ja_JP")), ["ja-JP"]);
        assert_eq!(
            list(&["en", "zh-Hans", "en-KR"], Some("en_KR@rg=krzzzz")),
            ["en-KR", "zh-Hans-KR"]
        );
        assert_eq!(list(&["en_GB", "en-gb", "fr"], None), ["en-GB", "fr"]);
        assert!(list(&[], Some("ko_KR")).is_empty());
    }

    #[test]
    fn the_macs_list_applies_at_attach_and_when_it_changes() {
        let mut last = Vec::new();
        let ko = || strings(&["ko-KR", "en-US"]);
        // At boot, whatever Android had.
        assert_eq!(
            update(&mut last, ko(), true).as_deref(),
            Some("ko-KR,en-US")
        );
        // A notification without a change of the list (the region, or
        // another process's): a choice made in Android stays.
        assert_eq!(update(&mut last, ko(), false), None);
        // The Mac's list changed: it wins again.
        let ja = strings(&["ja-JP", "ko-KR"]);
        assert_eq!(
            update(&mut last, ja.clone(), false).as_deref(),
            Some("ja-JP,ko-KR")
        );
        assert_eq!(update(&mut last, ja.clone(), false), None);
        // A new system_server: the Mac's list again.
        assert_eq!(update(&mut last, ja, true).as_deref(), Some("ja-JP,ko-KR"));
        // Nothing Android can name: nothing changes, and the last list
        // stays what a later change is compared with.
        assert_eq!(update(&mut last, Vec::new(), true), None);
        assert_eq!(update(&mut last, strings(&["ja-JP", "ko-KR"]), false), None);
    }
}
