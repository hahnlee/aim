//! `android.content.Intent` of an intent URI (`Intent.parseUri` at
//! `android-16.0.0_r1`, its `#Intent;…;end` format and the plain data
//! one), and an intent written as `Intent.writeToParcel` writes it.

use aim_binder_host::parcel::Parcel;

use super::intent::{ComponentName, Intent};
use super::intent_filter::ACTION_VIEW;
use super::uri::{Uri, decode};
use crate::bundle::Value;

/// `Intent.URI_INTENT_SCHEME`, `URI_ANDROID_APP_SCHEME`,
/// `URI_ALLOW_UNSAFE`.
pub const URI_INTENT_SCHEME: i32 = 1 << 0;
pub const URI_ANDROID_APP_SCHEME: i32 = 1 << 1;
pub const URI_ALLOW_UNSAFE: i32 = 1 << 2;
/// `Intent.IMMUTABLE_FLAGS`: the URI permission grants.
const IMMUTABLE_FLAGS: i32 = 0x1 | 0x2 | 0x40 | 0x80;
/// `UserHandle.USER_CURRENT`, an intent's content user hint by default.
const USER_CURRENT: i32 = -2;

/// An intent with the parts `Intent` (the resolution model) leaves out:
/// its source bounds and its extras.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FullIntent {
    pub intent: Intent,
    pub source_bounds: Option<[i32; 4]>,
    /// `mExtras`, in their `ArrayMap` order; none when null.
    pub extras: Option<Vec<(String, Value)>>,
}

/// `Integer.decode`: a sign, then `0x`, `0X` or `#` for hex, `0` for
/// octal, else decimal.
fn decode_integer(s: &str) -> Option<i32> {
    let (negative, rest) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if let Some(h) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")).or_else(|| rest.strip_prefix('#')) {
        (16, h)
    } else if rest.len() > 1 && rest.starts_with('0') {
        (8, &rest[1..])
    } else {
        (10, rest)
    };
    if digits.is_empty() || digits.starts_with(['-', '+']) {
        return None;
    }
    let magnitude = i64::from_str_radix(digits, radix).ok()?;
    i32::try_from(if negative { -magnitude } else { magnitude }).ok()
}

/// `Uri.decodeIfNeeded`.
fn decode_if_needed(s: &str) -> String {
    if s.contains('%') { decode(s) } else { s.to_owned() }
}

impl FullIntent {
    fn view(data: Option<Uri>) -> FullIntent {
        FullIntent { intent: Intent { action: Some(ACTION_VIEW.into()), data, ..Intent::default() }, ..FullIntent::default() }
    }

