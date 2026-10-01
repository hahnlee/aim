//! Decoded replies, compared and logged: what a model's `decode_reply`
//! makes of a reply with the generated readers.

use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, Returned, WriteParcelable};

/// A decoded reply or part of one. Binders and files are indices into the
/// exchange's identity table: equal index, same identity.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i32),
    Long(i64),
    /// An `f32`'s bits.
    Float(u32),
    Str(String),
    List(Vec<Value>),
    /// A parcelable's fields, in the order it writes them.
    Fields(Vec<(String, Value)>),
    Bytes(Vec<u8>),
    Binder(u32),
    File(u32),
    Exception {
        code: i32,
        message: String,
    },
    /// A binder status instead of a reply.
    Status(i32),
    Slice {
        count: i32,
        creator: Option<String>,
        items: Vec<u8>,
    },
}

impl Value {
    /// The value as JSON.
    pub fn json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(v) => out.push_str(&v.to_string()),
            Value::Long(v) => out.push_str(&v.to_string()),
            Value::Float(bits) => {
                let v = f32::from_bits(*bits);
                if v.is_finite() {
                    out.push_str(&v.to_string());
                } else {
                    json_string(&v.to_string(), out);
                }
            }
            Value::Str(s) => json_string(s, out),
            Value::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.json(out);
                }
                out.push(']');
            }
            Value::Fields(fields) => {
                out.push('{');
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    json_string(name, out);
                    out.push(':');
                    value.json(out);
                }
                out.push('}');
            }
            Value::Bytes(bytes) => {
                out.push_str("{\"bytes\":");
                json_string(&hex(bytes), out);
                out.push('}');
            }
            Value::Binder(i) => out.push_str(&format!("{{\"binder\":{i}}}")),
            Value::File(i) => out.push_str(&format!("{{\"file\":{i}}}")),
            Value::Exception { code, message } => {
                out.push_str(&format!("{{\"exception\":{code},\"message\":"));
                json_string(message, out);
                out.push('}');
            }
            Value::Status(s) => out.push_str(&format!("{{\"status\":{s}}}")),
            Value::Slice {
                count,
                creator,
                items,
            } => {
                out.push_str(&format!("{{\"slice\":{count},\"creator\":"));
                match creator {
                    Some(c) => json_string(c, out),
                    None => out.push_str("null"),
                }
                out.push_str(",\"items\":");
                json_string(&hex(items), out);
                out.push('}');
            }
        }
    }
}

pub(crate) fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A generated reader's result as a [`Value`].
pub trait IntoValue {
    fn into_value(self) -> Value;
}

macro_rules! scalar {
    ($($t:ty => $v:ident),*) => {
        $(impl IntoValue for $t {
            fn into_value(self) -> Value {
                Value::$v(self)
            }
        })*
    };
}

scalar!(bool => Bool, i32 => Int, i64 => Long, String => Str);

impl IntoValue for f32 {
    fn into_value(self) -> Value {
        Value::Float(self.to_bits())
    }
}

impl IntoValue for () {
    fn into_value(self) -> Value {
        Value::Null
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        self.map_or(Value::Null, T::into_value)
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::List(self.into_iter().map(T::into_value).collect())
    }
}

impl<T: IntoValue> IntoValue for Returned<T> {
    fn into_value(self) -> Value {
        match self {
            Ok(v) => v.into_value(),
            Err(e) => e.into_value(),
        }
    }
}

impl IntoValue for Exception {
    fn into_value(self) -> Value {
        Value::Exception {
            code: self.code,
            message: self.message,
        }
    }
}

impl IntoValue for Binder {
    /// After the framework's rewrite every binder is a handle, its
    /// identity's index.
    fn into_value(self) -> Value {
        match self {
            Binder::Handle(i) => Value::Binder(i),
            Binder::Local(_) => Value::Binder(u32::MAX),
        }
    }
}

