//! System properties: the serialized context trie, bionic's property areas,
//! init's property service rules, the boot-time `.prop` loading and the
//! `property_service` socket protocol.

pub mod area;
pub mod areas;
pub mod info;
pub mod load;
pub mod protocol;
pub mod service;

pub use area::{PA_SIZE, PROP_VALUE_MAX, PropArea, PropAreaReader, ReadProperty};
pub use areas::{FutexWaker, NoWake, PropertyAreas, WakeTarget};
pub use info::{PropertyInfoArea, PropertyInfoEntry, build_trie, parse_property_info_file};
pub use service::{PropertyPolicy, PropertyService, SetEffect, SetOutcome, Ucred};

use crate::libbase::{parse_double, parse_int, parse_uint};

/// `IsLegalPropertyName` (system/core/init/util.cpp): non-empty, no leading,
/// trailing or doubled `.`, and only `[A-Za-z0-9_.@:-]`.
pub fn is_legal_property_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes[0] == b'.' || bytes[bytes.len() - 1] == b'.' {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'.' {
            if bytes[index - 1] == b'.' {
                return false;
            }
            continue;
        }
        if matches!(byte, b'_' | b'-' | b'@' | b':') || byte.is_ascii_alphanumeric() {
            continue;
        }
        return false;
    }
    true
}

/// `IsLegalPropertyValue`: values of `PROP_VALUE_MAX` bytes or more only for
/// `ro.*`, and UTF-8 (bionic's `mbstowcs` is UTF-8, and stops at a NUL).
pub fn is_legal_property_value(name: &str, value: &[u8]) -> Result<(), String> {
    if value.len() >= PROP_VALUE_MAX && !name.starts_with("ro.") {
        return Err("Property value too long".to_string());
    }
    let until_nul = &value[..value.iter().position(|b| *b == 0).unwrap_or(value.len())];
    if std::str::from_utf8(until_nul).is_err() {
        return Err("Value is not a UTF8 encoded string".to_string());
    }
    Ok(())
}

/// `CheckType` (system/core/init/property_type.cpp).
pub fn check_type(type_string: &str, value: &str) -> bool {
    // Always allow clearing a property so the default takes over.
    if value.is_empty() {
        return true;
    }
    let type_strings: Vec<&str> = type_string.split(' ').collect();
    let kind = type_strings[0];
    match kind {
        "string" => true,
        "bool" => matches!(value, "true" | "false" | "1" | "0"),
        "int" => parse_int(value, i64::MIN, i64::MAX).is_some(),
        "uint" => !value.starts_with('-') && parse_uint(value, u64::MAX).is_some(),
        "double" => parse_double(value).is_some(),
        "size" => {
            let digits = value.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0
                && digits + 1 == value.len()
                && matches!(value.as_bytes()[digits], b'g' | b'k' | b'm')
        }
        "enum" => type_strings[1..].contains(&value),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_names() {
        assert!(is_legal_property_name("ro.build.id"));
        assert!(is_legal_property_name("a-b_c@d:e"));
        assert!(!is_legal_property_name(""));
        assert!(!is_legal_property_name(".a"));
        assert!(!is_legal_property_name("a."));
        assert!(!is_legal_property_name("a..b"));
        assert!(!is_legal_property_name("a b"));
        assert!(!is_legal_property_name("a/b"));
    }

    // Cases from system/core/init/property_type_test.cpp.
    #[test]
    fn types() {
        for value in [
            "",
            "-234",
            "234",
            "true",
            "false",
            "45645634563456345634563456",
            "some other string",
        ] {
            assert!(check_type("string", value));
        }
        for (value, ok) in [
            ("", true),
            ("abc", false),
            ("-abc", false),
            ("0", true),
            ("123", true),
            ("-123", true),
        ] {
            assert_eq!(check_type("int", value), ok, "int {value}");
        }
        assert!(check_type("int", &i64::MIN.to_string()));
        assert!(check_type("int", &i64::MAX.to_string()));
        for (value, ok) in [
            ("", true),
            ("abc", false),
            ("-abc", false),
            ("0", true),
            ("123", true),
            ("-123", false),
        ] {
            assert_eq!(check_type("uint", value), ok, "uint {value}");
        }
        assert!(check_type("uint", &u64::MAX.to_string()));
        for (value, ok) in [
            ("", true),
            ("abc", false),
            ("-abc", false),
            ("0.0", true),
            ("123.1", true),
            ("-123.1", true),
        ] {
            assert_eq!(check_type("double", value), ok, "double {value}");
        }
        // std::to_string(numeric_limits<double>::min() / max()).
        assert!(check_type("double", "0.000000"));
        assert!(check_type("double", &format!("{:.6}", f64::MAX)));
        for (value, ok) in [
            ("", true),
            ("ab", false),
            ("abcd", false),
            ("0", false),
            ("512g", true),
            ("512k", true),
            ("512m", true),
            ("512gggg", false),
            ("512mgk", false),
            ("g", false),
            ("m", false),
        ] {
            assert_eq!(check_type("size", value), ok, "size {value}");
        }
        for (type_, value, ok) in [
            ("enum abc", "", true),
            ("enum abc", "ab", false),
            ("enum abc", "abcd", false),
            ("enum 123 456 789", "0", false),
            ("enum abc", "abc", true),
            ("enum 123 456 789", "123", true),
            ("enum 123 456 789", "456", true),
            ("enum 123 456 789", "789", true),
        ] {
            assert_eq!(check_type(type_, value), ok, "{type_} {value}");
        }
    }
}
