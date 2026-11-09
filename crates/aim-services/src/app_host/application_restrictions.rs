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

/// The restrictions persisted in `file`: none when it does not exist; what
/// was read before an unreadable file or a malformed document, which the
/// original logs and returns.
pub(super) fn read(file: &Path) -> Result<Vec<(String, Value)>, Exception> {
    let mut restrictions = Vec::new();
    if !file.exists() {
        return Ok(restrictions);
    }
    let bytes = match super::device_policy::read_atomic_bytes(file) {
        Ok(bytes) => bytes.unwrap_or_default(),
        Err(e) => {
            eprintln!("UserManagerService: Error parsing {}: {e}", file.display());
            return Ok(restrictions);
        }
    };
    if let Err(Malformed(e)) = parse(&bytes, &mut restrictions)? {
        eprintln!("UserManagerService: Error parsing {}: {e}", file.display());
    }
    Ok(restrictions)
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

fn parse(bytes: &[u8], restrictions: &mut Vec<(String, Value)>) -> Parsed<()> {
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
fn put(bundle: &mut Vec<(String, Value)>, key: String, value: Value) {
    match bundle.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => bundle.push((key, value)),
    }
}

fn read_entry(restrictions: &mut Vec<(String, Value)>, parser: &mut Parser<'_>) -> Parsed<()> {
    let Event::Start(entry) = &parser.event else {
        return Ok(Ok(()));
    };
    if entry.name != "entry" {
        return Ok(Ok(()));
    }
    // The original's writer cannot persist a null key.
    let Some(key) = entry.string("key").map(|k| k.into_owned()) else {
        return Ok(Err(Malformed("restriction entry without a key".into())));
    };
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
            Some("i") => Value::Int(value.parse().map_err(|_| {
                Exception::illegal_argument(format!("For input string: \"{value}\""))
            })?),
            _ => Value::String(Some(value)),
        };
        put(restrictions, key, value);
    }
    Ok(Ok(()))
}

fn read_bundle_entry(parser: &mut Parser<'_>) -> Parsed<Vec<(String, Value)>> {
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

/// A bundle of a `Parcelable[]`, as `Bundle.writeToParcel` writes it.
fn parcel(entries: &[(String, Value)]) -> Vec<u8> {
    let mut p = Parcel::new();
    let entries: Vec<(&str, Value)> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    bundle::write(&mut p, &entries);
    p.data().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(xml: &str) -> (Vec<(String, Value)>, bool) {
        let mut restrictions = Vec::new();
        let complete = parse(xml.as_bytes(), &mut restrictions).unwrap().is_ok();
        (restrictions, complete)
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
             <entry key=\"inner\" type=\"B\"><entry key=\"x\" type=\"i\">1</entry></entry>\
             <entry key=\"many\" type=\"BA\"><entry type=\"B\"><entry key=\"y\" type=\"s\">z</entry></entry>\
             <entry type=\"B\" /></entry>\
             <entry key=\"name\" type=\"s\">again</entry>\
             </restrictions>",
        );
        assert!(complete);
        let inner = vec![("x".to_owned(), Value::Int(1))];
        let first = vec![("y".to_owned(), Value::String(Some("z".into())))];
        assert_eq!(
            restrictions,
            vec![
                ("on".to_owned(), Value::Bool(true)),
                ("count".to_owned(), Value::Int(-7)),
                ("name".to_owned(), Value::String(Some("again".into()))),
                ("empty".to_owned(), Value::String(Some(String::new()))),
                (
                    "list".to_owned(),
                    Value::StringArray(vec![Some("a".into()), Some("b".into())])
                ),
                ("inner".to_owned(), Value::Bundle(inner)),
                (
                    "many".to_owned(),
                    Value::ParcelableArray(vec![
                        ("android.os.Bundle".to_owned(), parcel(&first)),
                        ("android.os.Bundle".to_owned(), parcel(&[])),
                    ])
                ),
            ]
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
            vec![("a".to_owned(), Value::String(Some("1".into())))]
        );
        let (restrictions, complete) = parsed("");
        assert!(!complete && restrictions.is_empty());
        let error = parse(
            b"<restrictions><entry key=\"n\" type=\"i\">x</entry></restrictions>",
            &mut Vec::new(),
        )
        .err()
        .unwrap();
        assert_eq!(error.message, "For input string: \"x\"");
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
    fn a_missing_file_has_no_restrictions() {
        let path = std::env::temp_dir().join(format!("res_missing_{}.xml", std::process::id()));
        assert!(read(&path).unwrap().is_empty());
    }
}