/// A reply read with a generated reader, as a [`Value`]: what a model's
/// `decode_reply` returns for a method, e.g.
/// `decode(reply, pm::read_get_packages_for_uid_reply)`.
pub fn decode<T: IntoValue>(
    reply: &mut Reader<'_>,
    read: impl FnOnce(&mut Reader<'_>) -> Result<T>,
) -> Result<Value> {
    read(reply).map(IntoValue::into_value)
}

/// The rest of a parcel, uninterpreted: a parcelable without a reader.
#[derive(Clone, Debug, PartialEq)]
pub struct Opaque(pub Vec<u8>);

impl ReadParcelable for Opaque {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self(rest(r)?))
    }
}

impl IntoValue for Opaque {
    fn into_value(self) -> Value {
        Value::Bytes(self.0)
    }
}

fn rest(r: &mut Reader<'_>) -> Result<Vec<u8>> {
    let at = r.position();
    r.skip(r.remaining())?;
    Ok(r.since(at).0.to_vec())
}

/// A `ParceledListSlice` read whole: its count, its items' class (the
/// `CREATOR`'s) and their bytes (`1` and the item, for each). The
/// framework stitches the items an original fetched through the slice's
/// binder into the reply first, so it is always the rest of the reply,
/// as every method returning one returns nothing after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub count: i32,
    pub creator: Option<String>,
    pub items: Vec<u8>,
}

impl ReadParcelable for Slice {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let count = r.read_i32()?;
        if count <= 0 {
            return Ok(Self {
                count,
                creator: None,
                items: Vec::new(),
            });
        }
        Ok(Self {
            count,
            creator: r.read_string16()?,
            items: rest(r)?,
        })
    }
}

impl IntoValue for Slice {
    fn into_value(self) -> Value {
        Value::Slice {
            count: self.count,
            creator: self.creator,
            items: self.items,
        }
    }
}

/// A model's `ParceledListSlice`, every item inline (`BaseParceledListSlice`
/// writes as many as fit and hands out a binder for the rest).
pub struct ListSlice<T> {
    /// The items' class name (`writeParcelableCreator`).
    pub creator: String,
    pub items: Vec<T>,
}

impl<T: WriteParcelable> WriteParcelable for ListSlice<T> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.items.len() as i32);
        if self.items.is_empty() {
            return;
        }
        p.write_string16(Some(&self.creator));
        for item in &self.items {
            p.write_i32(1);
            item.write_to(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_service_aidl::{read_typed, write_typed};

    struct Item(i32);

    impl WriteParcelable for Item {
        fn write_to(&self, p: &mut Parcel) {
            p.write_i32(self.0);
        }
    }

    #[test]
    fn a_list_slice_reads_back_as_a_slice() {
        let mut p = Parcel::new();
        let slice = ListSlice {
            creator: "android.content.pm.PackageInfo".into(),
            items: vec![Item(5), Item(6)],
        };
        write_typed(&mut p, Some(&slice));
        let mut r = Reader::new(p.data(), p.objects());
        let read: Option<Slice> = read_typed(&mut r).unwrap();
        assert_eq!(
            read.into_value(),
            Value::Slice {
                count: 2,
                creator: Some("android.content.pm.PackageInfo".into()),
                items: [1, 5, 1, 6]
                    .iter()
                    .flat_map(|v: &i32| v.to_le_bytes())
                    .collect(),
            }
        );

        let mut p = Parcel::new();
        ListSlice::<Item> {
            creator: "x".into(),
            items: Vec::new(),
        }
        .write_to(&mut p);
        assert_eq!(p.data(), 0i32.to_le_bytes());
    }

    #[test]
    fn values_are_json() {
        let value = Value::Fields(vec![
            ("name".into(), Value::Str("a\"b".into())),
            (
                "list".into(),
                Value::List(vec![Value::Int(-1), Value::Null, Value::Binder(2)]),
            ),
            ("raw".into(), Value::Bytes(vec![0, 255])),
            ("thrown".into(), Exception::security("no").into_value()),
        ]);
        let mut out = String::new();
        value.json(&mut out);
        assert_eq!(
            out,
            r#"{"name":"a\"b","list":[-1,null,{"binder":2}],"raw":{"bytes":"00ff"},"thrown":{"exception":-1,"message":"no"}}"#
        );
    }
}
