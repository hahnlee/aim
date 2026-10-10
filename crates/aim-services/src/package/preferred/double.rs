//! Pinned libcore FloatingDecimal binary-to-ASCII conversion. Big integers
//! implement only the nonnegative scaling and single-digit division it uses.
// Portions ported from AOSP libcore FloatingDecimal.java (android-16.0.0_r1).
// Copyright (C) 1996, 2013 Oracle and/or its affiliates.
// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0
use std::cmp::Ordering;

#[derive(Clone, PartialEq, Eq)]
struct Big(Vec<u32>);
impl Big {
    fn new(value: u64) -> Self {
        let mut v = Self(vec![value as u32, (value >> 32) as u32]);
        v.trim();
        v
    }
    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
    fn mul(&mut self, value: u32) {
        let mut carry = 0u64;
        for digit in &mut self.0 {
            let next = u64::from(*digit) * u64::from(value) + carry;
            *digit = next as u32;
            carry = next >> 32;
        }
        if carry != 0 {
            self.0.push(carry as u32);
        }
    }
    fn scaled(value: u64, five: i32, two: i32) -> Self {
        assert!(five >= 0 && two >= 0);
        let mut result = Self::new(value);
        for _ in 0..five {
            result.mul(5);
        }
        for _ in 0..two {
            result.mul(2);
        }
        result
    }
    fn add(&self, other: &Self) -> Self {
        let mut result = Vec::new();
        let mut carry = 0u64;
        for index in 0..self.0.len().max(other.0.len()) {
            let next = u64::from(*self.0.get(index).unwrap_or(&0))
                + u64::from(*other.0.get(index).unwrap_or(&0))
                + carry;
            result.push(next as u32);
            carry = next >> 32;
        }
        if carry != 0 {
            result.push(carry as u32);
        }
        Self(result)
    }
    fn sub(&mut self, other: &Self) {
        assert!(self.cmp(other) != Ordering::Less);
        let mut borrow = 0u64;
        for (index, digit) in self.0.iter_mut().enumerate() {
            let sub = u64::from(*other.0.get(index).unwrap_or(&0)) + borrow;
            let old = u64::from(*digit);
            *digit = old.wrapping_sub(sub) as u32;
            borrow = u64::from(old < sub);
        }
        self.trim();
    }
    fn digit(&mut self, denominator: &Self) -> u8 {
        let mut digit = 0;
        while self.cmp(denominator) != Ordering::Less {
            self.sub(denominator);
            digit += 1;
            assert!(digit <= 9);
        }
        self.mul(10);
        digit
    }
}

