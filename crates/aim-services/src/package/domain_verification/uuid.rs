/*
 * Copyright (c) 2003, 2020, Oracle and/or its affiliates. All rights reserved.
 * Copyright (c) 1994, 2023, Oracle and/or its affiliates. All rights reserved.
 * DO NOT ALTER OR REMOVE COPYRIGHT NOTICES OR THIS FILE HEADER.
 *
 * This code is free software; you can redistribute it and/or modify it
 * under the terms of the GNU General Public License version 2 only, as
 * published by the Free Software Foundation.  Oracle designates this
 * particular file as subject to the "Classpath" exception as provided
 * by Oracle in the LICENSE file that accompanied this code.
 *
 * This code is distributed in the hope that it will be useful, but WITHOUT
 * ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
 * FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public License
 * version 2 for more details (a copy is included in the LICENSE file that
 * accompanied this code).
 *
 * You should have received a copy of the GNU General Public License version
 * 2 along with this work; if not, write to the Free Software Foundation,
 * Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301 USA.
 *
 * Please contact Oracle, 500 Oracle Parkway, Redwood Shores, CA 94065 USA
 * or visit www.oracle.com if you need additional information or have any
 * questions.
 */

//! UUID.fromStringCurrentJava/fromStringJava8 and Long hexadecimal parsing,
//! libcore android-16.0.0_r1. Unicode decimal starts derive from pinned ICU
//! UnicodeData.txt (Unicode-3.0; provenance in domain_verification/names.rs).

pub fn parse(name: &str, strict: bool) -> Result<String, String> {
    let invalid = || format!("Invalid UUID string: {name}");
    if strict && name.encode_utf16().count() > 36 {
        return Err("UUID string too large".into());
    }
    let mut parts = name.split('-').collect::<Vec<_>>();
    if !strict {
        while parts.last() == Some(&"") {
            parts.pop();
        }
    }
    if parts.len() != 5 {
        return Err(invalid());
    }
    let mut values = [0i64; 5];
    for (at, part) in parts.into_iter().enumerate() {
        values[at] = hexadecimal(part, strict)?;
    }
    let mask = |value: i64, bits: u32| {
        if strict {
            value & ((1i64 << bits) - 1)
        } else {
            value
        }
    };
    let msb = mask(values[0], 32).wrapping_shl(16) | mask(values[1], 16);
    let msb = msb.wrapping_shl(16) | mask(values[2], 16);
    let lsb = mask(values[3], 16).wrapping_shl(48) | mask(values[4], 48);
    Ok(format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (msb as u64 >> 32) as u32,
        (msb as u64 >> 16) as u16,
        msb as u16,
        (lsb as u64 >> 48) as u16,
        lsb as u64 & 0xffffffffffff
    ))
}

fn hexadecimal(part: &str, strict: bool) -> Result<i64, String> {
    if !strict && (part.starts_with('+') || part.starts_with('-')) {
        return Err("Sign character in wrong position".into());
    }
    let units = part.encode_utf16().collect::<Vec<_>>();
    let error = |index| {
        if strict {
            format!("Error at index {index} in: \"{part}\"")
        } else {
            format!("For input string: \"{part}\" under radix 16")
        }
    };
    if units.is_empty() {
        return Err(if strict { String::new() } else { error(0) });
    }
    let mut at = 0;
    let mut limit = -i64::MAX;
    let mut negative = false;
    if units[0] < b'0' as u16 {
        match units[0] {
            45 => {
                negative = true;
                limit = i64::MIN;
            }
            43 => (),
            _ => return Err(error(0)),
        }
        at += 1;
        if at == units.len() {
            return Err(error(at));
        }
    }
    let mut value = 0i64;
    while at < units.len() {
        let digit = digit(units[at]).ok_or_else(|| error(at))? as i64;
        if value < limit / 16 {
            return Err(error(at));
        }
        value *= 16;
        if value < limit + digit {
            return Err(error(at));
        }
        value -= digit;
        at += 1;
    }
    Ok(if negative { value } else { -value })
}

pub fn digit(unit: u16) -> Option<u8> {
    match unit {
        0x41..=0x46 => Some((unit - 0x41 + 10) as u8),
        0x61..=0x66 => Some((unit - 0x61 + 10) as u8),
        0xff21..=0xff26 => Some((unit - 0xff21 + 10) as u8),
        0xff41..=0xff46 => Some((unit - 0xff41 + 10) as u8),
        _ => DECIMAL_STARTS
            .iter()
            .find_map(|&start| unit.checked_sub(start).filter(|&d| d < 10).map(|d| d as u8)),
    }
}
const DECIMAL_STARTS: &[u16] = &[
    0x0030, 0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66, 0x0be6, 0x0c66, 0x0ce6,
    0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80,
    0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
    0xff10,
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uuid_modes_have_distinct_truncation_and_trailing_dash_rules() {
        assert_eq!(
            parse("1-2-3-4-5", true).unwrap(),
            "00000001-0002-0003-0004-000000000005"
        );
        assert!(parse("1-2-3-4-5-", true).is_err());
        assert!(parse("1-2-3-4-5-", false).is_ok());
        assert_eq!(
            parse("0-10000-0-0-0", true).unwrap(),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            parse("0-10000-0-0-0", false).unwrap(),
            "00000001-0000-0000-0000-000000000000"
        );
    }
}
