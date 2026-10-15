//! `PersistableBundle.restoreFromXml` and `XmlUtils`, Android 16.0.0_r1.
// Portions ported from AOSP PersistableBundle.java and XmlUtils.java.
// Copyright (C) 2006, 2014 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0
use aim_android_xml::{Element, Node};
use std::borrow::Cow;
mod parcel;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bundle {
    /// Null keys are accepted by the original ArrayMap. Duplicate keys replace.
    pub entries: Vec<(Option<String>, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i32),
    Long(i64),
    Double(Double),
    Bool(bool),
    String(String),
    Ints(Vec<i32>),
    Longs(Vec<i64>),
    Doubles(Vec<Double>),
    Bools(Vec<bool>),
    Strings(Vec<Option<String>>),
    Bundle(Bundle),
}

/// Preserve the payload bits; Java equality treats all NaNs as equal and
/// distinguishes signed zero.
#[derive(Clone, Copy, Debug, Default)]
pub struct Double(u64);
impl PartialEq for Double {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 || (self.value().is_nan() && other.value().is_nan())
    }
}
impl Double {
    pub fn new(value: f64) -> Self {
        Self(value.to_bits())
    }
    pub fn bits(self) -> u64 {
        self.0
    }
    pub fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Xml(String),
    Runtime(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xml(s) | Self::Runtime(s) => f.write_str(s),
        }
    }
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub(super) enum Event<'a> {
    Start(&'a Element, usize),
    End(&'a Element, usize),
    Text(Cow<'a, str>),
}

/// `next()` events, including text/CDATA but excluding comments.
pub(super) struct Cursor<'a> {
    events: Vec<Event<'a>>,
    at: usize,
}
impl<'a> Cursor<'a> {
    pub(super) fn new(root: &'a Element) -> Self {
        fn add<'a>(e: &'a Element, depth: usize, events: &mut Vec<Event<'a>>) {
            events.push(Event::Start(e, depth));
            let mut pending_text = false;
            for node in &e.content {
                match node {
                    Node::Element(child) => {
                        add(child, depth + 1, events);
                        pending_text = false;
                    }
                    Node::Token(aim_android_xml::TEXT | aim_android_xml::CDSECT, Some(text)) => {
                        if text.is_empty() {
                            continue;
                        }
                        if pending_text {
                            if let Some(Event::Text(previous)) = events.last_mut() {
                                previous.to_mut().push_str(text);
                            }
                        } else {
                            events.push(Event::Text(Cow::Borrowed(text)));
                        }
                        pending_text = true;
                    }
                    Node::Token(aim_android_xml::COMMENT | 8, _) => {}
                    _ => pending_text = false,
                }
            }
            events.push(Event::End(e, depth));
        }
        let mut events = Vec::new();
        add(root, 1, &mut events);
        Self { events, at: 0 }
    }
    pub(super) fn event(&self) -> Option<Event<'a>> {
        self.events.get(self.at).cloned()
    }
    pub(super) fn next(&mut self) -> Option<Event<'a>> {
        self.at += 1;
        self.event()
    }
    pub(super) fn skip(&mut self) {
        let depth = match self.event() {
            Some(Event::Start(_, d)) => d,
            _ => return,
        };
        while let Some(event) = self.next() {
            if matches!(event, Event::End(_, d) if d == depth) {
                break;
            }
        }
    }
    fn element(&self) -> Result<&'a Element> {
        match self.event() {
            Some(Event::Start(e, _)) => Ok(e),
            _ => Err(xml("expected start tag")),
        }
    }
}

fn xml(message: impl Into<String>) -> Error {
    Error::Xml(message.into())
}
fn required<T>(value: std::result::Result<Option<T>, String>, name: &str) -> Result<T> {
    value
        .map_err(xml)?
        .ok_or_else(|| xml(format!("missing {name}")))
}

