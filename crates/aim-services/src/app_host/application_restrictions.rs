//! An app's persisted restrictions, `/data/system/users/<user>/res_<package>.xml`,
//! read as `UserManagerService.readApplicationRestrictionsLAr`,
//! `readEntry` and `readBundleEntry` do at android-16.0.0_r1: an `entry`
//! per key, typed by its `type` attribute, a string array when it has an
//! `m` count of `value` elements.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;

use aim_android_xml::pull::{Event, Reader};
use aim_binder_host::parcel::{Exception, Parcel};

use crate::bundle::{self, Value};

/// A bundle's entries; `Bundle` takes a null key.
type Entries = Vec<(Option<String>, Value)>;

/// Restrictions as the reply's `Bundle`.
pub(super) struct Restrictions(pub Entries);

impl aim_service_aidl::WriteParcelable for Restrictions {
    fn write_to(&self, p: &mut Parcel) {
        write(p, &self.0);
    }
}

/// The restrictions persisted in `file`: none when it does not exist; what
/// was read before an unreadable file or a malformed document, which the
/// original logs and returns.
pub(super) fn read(file: &Path) -> Result<Restrictions, Exception> {
    let mut restrictions = Vec::new();
    if !file.exists() {
        return Ok(Restrictions(restrictions));
    }
    let bytes = match super::device_policy::read_atomic_bytes(file) {
        Ok(bytes) => bytes.unwrap_or_default(),
        Err(e) => {
            eprintln!("UserManagerService: Error parsing {}: {e}", file.display());
            return Ok(Restrictions(restrictions));
        }
    };
    if let Err(Malformed(e)) = parse(&bytes, &mut restrictions)? {
        eprintln!("UserManagerService: Error parsing {}: {e}", file.display());
    }
    Ok(Restrictions(restrictions))
}

/// An `IOException` or `XmlPullParserException`, which the original catches.
struct Malformed(String);

/// `NumberFormatException` escapes the original's reader to the caller.
type Parsed<T> = Result<Result<T, Malformed>, Exception>;

/// The parser's current event, as `getEventType` reports it.
struct Parser<'a> {
    reader: Reader<'a>,
    event: Event,
}

impl Parser<'_> {
    fn next(&mut self) -> Result<&Event, Malformed> {
        self.event = self.reader.next().map_err(Malformed)?;
        Ok(&self.event)
    }

    /// `XmlPullParser.nextText`.
    fn next_text(&mut self) -> Result<String, Malformed> {
        if !matches!(self.event, Event::Start(_)) {
            return Err(Malformed(
                "parser must be on START_TAG to read next text".into(),
            ));
        }
        let text = match self.next()? {
            Event::Text(text) => {
                let text = text.clone();
                self.next()?;
                text
            }
            _ => String::new(),
        };
        if !matches!(self.event, Event::End(_)) {
            return Err(Malformed(
                "event TEXT must be immediately followed by END_TAG".into(),
            ));
        }
        Ok(text)
    }

    /// `XmlUtils.nextElementWithin`.
    fn next_element_within(&mut self, outer: i32) -> Result<bool, Malformed> {
        loop {
            self.next()?;
            let depth = self.reader.depth();
            match self.event {
                Event::EndDocument => return Ok(false),
                Event::End(_) if depth == outer => return Ok(false),
                Event::Start(_) if depth == outer + 1 => return Ok(true),
                _ => {}
            }
        }
    }
}

fn parse(bytes: &[u8], restrictions: &mut Entries) -> Parsed<()> {
    let mut parser = match Reader::new(bytes) {
        Ok(reader) => Parser {
            reader,
            event: Event::EndDocument,
        },
        Err(e) => return Ok(Err(Malformed(e))),
    };
    // XmlUtils.nextElement, then the root's events up to the end.
    loop {
        match parser.next() {
            Ok(Event::Start(_)) => break,
            Ok(Event::EndDocument) => {
                return Ok(Err(Malformed("Unable to read restrictions file".into())));
            }
            Ok(_) => {}
            Err(e) => return Ok(Err(e)),
        }
    }
    loop {
        match parser.next() {
            Ok(Event::EndDocument) => return Ok(Ok(())),
            Ok(_) => {}
            Err(e) => return Ok(Err(e)),
        }
        if let Err(e) = read_entry(restrictions, &mut parser)? {
            return Ok(Err(e));
        }
    }
}

/// Java's `String.trim`.
fn trim(s: &str) -> String {
    s.trim_matches(|c| c <= ' ').to_owned()
}

/// `Bundle.put*`: a key put again replaces its value.
fn put(bundle: &mut Entries, key: Option<String>, value: Value) {
    match bundle.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => bundle.push((key, value)),
    }
}

