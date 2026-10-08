//! Preferred backups use the pinned modules-utils FastXmlSerializer, with
//! indentation disabled. PersistableBundle XML is emitted by its typed owner.
// Portions ported from AOSP FastXmlSerializer.java (android-16.0.0_r1).
// Copyright (C) 2006 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0
use crate::package::restrictions::persistable::{Bundle, Double, Value as Persistable};
use aim_android_xml::{Element, Node, Value};
use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};

pub(super) fn extras(bytes: &[u8]) -> std::result::Result<Element, String> {
    let mut reader = Reader::new(bytes, &[]);
    let bundle = read_bundle(&mut reader, 0).map_err(|_| "invalid persistable bundle parcel")?;
    if reader.remaining() != 0 {
        return Err("trailing persistable bundle parcel bytes".into());
    }
    bundle.save("extras").map_err(|error| error.to_string())
}
fn count(reader: &mut Reader<'_>) -> Result<usize> {
    usize::try_from(reader.read_i32()?).map_err(|_| BAD_VALUE)
}
fn read_bundle(reader: &mut Reader<'_>, depth: usize) -> Result<Bundle> {
    if depth > 64 {
        return Err(BAD_VALUE);
    }
    let length = reader.read_i32()?;
    if length == 0 {
        return Ok(Bundle::default());
    }
    if length < 0
        || !matches!(
            reader.read_i32()?,
            crate::bundle::MAGIC | crate::bundle::MAGIC_NATIVE
        )
    {
        return Err(BAD_VALUE);
    }
    let end = reader
        .position()
        .checked_add(length as usize)
        .filter(|&end| end <= reader.position() + reader.remaining())
        .ok_or(BAD_VALUE)?;
    let count = count(reader)?;
    if count > reader.remaining() / 8 {
        return Err(BAD_VALUE);
    }
    let mut bundle = Bundle::default();
    for _ in 0..count {
        let key = reader.read_string16()?;
        let value = read_value(reader, depth)?;
        if reader.position() > end {
            return Err(BAD_VALUE);
        }
        if let Some((_, old)) = bundle.entries.iter_mut().find(|(old, _)| *old == key) {
            *old = value;
        } else {
            bundle.entries.push((key, value));
        }
    }
    if reader.position() != end || reader.read_bool()? {
        return Err(BAD_VALUE);
    }
    bundle
        .entries
        .sort_by_key(|(key, _)| key.as_deref().map_or(0, crate::package::info::java_hash));
    Ok(bundle)
}
fn array<T>(
    reader: &mut Reader<'_>,
    width: usize,
    mut read: impl FnMut(&mut Reader<'_>) -> Result<T>,
) -> Result<Option<Vec<T>>> {
    let count = reader.read_i32()?;
    if count < 0 {
        return Ok(None);
    }
    let count = count as usize;
    if count > reader.remaining() / width {
        return Err(BAD_VALUE);
    }
    (0..count)
        .map(|_| read(reader))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}
fn read_value(reader: &mut Reader<'_>, depth: usize) -> Result<Persistable> {
    Ok(match reader.read_i32()? {
        -1 => Persistable::Null,
        0 => reader
            .read_string16()?
            .map(Persistable::String)
            .unwrap_or(Persistable::Null),
        1 => Persistable::Int(reader.read_i32()?),
        6 => Persistable::Long(reader.read_i64()?),
        8 => Persistable::Double(Double::new(f64::from_bits(reader.read_i64()? as u64))),
        9 => Persistable::Bool(reader.read_i32()? == 1),
        14 => array(reader, 4, |r| r.read_string16())?
            .map(Persistable::Strings)
            .unwrap_or(Persistable::Null),
        18 => array(reader, 4, |r| r.read_i32())?
            .map(Persistable::Ints)
            .unwrap_or(Persistable::Null),
        19 => array(reader, 8, |r| r.read_i64())?
            .map(Persistable::Longs)
            .unwrap_or(Persistable::Null),
        23 => array(reader, 4, |r| Ok(r.read_i32()? != 0))?
            .map(Persistable::Bools)
            .unwrap_or(Persistable::Null),
        25 => Persistable::Bundle(read_bundle(reader, depth + 1)?),
        28 => array(reader, 8, |r| {
            Ok(Double::new(f64::from_bits(r.read_i64()? as u64)))
        })?
        .map(Persistable::Doubles)
        .unwrap_or(Persistable::Null),
        _ => return Err(BAD_VALUE),
    })
}

pub(super) fn fast_xml(document: &Element) -> std::result::Result<Vec<u8>, String> {
    let mut output = String::from("<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n");
    write_element(document, &mut output)?;
    Ok(output.into_bytes())
}
fn escape(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("&quot;"),
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            c if (c as u32) < 32 => {
                output.push_str("&#");
                output.push_str(&(c as u32).to_string());
                output.push(';');
            }
            c => output.push(c),
        }
    }
}
fn attribute(value: &Value) -> std::result::Result<String, String> {
    Ok(match value {
        Value::String(s) | Value::Interned(s) => s.clone(),
        Value::Int(v) => v.to_string(),
        Value::IntHex(v) => format!("{v:x}"),
        Value::Long(v) => v.to_string(),
        Value::LongHex(v) => format!("{v:x}"),
        Value::Bool(v) => v.to_string(),
        // Java's shortest decimal formatting is an independently validated
        // owner; scientific/exponent formatting must not use Rust Display.
        Value::Double(v) => super::double::format(*v),
        _ => return Err("unsupported preferred XML attribute type".into()),
    })
}
fn write_element(element: &Element, output: &mut String) -> std::result::Result<(), String> {
    output.push('<');
    output.push_str(&element.name);
    for (name, value) in &element.attrs {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        escape(&attribute(value)?, output);
        output.push('"');
    }
    if element.content.is_empty() {
        output.push_str(" />\n");
        return Ok(());
    }
    let mut in_tag = true;
    for node in &element.content {
        match node {
            Node::Element(child) => {
                if in_tag {
                    output.push_str(">\n");
                    in_tag = false;
                }
                write_element(child, output)?;
            }
            Node::Token(aim_android_xml::TEXT, Some(text)) => {
                if in_tag {
                    output.push('>');
                    in_tag = false;
                }
                escape(text, output);
            }
            _ => return Err("unsupported token in preferred owner document".into()),
        }
    }
    if in_tag {
        output.push_str(" />\n");
    } else {
        output.push_str("</");
        output.push_str(&element.name);
        output.push_str(">\n");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fast_serializer_layout_and_control_escaping() {
        let document = Element {
            name: "root".into(),
            attrs: vec![("test".into(), Value::String("<&>\"\n\t\r\0é".into()))],
            content: vec![
                Node::Element(Element {
                    name: "empty".into(),
                    attrs: vec![],
                    content: vec![],
                }),
                Node::Element(Element {
                    name: "string".into(),
                    attrs: vec![],
                    content: vec![Node::Token(aim_android_xml::TEXT, Some("hello<&".into()))],
                }),
            ],
        };
        assert_eq!(
            String::from_utf8(fast_xml(&document).unwrap()).unwrap(),
            "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<root test=\"&lt;&amp;&gt;&quot;&#10;&#9;&#13;&#0;é\">\n<empty />\n<string>hello&lt;&amp;</string>\n</root>\n"
        );
    }
    #[test]
    fn persistable_decode_rejects_tail_and_preserves_values() {
        let bundle = Bundle {
            entries: vec![
                (Some("text".into()), Persistable::String("<&é".into())),
                (None, Persistable::Long(12)),
                (
                    Some("nested".into()),
                    Persistable::Bundle(Bundle {
                        entries: vec![(Some("truth".into()), Persistable::Bool(true))],
                    }),
                ),
                (Some("array".into()), Persistable::Ints(vec![0, -7])),
                (Some("null".into()), Persistable::Null),
            ],
        };
        let mut bytes = bundle.parcel().unwrap().data().to_vec();
        let document = extras(&bytes).unwrap();
        let roundtrip = Bundle::restore(&document).unwrap();
        assert_eq!(roundtrip.parcel().unwrap().data(), bytes);
        assert!(fast_xml(&document).is_ok());
        bytes.extend_from_slice(&[0; 4]);
        assert!(extras(&bytes).is_err());
        assert!(extras(&[]).is_err());
    }
}