pub(super) fn format(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        }
        .into();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .into();
    }
    let bits = value.to_bits();
    let mut fraction = bits & ((1u64 << 52) - 1);
    let mut exponent = ((bits >> 52) & 2047) as i32;
    let significant;
    if exponent == 0 {
        let shift = fraction.leading_zeros() as i32 - 11;
        significant = 64 - fraction.leading_zeros() as i32;
        fraction <<= shift;
        exponent = 1 - shift;
    } else {
        fraction |= 1 << 52;
        significant = 53;
    }
    exponent -= 1023;
    let trailing = fraction.trailing_zeros() as i32;
    let fraction_bits = 53 - trailing;
    let tiny = (fraction_bits - exponent - 1).max(0);
    let (mut digits, mut decimal) = if (-21..=62).contains(&exponent) && tiny == 0 {
        let insignificant = if exponent > significant {
            let power = exponent - significant - 1;
            if power > 1 && power < 64 {
                ((1u64 << power) as f64).log10().floor() as u32
            } else {
                0
            }
        } else {
            0
        };
        let mut integer = if exponent >= 52 {
            fraction << (exponent - 52)
        } else {
            fraction >> (52 - exponent)
        };
        let power = 10u64.pow(insignificant);
        let residue = integer % power;
        integer /= power;
        if insignificant != 0 && residue >= power / 2 {
            integer += 1;
        }
        let text = integer.to_string();
        let decimal = text.len() as i32 + insignificant as i32;
        let digits = text
            .trim_end_matches('0')
            .bytes()
            .map(|v| v - b'0')
            .collect();
        (digits, decimal)
    } else {
        let normalized = f64::from_bits((1023u64 << 52) | (fraction & ((1 << 52) - 1)));
        let estimate = (normalized - 1.5) * 0.289529654
            + 0.176091259
            + f64::from(exponent) * 0.301029995663981;
        let mut decimal = estimate.floor() as i32;
        let b5 = (-decimal).max(0);
        let s5 = decimal.max(0);
        let mut b2 = b5 + tiny + exponent;
        let mut s2 = s5 + tiny;
        let mut m2 = b2 - significant;
        fraction >>= trailing;
        b2 -= fraction_bits - 1;
        let common = b2.min(s2);
        b2 -= common;
        s2 -= common;
        m2 -= common;
        if fraction_bits == 1 {
            m2 -= 1;
        }
        if m2 < 0 {
            b2 -= m2;
            s2 -= m2;
            m2 = 0;
        }
        let five_bits = |power: i32| -> i32 {
            const BITS: [i32; 27] = [
                0, 3, 5, 7, 10, 12, 14, 17, 19, 21, 24, 26, 28, 31, 33, 35, 38, 40, 42, 45, 47, 49,
                52, 54, 56, 59, 61,
            ];
            BITS.get(power as usize).copied().unwrap_or(power * 3)
        };
        let b_bits = fraction_bits + b2 + five_bits(b5);
        let ten_s_bits = s2 + 1 + five_bits(s5 + 1);
        let (mut digits, round) = if b_bits < 64 && ten_s_bits < 64 {
            let width = if b_bits < 32 && ten_s_bits < 32 {
                32
            } else {
                64
            };
            let (digits, round, adjusted) = primitive(fraction, b5, b2, s5, s2, m2, decimal, width);
            decimal = adjusted;
            (digits, round)
        } else {
            let denominator = Big::scaled(1, s5, s2);
            let mut numerator = Big::scaled(fraction, b5, b2);
            let mut margin = Big::scaled(1, b5 + 1, m2 + 1);
            let mut ten_denominator = denominator.clone();
            ten_denominator.mul(10);
            let mut digits = Vec::new();
            let digit = numerator.digit(&denominator);
            let mut low = numerator.cmp(&margin) == Ordering::Less;
            let mut high = numerator.add(&margin).cmp(&ten_denominator) != Ordering::Less;
            if digit == 0 && !high {
                decimal -= 1;
            } else {
                digits.push(digit);
            }
            if !(-3..8).contains(&decimal) {
                low = false;
                high = false;
            }
            while !low && !high {
                let digit = numerator.digit(&denominator);
                margin.mul(10);
                low = numerator.cmp(&margin) == Ordering::Less;
                high = numerator.add(&margin).cmp(&ten_denominator) != Ordering::Less;
                digits.push(digit);
            }
            let mut twice = numerator.clone();
            twice.mul(2);
            let round = high
                && (!low
                    || twice.cmp(&ten_denominator) == Ordering::Greater
                    || twice.cmp(&ten_denominator) == Ordering::Equal
                        && digits.last().is_some_and(|digit| digit & 1 != 0));
            (digits, round)
        };
        if round {
            let mut index = digits.len() - 1;
            while digits[index] == 9 && index > 0 {
                digits[index] = 0;
                index -= 1;
            }
            if digits[index] == 9 {
                digits[index] = 1;
                decimal += 1;
            } else {
                digits[index] += 1;
            }
        }
        (digits, decimal + 1)
    };
    let mut result = String::new();
    if value.is_sign_negative() {
        result.push('-');
    }
    if decimal > 0 && decimal < 8 {
        for index in 0..decimal as usize {
            result.push(char::from(b'0' + *digits.get(index).unwrap_or(&0)));
        }
        result.push('.');
        if digits.len() > decimal as usize {
            for digit in &digits[decimal as usize..] {
                result.push(char::from(b'0' + *digit));
            }
        } else {
            result.push('0');
        }
    } else if decimal <= 0 && decimal > -3 {
        result.push_str("0.");
        for _ in 0..-decimal {
            result.push('0');
        }
        for digit in &digits {
            result.push(char::from(b'0' + *digit));
        }
    } else {
        result.push(char::from(b'0' + digits.remove(0)));
        result.push('.');
        if digits.is_empty() {
            result.push('0');
        } else {
            for digit in digits {
                result.push(char::from(b'0' + digit));
            }
        }
        decimal -= 1;
        result.push('E');
        result.push_str(&decimal.to_string());
    }
    result
}

fn primitive(
    fraction: u64,
    b5: i32,
    b2: i32,
    s5: i32,
    s2: i32,
    m2: i32,
    mut decimal: i32,
    width: u32,
) -> (Vec<u8>, bool, i32) {
    let wrap = |value: i128| {
        if width == 32 {
            i128::from(value as i32)
        } else {
            i128::from(value as i64)
        }
    };
    let mut numerator = wrap((i128::from(fraction) * 5i128.pow(b5 as u32)) << b2);
    let denominator = wrap(5i128.pow(s5 as u32) << s2);
    let mut margin = wrap(5i128.pow(b5 as u32) << m2);
    let ten_denominator = wrap(denominator * 10);
    let mut digits = Vec::new();
    let digit = (numerator / denominator) as u8;
    numerator = wrap((numerator % denominator) * 10);
    margin = wrap(margin * 10);
    let mut low = numerator < margin;
    let mut high = wrap(numerator + margin) > ten_denominator;
    if digit == 0 && !high {
        decimal -= 1;
    } else {
        digits.push(digit);
    }
    if !(-3..8).contains(&decimal) {
        low = false;
        high = false;
    }
    while !low && !high {
        let digit = (numerator / denominator) as u8;
        numerator = wrap((numerator % denominator) * 10);
        margin = wrap(margin * 10);
        if margin > 0 {
            low = numerator < margin;
            high = wrap(numerator + margin) > ten_denominator;
        } else {
            low = true;
            high = true;
        }
        digits.push(digit);
    }
    let difference = wrap(wrap(numerator * 2) - ten_denominator);
    let round = high
        && (!low
            || difference > 0
            || difference == 0 && digits.last().is_some_and(|digit| digit & 1 != 0));
    (digits, round, decimal)
}

#[cfg(test)]
mod tests {
    use super::format;
    #[test]
    fn pinned_java_decimal_boundaries() {
        assert_eq!(format(1e23), "9.999999999999999E22");
        assert_eq!(format(f64::from_bits(1)), "4.9E-324");
        assert_eq!(format(f64::MIN_POSITIVE), "2.2250738585072014E-308");
        assert_eq!(format(f64::MAX), "1.7976931348623157E308");
        assert_eq!(format(-0.0), "-0.0");
        assert_eq!(format(1e7), "1.0E7");
        assert_eq!(format(0.001), "0.001");
    }
}
