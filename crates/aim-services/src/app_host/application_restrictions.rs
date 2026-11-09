//! An app's persisted restrictions, `/data/system/users/<user>/res_<package>.xml`,
//! read as `UserManagerService.readApplicationRestrictionsLAr`,
//! `readEntry` and `readBundleEntry` do at android-16.0.0_r1: an `entry`
//! per key, typed by its `type` attribute, a string array when it has an
//! `m` count of `value` elements; written as
//! `writeApplicationRestrictionsLAr` and `writeBundle` write them, and
//! removed as `cleanAppRestrictionsForPackageLAr` removes them.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;

use aim_android_xml::pull::{Event, Reader};
use aim_android_xml::{Element, Node, TEXT, Value as XmlValue};
use aim_binder_host::parcel::{self, BAD_VALUE, Exception, Parcel};

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

/// Restrictions as `setApplicationRestrictions` takes them: whether the
/// `Bundle` is empty, and the document `writeBundle` makes of its entries
/// (in its `ArrayMap`'s order, the parcel's), or the exception it throws
/// on one: a null key, a value of another type than the ones it writes
/// (its `String[]` cast), or a `Parcelable[]` item that is not a `Bundle`.
pub(super) struct Incoming {
    empty: bool,
    document: Result<Element, String>,
}

impl aim_service_aidl::ReadParcelable for Incoming {
    fn read_from(r: &mut parcel::Reader<'_>) -> parcel::Result<Self> {
        let (count, entries) = read_bundle(r)?;
        Ok(Incoming {
            empty: count == 0,
            document: entries.map(|content| Element {
                name: "restrictions".into(),
                attrs: Vec::new(),
                content,
            }),
        })
    }
}

/// A `Bundle` (`readFromParcelInner`): its entry count, and its entries
/// as `writeBundle` writes them.
fn read_bundle(r: &mut parcel::Reader<'_>) -> parcel::Result<(i32, Result<Vec<Node>, String>)> {
    let length = r.read_i32()?;
    if length == 0 {
        return Ok((0, Ok(Vec::new())));
    }
    if length < 0 || length % 4 != 0 {
        return Err(BAD_VALUE);
    }
    if !matches!(r.read_i32()?, bundle::MAGIC | bundle::MAGIC_NATIVE) {
        return Err(BAD_VALUE);
    }
    let end = r.position() + length as usize;
    let count = r.read_i32()?;
    let mut entries = Vec::new();
    for _ in 0..count.max(0) {
        let key = r.read_string16()?;
        match read_entry_value(r, key)? {
            Some(Ok(entry)) => entries.push(entry),
            Some(Err(e)) => return skip_to(r, end).map(|()| (count, Err(e))),
            // A type this reader cannot step over: none `writeBundle` writes.
            None => {
                return skip_to(r, end)
                    .map(|()| (count, Err("ClassCastException: not a String[]".into())));
            }
        }
    }
    if r.position() != end {
        return Err(BAD_VALUE);
    }
    r.read_bool()?; // has an intent
    Ok((count, Ok(entries)))
}

/// The end of a bundle, after the entries left unread.
fn skip_to(r: &mut parcel::Reader<'_>, end: usize) -> parcel::Result<()> {
    r.skip(end.checked_sub(r.position()).ok_or(BAD_VALUE)?)?;
    r.read_bool()?;
    Ok(())
}

/// An `entry` element of `kind`, as `writeBundle` writes it: a string
/// array with its count `m`.
fn entry(key: Option<String>, kind: &str, m: Option<i32>, content: Vec<Node>) -> Node {
    let mut attrs = Vec::new();
    if let Some(key) = key {
        attrs.push(("key".to_owned(), XmlValue::String(key)));
    }
    attrs.push(("type".to_owned(), XmlValue::String(kind.into())));
    if let Some(m) = m {
        attrs.push(("m".to_owned(), XmlValue::Int(m)));
    }
    Node::Element(Element {
        name: "entry".into(),
        attrs,
        content,
    })
}

fn text(s: impl Into<String>) -> Vec<Node> {
    vec![Node::Token(TEXT, Some(s.into()))]
}