    /// `Intent.parseUri(uri, flags)`: an error as its
    /// `URISyntaxException`'s message, or for the old format
    /// (`getIntentOld`), which is not modelled.
    pub fn parse_uri(uri: &str, flags: i32) -> Result<FullIntent, String> {
        let android_app = uri.starts_with("android-app:");
        if flags & (URI_INTENT_SCHEME | URI_ANDROID_APP_SCHEME) != 0 && !uri.starts_with("intent:") && !android_app {
            return Ok(FullIntent::view(Some(Uri::parse(uri))));
        }
        let mut i = uri.rfind('#').map(|i| i as isize).unwrap_or(-1);
        if i == -1 {
            if !android_app {
                return Ok(FullIntent::view(Some(Uri::parse(uri))));
            }
        } else if !uri[i as usize..].starts_with("#Intent;") {
            if !android_app {
                return Err(format!("{uri}: an intent URI of the old format"));
            }
            i = -1;
        }
        let mut base = FullIntent::view(None);
        let mut selector: Option<FullIntent> = None;
        let mut explicit_action = false;
        let mut scheme: Option<String> = None;
        let mut data = if i >= 0 {
            let d = uri[..i as usize].to_owned();
            i += 8;
            d
        } else {
            uri.to_owned()
        };
        let syntax = |what: &str| format!("{what}: {uri}");
        while i >= 0 && !uri[i as usize..].starts_with("end") {
            let at = i as usize;
            let rest = &uri[at..];
            let eq = uri[at..].find('=').map_or(at as isize - 1, |e| (at + e) as isize);
            let semi = uri[at..].find(';').map(|s| at + s).ok_or_else(|| syntax("uri end not found"))?;
            let value = if eq < semi as isize { decode(&uri[eq as usize + 1..semi]) } else { String::new() };
            let in_selector = selector.is_some();
            let intent = selector.as_mut().unwrap_or(&mut base);
            if rest.starts_with("action=") {
                intent.intent.action = Some(value);
                explicit_action |= !in_selector;
            } else if rest.starts_with("category=") {
                let categories = intent.intent.categories.get_or_insert_with(Vec::new);
                if !categories.contains(&value) {
                    categories.push(value);
                }
            } else if rest.starts_with("type=") {
                intent.intent.ty = Some(value);
            } else if rest.starts_with("identifier=") {
                intent.intent.identifier = Some(value);
            } else if rest.starts_with("launchFlags=") {
                let mut f = decode_integer(&value).ok_or_else(|| syntax("NumberFormatException"))?;
                if flags & URI_ALLOW_UNSAFE == 0 {
                    f &= !IMMUTABLE_FLAGS;
                }
                intent.intent.flags = f;
            } else if rest.starts_with("extendedLaunchFlags=") {
                intent.intent.extended_flags = decode_integer(&value).ok_or_else(|| syntax("NumberFormatException"))?;
            } else if rest.starts_with("package=") {
                intent.intent.package = Some(value);
            } else if rest.starts_with("component=") {
                intent.intent.component = super::preferred::unflatten(&value);
            } else if rest.starts_with("scheme=") {
                if in_selector {
                    intent.intent.data = Some(Uri::parse(&format!("{value}:")));
                } else {
                    scheme = Some(value);
                }
            } else if rest.starts_with("sourceBounds=") {
                // `Rect.unflattenFromString`.
                let parts: Vec<i32> = value.split(' ').filter_map(|v| v.parse().ok()).collect();
                intent.source_bounds = <[i32; 4]>::try_from(parts.as_slice()).ok();
            } else if semi == at + 3 && rest.starts_with("SEL") {
                selector = Some(FullIntent::default());
            } else {
                if eq < at as isize + 2 {
                    return Err(syntax("illegal Intent URI format"));
                }
                let key = decode(&uri[at + 2..eq as usize]);
                let parsed = match &rest[..2] {
                    "S." => Some(Value::String(Some(value))),
                    "B." => Some(Value::Bool(value.eq_ignore_ascii_case("true"))),
                    "i." => value.parse().ok().map(Value::Int),
                    "l." => value.parse().ok().map(Value::Long),
                    "f." => value.parse().ok().map(Value::Float),
                    "b." | "c." | "d." | "s." => return Err(syntax("an extra of a type not modelled")),
                    _ => return Err(syntax("unknown EXTRA type")),
                };
                let value = parsed.ok_or_else(|| syntax("NumberFormatException"))?;
                let extras = intent.extras.get_or_insert_with(Vec::new);
                extras.retain(|(k, _)| *k != key);
                extras.push((key, value));
            }
            i = semi as isize + 1;
        }
        let mut intent = match selector {
            Some(sel) => {
                if base.intent.package.is_none() {
                    base.intent.selector = Some(Box::new(sel.intent));
                }
                base
            }
            None => base,
        };
        if data.starts_with("intent:") {
            data = data[7..].to_owned();
            if let Some(scheme) = &scheme {
                data = format!("{scheme}:{data}");
            }
        } else if data.starts_with("android-app:") {
            let bytes = data.as_bytes();
            if bytes.get(12) == Some(&b'/') && bytes.get(13) == Some(&b'/') {
                match data[14..].find('/').map(|e| e + 14) {
                    None => {
                        intent.intent.package = Some(decode_if_needed(&data[14..]));
                        if !explicit_action {
                            intent.intent.action = Some("android.intent.action.MAIN".into());
                        }
                        data = String::new();
                    }
                    Some(mut end) => {
                        let mut authority = None;
                        let mut scheme = None;
                        intent.intent.package = Some(decode_if_needed(&data[14..end]));
                        if end + 1 < data.len() {
                            match data[end + 1..].find('/').map(|e| e + end + 1) {
                                Some(new_end) => {
                                    scheme = Some(decode_if_needed(&data[end + 1..new_end]));
                                    end = new_end;
                                    if end < data.len()
                                        && let Some(new_end) = data[end + 1..].find('/').map(|e| e + end + 1)
                                    {
                                        authority = Some(decode_if_needed(&data[end + 1..new_end]));
                                        end = new_end;
                                    }
                                }
                                None => scheme = Some(decode_if_needed(&data[end + 1..])),
                            }
                        }
                        data = match (scheme, authority) {
                            (None, _) => {
                                if !explicit_action {
                                    intent.intent.action = Some("android.intent.action.MAIN".into());
                                }
                                String::new()
                            }
                            (Some(s), None) => format!("{s}:"),
                            (Some(s), Some(a)) => format!("{s}://{a}{}", &data[end..]),
                        };
                    }
                }
            } else {
                data = String::new();
            }
        }
        if !data.is_empty() {
            intent.intent.data = Some(Uri::parse(&data));
        }
        Ok(intent)
    }