/// `Integer.parseInt`, of ASCII digits: the other Unicode decimal digits
/// `Character.digit` takes are not recognized.
fn parse_int(value: &str) -> Result<i32, Exception> {
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Exception::illegal_argument(format!(
            "For input string: \"{value}\""
        )));
    }
    value
        .parse()
        .map_err(|_| Exception::illegal_argument(format!("For input string: \"{value}\"")))
}

fn read_entry(restrictions: &mut Entries, parser: &mut Parser<'_>) -> Parsed<()> {
    let Event::Start(entry) = &parser.event else {
        return Ok(Ok(()));
    };
    if entry.name != "entry" {
        return Ok(Ok(()));
    }
    let key = entry.string("key").map(|k| k.into_owned());
    let kind = entry.string("type").map(|t| t.into_owned());
    // `getAttributeInt(null, "m", -1)`: an unconvertible count is the default.
    let mut count = entry.int("m").ok().flatten().unwrap_or(-1);
    if count != -1 {
        let mut values = Vec::new();
        while count > 0 {
            match parser.next() {
                Ok(Event::EndDocument) => break,
                Ok(Event::Start(value)) if value.name == "value" => {}
                Ok(_) => continue,
                Err(e) => return Ok(Err(e)),
            }
            match parser.next_text() {
                Ok(text) => values.push(Some(trim(&text))),
                Err(e) => return Ok(Err(e)),
            }
            count -= 1;
        }
        put(restrictions, key, Value::StringArray(values));
    } else if kind.as_deref() == Some("B") {
        match read_bundle_entry(parser)? {
            Ok(bundle) => put(restrictions, key, Value::Bundle(bundle)),
            Err(e) => return Ok(Err(e)),
        }
    } else if kind.as_deref() == Some("BA") {
        let outer = parser.reader.depth();
        let mut bundles = Vec::new();
        loop {
            match parser.next_element_within(outer) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => return Ok(Err(e)),
            }
            match read_bundle_entry(parser)? {
                Ok(bundle) => bundles.push(("android.os.Bundle".to_owned(), parcel(&bundle))),
                Err(e) => return Ok(Err(e)),
            }
        }
        put(restrictions, key, Value::ParcelableArray(bundles));
    } else {
        let value = match parser.next_text() {
            Ok(text) => trim(&text),
            Err(e) => return Ok(Err(e)),
        };
        let value = match kind.as_deref() {
            Some("b") => Value::Bool(value.eq_ignore_ascii_case("true")),
            Some("i") => Value::Int(parse_int(&value)?),
            _ => Value::String(Some(value)),
        };
        put(restrictions, key, value);
    }
    Ok(Ok(()))
}

fn read_bundle_entry(parser: &mut Parser<'_>) -> Parsed<Entries> {
    let mut bundle = Vec::new();
    let outer = parser.reader.depth();
    loop {
        match parser.next_element_within(outer) {
            Ok(true) => {}
            Ok(false) => return Ok(Ok(bundle)),
            Err(e) => return Ok(Err(e)),
        }
        if let Err(e) = read_entry(&mut bundle, parser)? {
            return Ok(Err(e));
        }
    }
}

fn write(p: &mut Parcel, entries: &[(Option<String>, Value)]) {
    let entries: Vec<(Option<&str>, Value)> = entries
        .iter()
        .map(|(k, v)| (k.as_deref(), v.clone()))
        .collect();
    bundle::write_nullable(p, &entries);
}