impl Bundle {
    /// PersistableBundle.saveToXml/XmlUtils value tags at the pinned version.
    pub fn save(&self, name: &str) -> Result<Element> {
        if self
            .entries
            .iter()
            .any(|(_, v)| matches!(v, Value::Strings(items) if items.iter().any(Option::is_none)))
        {
            return Err(Error::Runtime(
                "null string-array item cannot be serialized".into(),
            ));
        }
        let mut root = Element {
            name: name.into(),
            attrs: vec![],
            content: vec![],
        };
        for (key, value) in &self.entries {
            let (tag, scalar) = match value {
                Value::Null => ("null", None),
                Value::Int(v) => ("int", Some(aim_android_xml::Value::Int(*v))),
                Value::Long(v) => ("long", Some(aim_android_xml::Value::Long(*v))),
                Value::Double(v) => ("double", Some(aim_android_xml::Value::Double(v.value()))),
                Value::Bool(v) => ("boolean", Some(aim_android_xml::Value::Bool(*v))),
                Value::String(_) => ("string", None),
                Value::Ints(_) => ("int-array", None),
                Value::Longs(_) => ("long-array", None),
                Value::Doubles(_) => ("double-array", None),
                Value::Bools(_) => ("boolean-array", None),
                Value::Strings(_) => ("string-array", None),
                Value::Bundle(_) => ("pbundle_as_map", None),
            };
            let mut e = match value {
                Value::Bundle(bundle) => bundle.save(tag)?,
                _ => Element {
                    name: tag.into(),
                    attrs: vec![],
                    content: vec![],
                },
            };
            if let Some(key) = key {
                e.attrs
                    .push(("name".into(), aim_android_xml::Value::String(key.clone())));
            }
            if let Some(value) = scalar {
                e.attrs.push(("value".into(), value));
            }
            let array = match value {
                Value::Ints(v) => Some(
                    v.iter()
                        .map(|v| Some(aim_android_xml::Value::Int(*v)))
                        .collect::<Vec<_>>(),
                ),
                Value::Longs(v) => Some(
                    v.iter()
                        .map(|v| Some(aim_android_xml::Value::Long(*v)))
                        .collect(),
                ),
                Value::Doubles(v) => Some(
                    v.iter()
                        .map(|v| Some(aim_android_xml::Value::Double(v.value())))
                        .collect(),
                ),
                Value::Bools(v) => Some(
                    v.iter()
                        .map(|v| Some(aim_android_xml::Value::Bool(*v)))
                        .collect(),
                ),
                Value::Strings(v) => Some(
                    v.iter()
                        .map(|v| {
                            v.as_ref()
                                .map(|v| aim_android_xml::Value::String(v.clone()))
                        })
                        .collect(),
                ),
                Value::String(text) => {
                    e.content
                        .push(Node::Token(aim_android_xml::TEXT, Some(text.clone())));
                    None
                }
                _ => None,
            };
            if let Some(items) = array {
                e.attrs.push((
                    "num".into(),
                    aim_android_xml::Value::Int(items.len() as i32),
                ));
                for value in items {
                    let attrs = value.map(|v| vec![("value".into(), v)]).unwrap_or_default();
                    e.content.push(Node::Element(Element {
                        name: "item".into(),
                        attrs,
                        content: vec![],
                    }));
                }
            }
            root.content.push(Node::Element(e));
        }
        Ok(root)
    }

    /// A text XML tree, or a tree read by `aim_android_xml::read_next` so the
    /// source parser's text/CDATA semantics are preserved.
    pub fn restore(root: &Element) -> Result<Self> {
        Self::read(&mut Cursor::new(root))
    }

