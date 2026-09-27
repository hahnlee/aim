//! Port of `ExpandProps` (system/core/init/util.cpp).

use crate::PropertyLookup;

/// `__ANDROID_API_P__`, `__ANDROID_API_Q__`, `__ANDROID_API_R__`.
pub const ANDROID_API_P: u32 = 28;
pub const ANDROID_API_Q: u32 = 29;
pub const ANDROID_API_R: u32 = 30;

/// Expands `${name}` and `${name:-default}` against `properties`, as init
/// does for command arguments, `import` paths and some service options.
///
/// - `$$` is a literal `$`; a trailing `$` is dropped.
/// - A property that is unset or empty takes the default; with no default it
///   is an error ("doesn't exist while expanding").
/// - Nested expansion is not supported (`${a.${b}}` reads the name `a.${b`).
/// - `$name` without braces takes the rest of the string as the name. It is
///   an error when the vendor image is R or newer (`vendor_api_level`), and
///   only logged before that.
pub fn expand_props(
    src: &str,
    properties: &dyn PropertyLookup,
    vendor_api_level: u32,
) -> Result<String, String> {
    let mut dst = String::new();
    let mut rest = src;
    loop {
        let Some(dollar) = rest.find('$') else {
            dst.push_str(rest);
            return Ok(dst);
        };
        dst.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];

        if let Some(stripped) = after.strip_prefix('$') {
            dst.push('$');
            rest = stripped;
            continue;
        }
        if after.is_empty() {
            return Ok(dst);
        }

        let mut prop_name;
        let mut default_value = String::new();
        let remaining;
        if let Some(braced) = after.strip_prefix('{') {
            let Some(end) = braced.find('}') else {
                return Err(format!(
                    "unexpected end of string in '{src}', looking for }}"
                ));
            };
            prop_name = braced[..end].to_string();
            remaining = &braced[end + 1..];
            if let Some(def) = prop_name.find(":-") {
                default_value = prop_name[def + 2..].to_string();
                prop_name.truncate(def);
            }
        } else {
            prop_name = after.to_string();
            if vendor_api_level >= ANDROID_API_R {
                return Err(format!(
                    "using deprecated syntax for specifying property '{after}', use ${{name}} instead"
                ));
            }
            remaining = "";
        }

        if prop_name.is_empty() {
            return Err(format!("invalid zero-length property name in '{src}'"));
        }

        let mut value = properties.property_or(&prop_name, "");
        if value.is_empty() {
            if default_value.is_empty() {
                return Err(format!(
                    "property '{prop_name}' doesn't exist while expanding '{src}'"
                ));
            }
            value = default_value;
        }
        dst.push_str(&value);
        rest = remaining;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn props() -> BTreeMap<String, String> {
        [
            ("ro.hardware", "ranchu"),
            ("ro.empty", ""),
            ("ro.zygote", "zygote64"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn expands_like_init() {
        let p = props();
        let e = |s| expand_props(s, &p, 36);
        assert_eq!(e("/init.${ro.hardware}.rc").unwrap(), "/init.ranchu.rc");
        assert_eq!(e("${ro.missing:-dflt}").unwrap(), "dflt");
        assert_eq!(e("${ro.empty:-dflt}").unwrap(), "dflt");
        assert_eq!(e("a$$b").unwrap(), "a$b");
        assert_eq!(e("trailing$").unwrap(), "trailing");
        assert!(e("${ro.missing}").unwrap_err().contains("doesn't exist"));
        assert!(e("${ro.empty}").is_err());
        assert!(e("${}").unwrap_err().contains("zero-length"));
        assert!(e("${unterminated").unwrap_err().contains("looking for }"));
        assert!(e("$ro.hardware").unwrap_err().contains("deprecated"));
        assert_eq!(
            expand_props("$ro.hardware", &p, ANDROID_API_Q).unwrap(),
            "ranchu"
        );
        assert_eq!(e("${ro.zygote}-${ro.hardware}").unwrap(), "zygote64-ranchu");
    }
}