/// One entry's value (`Parcel.readValue`) as `writeBundle` writes it; none
/// for a type whose length is not known.
fn read_entry_value(
    r: &mut parcel::Reader<'_>,
    key: Option<String>,
) -> parcel::Result<Option<Result<Node, String>>> {
    let kind = r.read_i32()?;
    let value = match kind {
        bundle::VAL_NULL => Ok(("s", None, text(""))),
        bundle::VAL_STRING => Ok(("s", None, text(r.read_string16()?.unwrap_or_default()))),
        bundle::VAL_INTEGER => Ok(("i", None, text(r.read_i32()?.to_string()))),
        bundle::VAL_BOOLEAN => Ok(("b", None, text(r.read_bool()?.to_string()))),
        bundle::VAL_BUNDLE => read_bundle(r)?.1.map(|content| ("B", None, content)),
        bundle::VAL_STRINGARRAY => match r.read_i32()? {
            n if n < 0 => Ok(("s", None, text(""))),
            n => {
                let mut values = Vec::new();
                for _ in 0..n {
                    values.push(Node::Element(Element {
                        name: "value".into(),
                        attrs: Vec::new(),
                        content: text(r.read_string16()?.unwrap_or_default()),
                    }));
                }
                Ok(("sa", Some(n), values))
            }
        },
        bundle::VAL_PARCELABLEARRAY => {
            let len = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
            let end = r.position() + len;
            let n = r.read_i32()?;
            let mut bundles = Ok(Vec::new());
            for _ in 0..n.max(0) {
                if r.read_string16()?.as_deref() != Some("android.os.Bundle") {
                    bundles = Err(
                        "IllegalArgumentException: bundle-array can only hold Bundles".to_owned(),
                    );
                    break;
                }
                let content = read_bundle(r)?.1;
                if let Ok(items) = &mut bundles {
                    match content {
                        Ok(content) => items.push(entry(None, "B", None, content)),
                        Err(e) => bundles = Err(e),
                    }
                }
            }
            r.skip(end.checked_sub(r.position()).ok_or(BAD_VALUE)?)?;
            if n < 0 {
                Ok(("s", None, text("")))
            } else {
                bundles.map(|items| ("BA", None, items))
            }
        }
        kind if bundle::is_length_prefixed(kind) => {
            let len = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
            r.skip(len)?;
            Err("ClassCastException: not a String[]".to_owned())
        }
        _ => return Ok(None),
    };
    Ok(Some(value.and_then(|(kind, m, content)| {
        let key = key.ok_or("NullPointerException: a null key")?;
        Ok(entry(Some(key), kind, m, content))
    })))
}

