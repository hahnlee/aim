//! The text syntax accepted by `Double.parseDouble` and `Float.parseFloat`.
pub(super) fn parse(text: &str) -> Option<f64> {
    let text = text.trim_matches(|c: char| c <= '\u{20}');
    let (negative, body) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let sign = if negative { -1.0 } else { 1.0 };
    match body {
        "NaN" => return Some(f64::NAN),
        "Infinity" => return Some(sign * f64::INFINITY),
        _ => {}
    }
    let body = body.strip_suffix(['d', 'D', 'f', 'F']).unwrap_or(body);
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        return hex_bits(hex, 52, 1023, -1022).map(|v| sign * f64::from_bits(v));
    }
    if !decimal(body) {
        return None;
    }
    body.parse::<f64>().ok().map(|v| sign * v)
}

pub(super) fn parse_float(text: &str) -> Option<f32> {
    let text = text.trim_matches(|c: char| c <= '\u{20}');
    let (negative, body) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let sign = if negative { -1.0 } else { 1.0 };
    match body {
        "NaN" => return Some(f32::NAN),
        "Infinity" => return Some(sign * f32::INFINITY),
        _ => {}
    }
    let body = body.strip_suffix(['d', 'D', 'f', 'F']).unwrap_or(body);
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        return hex_bits(hex, 23, 127, -126).map(|v| sign * f32::from_bits(v as u32));
    }
    if !decimal(body) {
        return None;
    }
    body.parse::<f32>().ok().map(|v| sign * v)
}

fn decimal(body: &str) -> bool {
    let (mantissa, exponent) = split_exponent(body, ['e', 'E']);
    let mut digits = 0;
    let mut dot = false;
    for c in mantissa.bytes() {
        match c {
            b'0'..=b'9' => digits += 1,
            b'.' if !dot => dot = true,
            _ => return false,
        }
    }
    if digits == 0 || exponent.is_some_and(|e| exponent_value(e).is_none()) {
        return false;
    }
    true
}

fn split_exponent<const N: usize>(s: &str, markers: [char; N]) -> (&str, Option<&str>) {
    match s.find(markers) {
        Some(at) => (&s[..at], Some(&s[at + 1..])),
        None => (s, None),
    }
}

fn exponent_value(s: &str) -> Option<i64> {
    let (negative, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = digits.bytes().fold(0i64, |v, b| {
        v.saturating_mul(10).saturating_add((b - b'0') as i64)
    });
    Some(if negative { -value } else { value })
}

fn hex_bits(hex: &str, fraction: usize, max: i64, min: i64) -> Option<u64> {
    let (mantissa, exponent) = split_exponent(hex, ['p', 'P']);
    let exponent = exponent_value(exponent?)?;
    let mut bits = Vec::new();
    let mut dot = false;
    let mut integer_digits = 0i64;
    for c in mantissa.chars() {
        if c == '.' && !dot {
            dot = true;
            continue;
        }
        let digit = c.to_digit(16)?;
        if !c.is_ascii() {
            return None;
        }
        if !dot {
            integer_digits += 1;
        }
        bits.extend((0..4).rev().map(|bit| digit & (1 << bit) != 0));
    }
    if bits.is_empty() {
        return None;
    }
    let Some(first) = bits.iter().position(|bit| *bit) else {
        return Some(0);
    };
    let mut top = (integer_digits * 4 - first as i64 - 1).saturating_add(exponent);
    let infinity = ((max * 2 + 1) as u64) << fraction;
    if top > max {
        return Some(infinity);
    }
    let smallest = min - fraction as i64;
    if top < smallest - 1 {
        return Some(0);
    }
    let count = if top >= min {
        fraction + 1
    } else {
        (top - smallest + 1) as usize
    };
    let bit = |at: usize| bits.get(first + at).copied().unwrap_or(false);
    let mut significand = (0..count).fold(0u64, |v, at| (v << 1) | u64::from(bit(at)));
    let sticky = bits.iter().skip(first + count + 1).any(|bit| *bit);
    if bit(count) && (sticky || significand & 1 != 0) {
        significand += 1;
    }
    if top < min {
        return Some(significand);
    }
    if significand == 1 << (fraction + 1) {
        significand >>= 1;
        top += 1;
    }
    if top > max {
        return Some(infinity);
    }
    Some(((top + max) as u64) << fraction | (significand & ((1 << fraction) - 1)))
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_float};
    #[test]
    fn float_rounds_directly_to_single_precision() {
        for (text, bits) in [
            ("0x1.0000010000000001p0", 1f32.to_bits() + 1),
            ("0x1.000001p0", 1f32.to_bits()),
            ("0x1.000003p0", 1f32.to_bits() + 2),
            ("0x1p-149", 1),
            ("0x1p-150", 0),
            ("-0.0f", (-0f32).to_bits()),
            ("0x1.fffffep127", f32::MAX.to_bits()),
        ] {
            assert_eq!(parse_float(text).unwrap().to_bits(), bits, "{text}");
        }
    }
    #[test]
    fn java_syntax_and_rounding() {
        for (s, bits) in [
            ("  -0.0D\t", (-0.0f64).to_bits()),
            ("0x1.fffffffffffffp1023", f64::MAX.to_bits()),
            ("0x1p-1074", 1),
            ("0x1p-1075", 0),
            ("0x1.0000000000001p-1075", 1),
            ("0x1.00000000000008p0", 1f64.to_bits()),
            ("0x1.00000000000018p0", 1f64.to_bits() + 2),
            ("0x1p99999999999999999999", f64::INFINITY.to_bits()),
        ] {
            assert_eq!(parse(s).unwrap().to_bits(), bits, "{s}");
        }
        for s in [
            "inf",
            "nan",
            "InfinityD",
            "0x1",
            "0x.p1",
            "1e",
            "1e2e3",
            "1_0",
            "\u{a0}1",
        ] {
            assert!(parse(s).is_none(), "{s}");
        }
    }
}