    pub(super) fn read(cursor: &mut Cursor<'_>) -> Result<Self> {
        let (name, depth) = match cursor.event() {
            Some(Event::Start(e, depth)) => (e.name.clone(), depth),
            _ => return Err(xml("expected bundle tag")),
        };
        // Empty bundles stop on their own end tag, leaving the next sibling
        // for the suspension/settings owner.
        while let Some(event) = cursor.next() {
            match event {
                Event::End(_, d) if d >= depth => break,
                Event::Start(_, _) => return read_map(cursor, &name),
                _ => {}
            }
        }
        Ok(Self::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_types_restore_nested_arrays_and_reject_original_null_string_write() {
        let value = Bundle {
            entries: vec![
                (None, Value::Null),
                (Some("text".into()), Value::String("<text>&".into())),
                (Some("ints".into()), Value::Ints(vec![1, -2])),
                (Some("longs".into()), Value::Longs(vec![i64::MIN])),
                (Some("double".into()), Value::Double(Double::new(-0.0))),
                (
                    Some("doubles".into()),
                    Value::Doubles(vec![Double::new(f64::NAN)]),
                ),
                (Some("bools".into()), Value::Bools(vec![true, false])),
                (
                    Some("strings".into()),
                    Value::Strings(vec![Some("one".into())]),
                ),
                (
                    Some("nested".into()),
                    Value::Bundle(Bundle {
                        entries: vec![
                            (Some("long".into()), Value::Long(9)),
                            (Some("int".into()), Value::Int(8)),
                            (Some("bool".into()), Value::Bool(true)),
                        ],
                    }),
                ),
            ],
        };
        let root = value.save("app-extras").unwrap();
        let restored = Bundle::restore(
            &aim_android_xml::read(&aim_android_xml::abx::write(&root).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(value, restored);
        assert!(matches!(
            Bundle {
                entries: vec![(None, Value::Strings(vec![None]))]
            }
            .save("extras"),
            Err(Error::Runtime(_))
        ));
    }

    #[test]
    fn duplicate_defusing_occurs_after_map_population() {
        let root = aim_android_xml::read_next(b"<extras><int name='same' value='1'/><float name='same' value='2.5'/><map name='nested'><int name='keep' value='3'/><list name='drop'/></map></extras>").unwrap();
        let value = Bundle::restore(&root).unwrap();
        assert_eq!(
            value.entries,
            vec![(
                Some("nested".into()),
                Value::Bundle(Bundle {
                    entries: vec![(Some("keep".into()), Value::Int(3))]
                })
            )]
        );
    }

    #[test]
    fn arrays_retain_declared_default_slots_and_null_strings() {
        let root = aim_android_xml::read_next(b"<extras><int-array name='ints' num='3'><item value='7'/></int-array><string-array name='strings' num='2'><item/></string-array></extras>").unwrap();
        let value = Bundle::restore(&root).unwrap();
        assert_eq!(
            value.entries,
            vec![
                (Some("ints".into()), Value::Ints(vec![7, 0, 0])),
                (Some("strings".into()), Value::Strings(vec![None, None]))
            ]
        );
        assert_ne!(Double::new(0.0), Double::new(-0.0));
        assert_eq!(Double::new(f64::NAN), Double::new(-f64::NAN));
    }

    #[test]
    fn malformed_arrays_distinguish_xml_and_runtime_errors() {
        for (xml, runtime) in [
            ("<extras><int-array num='-1'/></extras>", true),
            (
                "<extras><int-array num='0'><item value='1'/></int-array></extras>",
                true,
            ),
            (
                "<extras><int-array num='1'><item value='bad'/></int-array></extras>",
                false,
            ),
        ] {
            let error =
                Bundle::restore(&aim_android_xml::read_next(xml.as_bytes()).unwrap()).unwrap_err();
            assert_eq!(matches!(error, Error::Runtime(_)), runtime);
        }
    }
}

// Float, byte-array, list and set are valid XmlUtils values, but the
// PersistableBundle constructor removes them *after* duplicate replacement.
fn read_map(cursor: &mut Cursor<'_>, end: &str) -> Result<Bundle> {
    let mut entries: Vec<(Option<String>, Option<Value>)> = Vec::new();
    loop {
        match cursor.event() {
            Some(Event::Start(e, _)) => {
                let key = e.string("name").map(|s| s.into_owned());
                let value = read_value(cursor)?;
                if let Some(entry) = entries.iter_mut().find(|(k, _)| *k == key) {
                    entry.1 = value;
                } else {
                    entries.push((key, value));
                }
            }
            Some(Event::End(e, _)) => {
                if e.name != end {
                    return Err(xml(format!("unexpected end {}", e.name)));
                }
                return Ok(Bundle {
                    entries: entries
                        .into_iter()
                        .filter_map(|(k, v)| v.map(|v| (k, v)))
                        .collect(),
                });
            }
            Some(Event::Text(_)) => {}
            None => return Err(xml("document ended before bundle end")),
        }
        cursor.next();
    }
}

fn read_value(cursor: &mut Cursor<'_>) -> Result<Option<Value>> {
    let e = cursor.element()?;
    let value = match e.name.as_str() {
        "null" => Some(Value::Null),
        "int" => Some(Value::Int(required(e.int("value"), "value")?)),
        "long" => Some(Value::Long(required(e.long("value"), "value")?)),
        "double" => Some(Value::Double(Double::new(required(
            e.double("value"),
            "value",
        )?))),
        "boolean" => Some(Value::Bool(required(e.bool("value"), "value")?)),
        "float" => {
            required(e.float("value"), "value")?;
            None
        }
        "string" => {
            let mut text = String::new();
            while let Some(event) = cursor.next() {
                match event {
                    Event::Text(value) => text.push_str(&value),
                    Event::Start(_, _) => return Err(xml("start tag in string")),
                    Event::End(end, _) if end.name == "string" => {
                        return Ok(Some(Value::String(text)));
                    }
                    Event::End(_, _) => return Err(xml("unexpected string end")),
                }
            }
            return Err(xml("document ended before string end"));
        }
        "int-array" => {
            return array(cursor, 0i32, |e| required(e.int("value"), "value"))
                .map(|v| Some(Value::Ints(v)));
        }
        "long-array" => {
            return array(cursor, 0i64, |e| required(e.long("value"), "value"))
                .map(|v| Some(Value::Longs(v)));
        }
        "double-array" => {
            return array(cursor, Double::default(), |e| {
                required(e.double("value"), "value").map(Double::new)
            })
            .map(|v| Some(Value::Doubles(v)));
        }
        "boolean-array" => {
            return array(cursor, false, |e| required(e.bool("value"), "value"))
                .map(|v| Some(Value::Bools(v)));
        }
        "string-array" => {
            return array(cursor, None, |e| {
                Ok(e.string("value").map(|s| s.into_owned()))
            })
            .map(|v| Some(Value::Strings(v)));
        }
        "map" => {
            cursor.next();
            return read_map(cursor, "map").map(|v| Some(Value::Bundle(v)));
        }
        "pbundle_as_map" => return Bundle::read(cursor).map(|v| Some(Value::Bundle(v))),
        "list" | "set" => {
            let end = e.name.clone();
            cursor.next();
            loop {
                match cursor.event() {
                    Some(Event::Start(_, _)) => {
                        read_value(cursor)?;
                    }
                    Some(Event::End(e, _)) if e.name == end => return Ok(None),
                    Some(Event::End(_, _)) => return Err(xml("unexpected collection end")),
                    Some(Event::Text(_)) => {}
                    None => return Err(xml("document ended before collection end")),
                }
                cursor.next();
            }
        }
        "byte-array" => {
            let num = required(e.int("num"), "num")?;
            while let Some(event) = cursor.next() {
                match event {
                    Event::Text(text) if num > 0 => {
                        if text.encode_utf16().count() as i64 != i64::from(num) * 2 {
                            return Err(xml("invalid byte-array length"));
                        }
                        if !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Err(Error::Runtime("invalid byte-array hex".into()));
                        }
                    }
                    Event::End(e, _) if e.name == "byte-array" => return Ok(None),
                    Event::End(_, _) => return Err(xml("unexpected byte-array end")),
                    _ => {}
                }
            }
            return Err(xml("document ended before byte-array end"));
        }
        _ => return Err(xml(format!("unknown value {}", e.name))),
    };
    let end = e.name.clone();
    while let Some(event) = cursor.next() {
        match event {
            Event::End(e, _) if e.name == end => return Ok(value),
            _ => return Err(xml(format!("unexpected content in {end}"))),
        }
    }
    Err(xml("document ended before value end"))
}

fn array<T: Clone>(
    cursor: &mut Cursor<'_>,
    default: T,
    read: impl Fn(&Element) -> Result<T>,
) -> Result<Vec<T>> {
    let e = cursor.element()?;
    let num = required(e.int("num"), "num")?;
    let num = usize::try_from(num).map_err(|_| Error::Runtime("negative array size".into()))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(num)
        .map_err(|_| Error::Runtime("array allocation failed".into()))?;
    values.resize(num, default);
    let end = e.name.clone();
    let mut index = 0;
    while let Some(event) = cursor.next() {
        match event {
            Event::Start(e, _) if e.name == "item" => {
                let value = read(e)?;
                *values
                    .get_mut(index)
                    .ok_or_else(|| Error::Runtime("array index out of bounds".into()))? = value;
            }
            Event::End(e, _) if e.name == end => return Ok(values),
            Event::End(e, _) if e.name == "item" => index += 1,
            Event::Text(_) => {}
            _ => return Err(xml("expected array item")),
        }
    }
    Err(xml("document ended before array end"))
}