/// `setApplicationRestrictions`' store: `restrictions` written to `file`
/// as `writeApplicationRestrictionsLAr` writes them (an exception logged,
/// the file left as it was), or the file removed for none
/// (`cleanAppRestrictionsForPackageLAr`). Whether the app is to be told:
/// always after a write, after a removal only of an existing file.
pub(super) fn store(file: &Path, restrictions: Option<&Incoming>) -> bool {
    let Some(restrictions) = restrictions.filter(|r| !r.empty) else {
        if !file.exists() {
            return false;
        }
        if let Err(e) = std::fs::remove_file(file) {
            eprintln!("UserManagerService: {} not deleted: {e}", file.display());
        }
        return true;
    };
    let written = restrictions
        .document
        .clone()
        .and_then(|root| aim_android_xml::abx::write(&root))
        .and_then(|bytes| {
            super::usage_database::atomic_write(file, &bytes).map_err(|e| e.to_string())
        });
    if let Err(e) = written {
        eprintln!("UserManagerService: Error writing application restrictions list: {e}");
    }
    true
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

    /// `restrictions` as an app's `Bundle` in a call's data
    /// (`writeTypedObject`).
    fn incoming(restrictions: &[(Option<&str>, Value)]) -> Incoming {
        let mut p = Parcel::new();
        p.write_i32(1);
        bundle::write_nullable(&mut p, restrictions);
        let mut r = parcel::Reader::new(p.data(), &[]);
        let incoming = aim_service_aidl::read_typed::<Incoming>(&mut r)
            .unwrap()
            .unwrap();
        assert_eq!(r.remaining(), 0);
        incoming
    }

    #[test]
    fn written_restrictions_read_back_as_they_were_set() {
        let dir = crate::app_host::test_temp_dir("application-restrictions-write");
        let file = dir.join("res_com.example.xml");
        let set = [
            (Some("on"), Value::Bool(true)),
            (Some("off"), Value::Bool(false)),
            (Some("count"), Value::Int(-7)),
            (Some("name"), Value::String(Some("value".into()))),
            (Some("empty"), Value::String(Some(String::new()))),
            (
                Some("list"),
                Value::StringArray(vec![Some("a".into()), Some(" b ".into())]),
            ),
            (
                Some("inner"),
                Value::Bundle(vec![(Some("x".into()), Value::Int(1))]),
            ),
            (
                Some("many"),
                Value::ParcelableArray(vec![
                    (
                        "android.os.Bundle".into(),
                        parcel(&[(key("x"), Value::Int(1))]),
                    ),
                    ("android.os.Bundle".into(), parcel(&[])),
                ]),
            ),
        ];
        assert!(store(&file, Some(&incoming(&set))));
        let bytes = std::fs::read(&file).unwrap();
        assert!(bytes.starts_with(b"ABX"));
        // The original trims what it reads back.
        let mut expected = set.to_vec();
        expected[5].1 = Value::StringArray(vec![Some("a".into()), Some("b".into())]);
        let mut p = Parcel::new();
        bundle::write_nullable(&mut p, &expected);
        let mut q = Parcel::new();
        write(&mut q, &read(&file).unwrap().0);
        assert_eq!(p.data(), q.data());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn null_values_are_written_as_empty_strings() {
        let dir = crate::app_host::test_temp_dir("application-restrictions-null");
        let file = dir.join("res_com.example.xml");
        let set = [
            (Some("n"), Value::Null),
            (Some("s"), Value::String(None)),
            (Some("a"), Value::StringArray(vec![None])),
        ];
        assert!(store(&file, Some(&incoming(&set))));
        let empty = || Value::String(Some(String::new()));
        let mut read = read(&file).unwrap().0;
        read.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            read,
            [
                (key("a"), Value::StringArray(vec![Some(String::new())])),
                (key("n"), empty()),
                (key("s"), empty()),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unwritable_value_leaves_the_file_and_still_tells_the_app() {
        let dir = crate::app_host::test_temp_dir("application-restrictions-unwritable");
        let file = dir.join("res_com.example.xml");
        let before = [(Some("k"), Value::String(Some("v".into())))];
        assert!(store(&file, Some(&incoming(&before))));
        let kept = std::fs::read(&file).unwrap();
        let not_a_bundle =
            Value::ParcelableArray(vec![("android.graphics.Rect".into(), vec![0; 16])]);
        for unwritable in [
            vec![(Some("k"), Value::Long(1))],
            vec![(Some("k"), Value::IntArray(vec![1]))],
            vec![(Some("k"), not_a_bundle)],
            vec![(None, Value::Int(1))],
            vec![
                (Some("a"), Value::Int(1)),
                (Some("b"), Value::Bundle(vec![(key("l"), Value::Long(1))])),
            ],
        ] {
            assert!(store(&file, Some(&incoming(&unwritable))));
            assert_eq!(std::fs::read(&file).unwrap(), kept);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_restrictions_remove_the_file_and_tell_only_if_there_was_one() {
        let dir = crate::app_host::test_temp_dir("application-restrictions-clean");
        let file = dir.join("res_com.example.xml");
        assert!(!store(&file, None));
        assert!(!store(&file, Some(&incoming(&[]))));
        assert!(store(&file, Some(&incoming(&[(Some("k"), Value::Int(1))]))));
        assert!(store(&file, Some(&incoming(&[]))));
        assert!(!file.exists());
        assert!(store(&file, Some(&incoming(&[(Some("k"), Value::Int(1))]))));
        assert!(store(&file, None));
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