/// A bundle of a `Parcelable[]`, as `Bundle.writeToParcel` writes it.
fn parcel(entries: &[(Option<String>, Value)]) -> Vec<u8> {
    let mut p = Parcel::new();
    write(&mut p, entries);
    p.data().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(xml: &str) -> (Entries, bool) {
        let mut restrictions = Vec::new();
        let complete = parse(xml.as_bytes(), &mut restrictions).unwrap().is_ok();
        (restrictions, complete)
    }

    fn key(k: &str) -> Option<String> {
        Some(k.to_owned())
    }

    fn le(words: &[i32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn entries_of_every_type_as_the_original_writes_them() {
        let (restrictions, complete) = parsed(
            "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<restrictions>\
             <entry key=\"on\" type=\"b\">TRUE</entry>\
             <entry key=\"count\" type=\"i\"> -7 </entry>\
             <entry key=\"name\" type=\"s\">\n  value\n</entry>\
             <entry key=\"empty\" type=\"s\" />\
             <entry key=\"list\" type=\"sa\" m=\"2\"><value> a </value><value>b</value></entry>\
             <entry key=\"inner\" type=\"B\"><entry key=\"x\" type=\"i\">+1</entry></entry>\
             <entry key=\"many\" type=\"BA\"><entry type=\"B\"><entry key=\"y\" type=\"s\">z</entry></entry>\
             <entry type=\"B\" /></entry>\
             <entry key=\"name\" type=\"s\">again</entry>\
             </restrictions>",
        );
        assert!(complete);
        assert_eq!(
            restrictions,
            vec![
                (key("on"), Value::Bool(true)),
                (key("count"), Value::Int(-7)),
                (key("name"), Value::String(Some("again".into()))),
                (key("empty"), Value::String(Some(String::new()))),
                (
                    key("list"),
                    Value::StringArray(vec![Some("a".into()), Some("b".into())])
                ),
                (key("inner"), Value::Bundle(vec![(key("x"), Value::Int(1))])),
                (
                    key("many"),
                    Value::ParcelableArray(vec![
                        // Length, BUNDLE_MAGIC, one entry: "y" (VAL_STRING)
                        // "z", and no intent; then an empty bundle.
                        (
                            "android.os.Bundle".to_owned(),
                            le(&[24, 0x4C44_4E42, 1, 1, 0x79, 0, 1, 0x7A, 0])
                        ),
                        ("android.os.Bundle".to_owned(), le(&[0])),
                    ])
                ),
            ]
        );
    }

    #[test]
    fn an_entry_without_a_key_has_the_null_key() {
        let (restrictions, complete) = parsed(
            "<restrictions><entry key=\"a\" type=\"s\">1</entry><entry type=\"s\">2</entry>\
             <entry type=\"i\">3</entry></restrictions>",
        );
        assert!(complete);
        assert_eq!(
            restrictions,
            vec![
                (key("a"), Value::String(Some("1".into()))),
                (None, Value::Int(3)),
            ]
        );
        // ArrayMap hashes null as 0, before "a" (97); writeString(null).
        let mut p = Parcel::new();
        write(&mut p, &restrictions);
        let mut expected = le(&[36, 0x4C44_4E42, 2, -1, 1, 3, 1, 0x61, 0, 1, 0x31]);
        expected.extend(le(&[0]));
        assert_eq!(p.data(), expected);
    }

    #[test]
    fn an_unconvertible_count_reads_the_entry_as_a_single_value() {
        let (restrictions, complete) =
            parsed("<restrictions><entry key=\"k\" type=\"s\" m=\"x\">v</entry></restrictions>");
        assert!(complete);
        assert_eq!(
            restrictions,
            vec![(key("k"), Value::String(Some("v".into())))]
        );
    }

    #[test]
    fn a_malformed_document_keeps_what_was_read_and_a_bad_int_throws() {
        let (restrictions, complete) = parsed(
            "<restrictions><entry key=\"a\" type=\"s\">1</entry><entry key=\"b\" type=\"s\"><x/></entry>",
        );
        assert!(!complete);
        assert_eq!(
            restrictions,
            vec![(key("a"), Value::String(Some("1".into())))]
        );
        let (restrictions, complete) = parsed("");
        assert!(!complete && restrictions.is_empty());
        for bad in ["x", "", "-", "2147483648", "\u{0661}"] {
            let xml =
                format!("<restrictions><entry key=\"n\" type=\"i\">{bad}</entry></restrictions>");
            let error = parse(xml.as_bytes(), &mut Vec::new()).err().unwrap();
            assert_eq!(error.message, format!("For input string: \"{bad}\""));
        }
    }

    #[test]
    fn binary_xml_reads_as_text_xml() {
        let xml = "<restrictions><entry key=\"list\" type=\"sa\" m=\"1\"><value>a</value></entry>\
                   <entry key=\"on\" type=\"b\">true</entry></restrictions>";
        let abx =
            aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap();
        let mut restrictions = Vec::new();
        assert!(parse(&abx, &mut restrictions).unwrap().is_ok());
        assert_eq!(restrictions, parsed(xml).0);
        assert_eq!(restrictions.len(), 2);
    }

    #[test]
    fn the_legacy_backup_is_read_for_an_existing_base_file() {
        let dir = crate::app_host::test_temp_dir("application-restrictions-backup");
        let base = dir.join("res_com.example.xml");
        let backup = dir.join("res_com.example.xml.bak");
        let document = |v: &str| {
            format!("<restrictions><entry key=\"k\" type=\"s\">{v}</entry></restrictions>")
        };
        // The original tests the base file before AtomicFile.openRead.
        std::fs::write(&backup, document("backup")).unwrap();
        assert!(read(&base).unwrap().0.is_empty());
        std::fs::write(&base, document("base")).unwrap();
        assert_eq!(
            read(&base).unwrap().0,
            [(key("k"), Value::String(Some("backup".into())))]
        );
        std::fs::remove_file(&backup).unwrap();
        assert_eq!(
            read(&base).unwrap().0,
            [(key("k"), Value::String(Some("base".into())))]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
