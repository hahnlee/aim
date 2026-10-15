//! VerifierDeviceIdentity.parse/encodeBase32, android-16.0.0_r1.
//! Copyright (C) 2011 The Android Open Source Project, Apache License 2.0.
use super::{ReadError, Settings, string};
use aim_android_xml::Element;

impl Settings {
    pub fn read_verifier(&mut self, start: &Element) -> Result<(), ReadError> {
        let input = string(start, "device")
            .ok_or_else(|| ReadError::FatalInput("null verifier device identity".into()))?;
        let canonical = parse(&input).map_err(ReadError::File)?;
        self.verifier = Some(canonical);
        Ok(())
    }
}

fn parse(input: &str) -> Result<String, String> {
    let mut value = 0u64;
    let mut count = 0;
    for character in input.chars() {
        // String.getBytes("US-ASCII") replaces unmappable scalar values with '?'.
        let byte = if character.is_ascii() {
            character as u8
        } else {
            b'?'
        };
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a',
            b'2'..=b'7' => byte - b'2' + 26,
            b'0' => b'O' - b'A',
            b'1' => b'I' - b'A',
            b'-' => continue,
            _ => return Err(format!("base base-32 character: {byte}")),
        };
        value = (value << 5) | u64::from(digit);
        count += 1;
        if count == 1 && digit > 15 {
            return Err("illegal start character; will overflow".into());
        }
        if count > 13 {
            return Err("too long; should have 13 characters".into());
        }
    }
    if count != 13 {
        return Err("too short; should have 13 characters".into());
    }
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::with_capacity(16);
    for index in 0..13 {
        if index > 0 && index % 4 == 0 {
            output.push('-');
        }
        let shift = 5 * (12 - index);
        output.push(alphabet[((value >> shift) & 31) as usize] as char);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalization_accepts_separators_case_and_ambiguous_digits() {
        assert_eq!(parse("--aaaaaaaaaaaaa--").unwrap(), "AAAA-AAAA-AAAA-A");
        assert_eq!(parse("p777777777777").unwrap(), "P777-7777-7777-7");
        assert_eq!(parse("a00001111aaaa").unwrap(), "AOOO-OIII-IAAA-A");
        assert_eq!(
            parse("QAAAAAAAAAAAA").unwrap_err(),
            "illegal start character; will overflow"
        );
        assert_eq!(
            parse("AAAAAAAAAAAAAA").unwrap_err(),
            "too long; should have 13 characters"
        );
        assert_eq!(
            parse("\u{1f600}").unwrap_err(),
            "base base-32 character: 63"
        );
    }

    #[test]
    fn failed_verifier_read_retains_previous_identity_and_classifies_null() {
        let mut settings = Settings {
            verifier: Some("AAAA-AAAA-AAAA-A".into()),
            ..Default::default()
        };
        let start = aim_android_xml::read(b"<verifier device='bad'/>").unwrap();
        assert!(matches!(
            settings.read_verifier(&start),
            Err(ReadError::File(_))
        ));
        assert_eq!(settings.verifier.as_deref(), Some("AAAA-AAAA-AAAA-A"));
        assert!(matches!(
            settings.read_verifier(&aim_android_xml::read(b"<verifier/>").unwrap()),
            Err(ReadError::FatalInput(_))
        ));
    }
}