    /// `Intent.writeToParcel`, with no clip data, original intent or
    /// creator token.
    pub fn write(&self, p: &mut Parcel) {
        write_intent(p, &self.intent, self.source_bounds, self.extras.as_deref());
    }
}

fn write_intent(p: &mut Parcel, i: &Intent, source_bounds: Option<[i32; 4]>, extras: Option<&[(String, Value)]>) {
    p.write_string8(i.action.as_deref());
    // `Uri.writeToParcel(out, uri)`: `NULL_TYPE_ID` for none.
    match &i.data {
        Some(uri) => uri.write(p),
        None => p.write_i32(0),
    }
    p.write_string8(i.ty.as_deref());
    p.write_string8(i.identifier.as_deref());
    p.write_i32(i.flags);
    p.write_i32(i.extended_flags);
    p.write_string8(i.package.as_deref());
    match &i.component {
        Some(ComponentName { package, class }) => {
            p.write_string16(Some(package));
            p.write_string16(Some(class));
        }
        None => p.write_string16(None),
    }
    match source_bounds {
        Some(r) => {
            p.write_i32(1);
            for v in r {
                p.write_i32(v);
            }
        }
        None => p.write_i32(0),
    }
    // An `ArraySet`: by hash.
    let categories = i.categories.clone().map(|c| crate::package::info::array_order(c, |s| s.as_str())).unwrap_or_default();
    p.write_i32(categories.len() as i32);
    for c in &categories {
        p.write_string8(Some(c));
    }
    match &i.selector {
        Some(sel) => {
            p.write_i32(1);
            write_intent(p, sel, None, None);
        }
        None => p.write_i32(0),
    }
    p.write_i32(0); // no clip data
    p.write_i32(USER_CURRENT);
    // `writeBundle`: -1 for none.
    match extras {
        Some(extras) => {
            let entries: Vec<(&str, Value)> = extras.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            crate::bundle::write(p, &entries);
        }
        None => p.write_i32(-1),
    }
    p.write_i32(0); // no original intent
    p.write_i32(0); // no creator token
}

#[cfg(test)]
mod tests {
    use aim_binder_host::parcel::Reader;

    use super::*;
    use crate::package::intent_filter::Plain;

    /// A shortcut's intent: data, then its action, flags and component.
    #[test]
    fn parses_the_intent_format() {
        let i = FullIntent::parse_uri(
            "content://com.android.contacts/contacts#Intent;action=android.intent.action.INSERT;launchFlags=0x1000c000;component=com.google.android.contacts/com.google.android.apps.contacts.editor.EditorActivity;end",
            0,
        )
        .unwrap();
        assert_eq!(i.intent.action.as_deref(), Some("android.intent.action.INSERT"));
        assert_eq!(i.intent.data.as_ref().map(|d| d.as_str()), Some("content://com.android.contacts/contacts"));
        assert_eq!(i.intent.flags, 0x1000c000);
        assert_eq!(i.intent.component.as_ref().map(|c| c.class.as_str()), Some("com.google.android.apps.contacts.editor.EditorActivity"));
        // Read back as `Intent(Parcel)` reads it.
        let mut p = Parcel::new();
        i.write(&mut p);
        let back = Intent::read(&mut Reader::new(p.data(), &[]), &mut Plain).unwrap();
        assert_eq!(back, i.intent);
    }

    /// The URI permission flags are dropped unless allowed; a relative
    /// component, a selector, extras and a plain URI.
    #[test]
    fn parses_flags_selectors_extras_and_plain_uris() {
        let i = FullIntent::parse_uri("#Intent;launchFlags=0x10000043;component=p/.A;S.k=v%3B;i.n=7;SEL;action=s;end", 0).unwrap();
        assert_eq!(i.intent.flags, 0x10000000);
        assert_eq!(i.intent.component, Some(ComponentName { package: "p".into(), class: "p.A".into() }));
        assert_eq!(i.intent.action.as_deref(), Some(ACTION_VIEW));
        assert_eq!(i.intent.selector.as_ref().and_then(|s| s.action.as_deref()), Some("s"));
        assert_eq!(i.extras, Some(vec![("k".into(), Value::String(Some("v;".into()))), ("n".into(), Value::Int(7))]));
        let plain = FullIntent::parse_uri("http://example.com", 0).unwrap();
        assert_eq!((plain.intent.action.as_deref(), plain.intent.data.as_ref().map(|d| d.as_str())), (Some(ACTION_VIEW), Some("http://example.com")));
        assert!(FullIntent::parse_uri("#Intent;action=a", 0).is_err());
    }
}
