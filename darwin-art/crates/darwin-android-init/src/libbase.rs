//! The libbase parsing helpers init relies on (android-base/parseint.h,
//! parsedouble.h, parsebool.cpp, strings.cpp), with their exact acceptance
//! rules.

/// C `isspace` in the "C" locale.
pub(crate) fn is_c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn skip_c_space(s: &str) -> &str {
    let skip = s.bytes().take_while(|b| is_c_space(*b)).count();
    &s[skip..]
}

/// `strtoull`/`strtoll` digits for `base` (10 or 16, with an optional `0x`
/// prefix in base 16). Returns the magnitude and whether the whole input
/// was consumed; `None` when nothing was parsed or on overflow.
fn scan_digits(s: &str, base: u32) -> Option<(u128, bool)> {
    let mut digits = s;
    if base == 16
        && let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        && rest.bytes().next().is_some_and(|b| b.is_ascii_hexdigit())
    {
        digits = rest;
    }
    let count = digits
        .bytes()
        .take_while(|b| (*b as char).is_digit(base))
        .count();
    if count == 0 {
        return None;
    }
    let mut value: u128 = 0;
    for byte in digits[..count].bytes() {
        value = value
            .checked_mul(base as u128)?
            .checked_add((byte as char).to_digit(base)? as u128)?;
        if value > u64::MAX as u128 * 2 {
            // Far past any target type: the C call reports ERANGE.
            return None;
        }
    }
    Some((value, count == digits.len()))
}

/// `android::base::ParseInt(s, &out, min, max)` for a 64-bit target.
pub(crate) fn parse_int(s: &str, min: i64, max: i64) -> Option<i64> {
    let s = skip_c_space(s);
    let base = if s.starts_with("0x") || s.starts_with("0X") {
        16
    } else {
        10
    };
    let body = skip_c_space(s);
    let (negative, body) = match body.as_bytes().first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };
    let (magnitude, complete) = scan_digits(body, base)?;
    if !complete {
        return None;
    }
    let value: i128 = if negative {
        -(magnitude as i128)
    } else {
        magnitude as i128
    };
    if value < i64::MIN as i128 || value > i64::MAX as i128 {
        return None;
    }
    let value = value as i64;
    (min..=max).contains(&value).then_some(value)
}

/// `android::base::ParseUint(s, &out, max)` (no size suffixes).
pub(crate) fn parse_uint(s: &str, max: u64) -> Option<u64> {
    let s = skip_c_space(s);
    if s.starts_with('-') {
        return None;
    }
    let base = if s.starts_with("0x") || s.starts_with("0X") {
        16
    } else {
        10
    };
    let body = s.strip_prefix('+').unwrap_or(s);
    let (value, complete) = scan_digits(body, base)?;
    if !complete || value > u64::MAX as u128 {
        return None;
    }
    let value = value as u64;
    (value <= max).then_some(value)
}

/// `android::base::ParseDouble(s, &out)`: the whole string must be a
/// `strtod` number and must not overflow.
pub(crate) fn parse_double(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let body = skip_c_space(s);
    let (sign, unsigned) = match body.as_bytes().first() {
        Some(b'-') => (-1.0, &body[1..]),
        Some(b'+') => (1.0, &body[1..]),
        _ => (1.0, body),
    };
    let lower = unsigned.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if lower == "nan" || (lower.starts_with("nan(") && lower.ends_with(')')) {
        return Some(f64::NAN);
    }
    if let Some(hex) = lower.strip_prefix("0x") {
        return parse_hex_float(hex).map(|v| sign * v);
    }
    let valid = !unsigned.is_empty()
        && unsigned
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    if !valid {
        return None;
    }
    let value: f64 = unsigned.parse().ok()?;
    if value.is_infinite() {
        return None;
    }
    Some(sign * value)
}

fn parse_hex_float(hex: &str) -> Option<f64> {
    let (mantissa, exponent) = match hex.split_once('p') {
        Some((m, e)) => (m, e.parse::<i32>().ok()?),
        None => (hex, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let mut value = 0f64;
    for c in int_part.chars() {
        value = value * 16.0 + c.to_digit(16)? as f64;
    }
    let mut scale = 1.0 / 16.0;
    for c in frac_part.chars() {
        value += c.to_digit(16)? as f64 * scale;
        scale /= 16.0;
    }
    let value = value * 2f64.powi(exponent);
    (!value.is_infinite()).then_some(value)
}

/// `android::base::ParseBool`.
pub(crate) fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "y" | "yes" | "on" | "true" => Some(true),
        "0" | "n" | "no" | "off" | "false" => Some(false),
        _ => None,
    }
}

/// `android::base::Split(s, delimiters)` for a single delimiter byte.
pub(crate) fn split(s: &str, delimiter: char) -> Vec<String> {
    s.split(delimiter).map(str::to_string).collect()
}

/// `android::base::Trim`.
pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && is_c_space(c as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_matches_libbase() {
        assert_eq!(parse_int("12", i64::MIN, i64::MAX), Some(12));
        assert_eq!(parse_int(" -3", i64::MIN, i64::MAX), Some(-3));
        assert_eq!(parse_int("0x10", i64::MIN, i64::MAX), Some(16));
        assert_eq!(parse_int("010", i64::MIN, i64::MAX), Some(10));
        assert_eq!(parse_int("-0x10", i64::MIN, i64::MAX), None);
        assert_eq!(parse_int("1a", i64::MIN, i64::MAX), None);
        assert_eq!(parse_int("", i64::MIN, i64::MAX), None);
        assert_eq!(parse_int("8", 0, 7), None);
        assert_eq!(parse_uint("-1", u64::MAX), None);
        assert_eq!(parse_uint("0xff", u64::MAX), Some(255));
        assert_eq!(parse_uint("18446744073709551616", u64::MAX), None);
        assert_eq!(parse_double("1.5"), Some(1.5));
        assert_eq!(parse_double("1e999"), None);
        assert_eq!(parse_double("0x1p4"), Some(16.0));
        assert_eq!(parse_double("abc"), None);
    }
}
