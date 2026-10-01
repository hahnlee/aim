//! A manifest element's attributes as the parser reads them: through
//! `Resources.obtainAttributes` (`RetrieveAttributes`, each value followed
//! through the resources) and `TypedArray`'s getters, or by name through
//! `XmlResourceParser`.
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`,
//! `android.content.res.TypedArray`, `android.util.TypedValue`,
//! `com.android.internal.util.XmlUtils`, `libs/androidfw`), Copyright (C)
//! The Android Open Source Project, Licensed under the Apache License,
//! Version 2.0.

use std::cell::RefCell;
use std::collections::HashMap;

use aim_apps::res::{Element, Value as XmlValue};

use super::resources::{
    DATA_NULL_EMPTY, Resources, Selected, TYPE_ATTRIBUTE, TYPE_DIMENSION, TYPE_DYNAMIC_REFERENCE,
    TYPE_FIRST_COLOR_INT, TYPE_FIRST_INT, TYPE_FLOAT, TYPE_FRACTION, TYPE_INT_BOOLEAN,
    TYPE_INT_HEX, TYPE_LAST_COLOR_INT, TYPE_LAST_INT, TYPE_NULL, TYPE_REFERENCE, TYPE_STRING,
    runtime_id,
};

/// `ParsingUtils.ANDROID_RES_NAMESPACE`.
pub const ANDROID: &str = "http://schemas.android.com/apk/res/android";

const TYPE_DYNAMIC_ATTRIBUTE: u8 = 0x08;

/// A value of an attribute, resolved (`TypedValue`).
#[derive(Clone, Debug)]
pub struct Value {
    pub kind: u8,
    pub data: u32,
    pub resource_id: u32,
    /// Native configuration-change flags.
    flags: u32,
    /// The string of a `TYPE_STRING`, and whether it is the XML's own.
    pub string: Option<String>,
    from_xml: bool,
}

impl Value {
    /// `TypedValue.coerceToString`.
    pub fn coerce_to_string(&self) -> Option<String> {
        if self.kind == TYPE_STRING {
            return self.string.clone();
        }
        coerce_to_string(self.kind, self.data)
    }
}

/// `TypedValue.coerceToString(type, data)`.
pub fn coerce_to_string(kind: u8, data: u32) -> Option<String> {
    const DIMENSION_UNITS: [&str; 6] = ["px", "dip", "sp", "pt", "in", "mm"];
    const FRACTION_UNITS: [&str; 2] = ["%", "%p"];
    let unit = (data & 0xf) as usize;
    Some(match kind {
        TYPE_NULL => return None,
        TYPE_REFERENCE => format!("@{}", data as i32),
        TYPE_ATTRIBUTE => format!("?{}", data as i32),
        TYPE_FLOAT => java_float(f32::from_bits(data)),
        TYPE_DIMENSION => format!(
            "{}{}",
            java_float(complex_to_float(data)),
            DIMENSION_UNITS.get(unit)?
        ),
        TYPE_FRACTION => format!(
            "{}{}",
            java_float(complex_to_float(data) * 100.0),
            FRACTION_UNITS.get(unit)?
        ),
        TYPE_INT_HEX => format!("0x{data:x}"),
        TYPE_INT_BOOLEAN => (if data != 0 { "true" } else { "false" }).to_owned(),
        TYPE_FIRST_COLOR_INT..=TYPE_LAST_COLOR_INT => format!("#{data:x}"),
        TYPE_FIRST_INT..=TYPE_LAST_INT => (data as i32).to_string(),
        _ => return None,
    })
}

/// `TypedValue.complexToFloat`.
fn complex_to_float(data: u32) -> f32 {
    const RADIX: [f32; 4] = [
        1.0 / (1u64 << 8) as f32,
        1.0 / (1u64 << 15) as f32,
        1.0 / (1u64 << 23) as f32,
        1.0 / (1u64 << 31) as f32,
    ];
    (data & 0xffff_ff00) as i32 as f32 * RADIX[((data >> 4) & 3) as usize]
}

/// `Float.toString`: the shortest digits that read back, as a decimal
/// between 10^-3 and 10^7, else in computerized scientific notation.
pub fn java_float(f: f32) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let a = f.abs();
    if (1e-3..1e7).contains(&a) {
        let s = format!("{f}");
        if s.contains('.') { s } else { s + ".0" }
    } else {
        let s = format!("{f:e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = if m.contains('.') {
            m.to_owned()
        } else {
            format!("{m}.0")
        };
        format!("{m}E{e}")
    }
}

/// `XmlUtils.convertValueToBoolean`.
fn convert_to_boolean(v: Option<&str>, default: bool) -> bool {
    match v {
        None | Some("") => default,
        Some(s) => s == "1" || s == "true" || s == "TRUE",
    }
}

/// `XmlUtils.convertValueToInt`; `None` where `Integer.parseInt` throws.
fn convert_to_int(v: Option<&str>, default: i32) -> Option<i32> {
    let Some(s) = v.filter(|s| !s.is_empty()) else {
        return Some(default);
    };
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1, s),
    };
    let (base, digits) = if let Some(r) = rest.strip_prefix('0') {
        if r.is_empty() {
            return Some(0);
        }
        match r.strip_prefix(['x', 'X']) {
            Some(h) => (16, h),
            None => (8, r),
        }
    } else if let Some(h) = rest.strip_prefix('#') {
        (16, h)
    } else {
        (10, rest)
    };
    let v = i64::from_str_radix(digits, base).ok()?;
    i32::try_from(v).ok().map(|v| (v as i64 * sign) as i32)
}

/// `ActivityInfo.CONFIG_NATIVE_BITS`, by Java bit.
const CONFIG_NATIVE_BITS: [u32; 16] = [
    0x0002, 0x0001, 0x0004, 0x0008, 0x0010, 0x0020, 0x0040, 0x0080, 0x0800, 0x1000, 0x0200, 0x2000,
    0x0100, 0x4000, 0x10000, 0x20000,
];

/// `ActivityInfo.activityInfoConfigNativeToJava`.
fn native_to_java(native: u32) -> u32 {
    CONFIG_NATIVE_BITS
        .iter()
        .enumerate()
        .filter(|&(_, &b)| native & b != 0)
        .fold(0, |out, (i, _)| out | 1 << i)
}

/// `Configuration.NATIVE_CONFIG_VERSION`, as the parser passes it.
pub const NATIVE_CONFIG_VERSION: u32 = 0x0400;

/// What reading attributes needs: the resources, the framework's attribute
/// ids by name, and the first error a getter hit (Java throws there).
pub struct Ctx<'a> {
    pub res: &'a Resources<'a>,
    pub attrs: &'a HashMap<String, u32>,
    pub error: RefCell<Option<String>>,
}

impl Ctx<'_> {
    fn fail(&self, what: String) {
        self.error.borrow_mut().get_or_insert(what);
    }

    /// `android:<name>`'s resource id.
    pub fn attr_id(&self, name: &str) -> u32 {
        match self.attrs.get(name) {
            Some(&id) => id,
            None => {
                self.fail(format!("no attribute android:{name}"));
                0
            }
        }
    }

    /// The typed array of `e` for the attributes the parser asks of it.
    pub fn obtain<'e>(&'e self, e: &'e Element) -> TypedArray<'e> {
        TypedArray { ctx: self, e }
    }
}

/// `TypedArray` over one element, by attribute name.
pub struct TypedArray<'a> {
    ctx: &'a Ctx<'a>,
    e: &'a Element,
}

impl TypedArray<'_> {
    /// `RetrieveAttributes` for one attribute: its value followed through
    /// the resources; `None` for no value.
    fn value(&self, name: &str) -> Value {
        let id = self.ctx.attr_id(name);
        let null = Value {
            kind: TYPE_NULL,
            data: 0,
            resource_id: 0,
            flags: 0,
            string: None,
            from_xml: false,
        };
        let Some(a) = self.e.attrs.iter().find(|a| a.id == id && id != 0) else {
            return null;
        };
        let kind = match a.kind {
            TYPE_DYNAMIC_REFERENCE => TYPE_REFERENCE,
            TYPE_DYNAMIC_ATTRIBUTE => TYPE_ATTRIBUTE,
            k => k,
        };
        // `ResXMLParser::getAttributeValue`: through the XML's
        // `DynamicRefTable`.
        let data = match kind {
            TYPE_REFERENCE | TYPE_ATTRIBUTE => runtime_id(a.data),
            _ => a.data,
        };
        let mut v = Selected {
            kind,
            data,
            table: None,
            resid: 0,
            flags: 0,
        };
        if v.kind != TYPE_NULL {
            self.ctx.res.resolve(&mut v);
        }
        if v.kind == TYPE_REFERENCE && v.data == 0 {
            return null;
        }
        let string = match (v.kind, v.table) {
            (TYPE_STRING, None) => match &a.value {
                XmlValue::String(s) => Some(s.clone()),
                _ => None,
            },
            (TYPE_STRING, Some(t)) => self.ctx.res.string(t, v.data).map(str::to_owned),
            _ => None,
        };
        Value {
            kind: v.kind,
            data: v.data,
            resource_id: v.resid,
            flags: v.flags,
            string,
            from_xml: v.table.is_none(),
        }
    }

    /// `peekValue`.
    pub fn peek(&self, name: &str) -> Option<Value> {
        Some(self.value(name)).filter(|v| v.kind != TYPE_NULL)
    }

    /// `getType`.
    pub fn kind(&self, name: &str) -> u8 {
        self.value(name).kind
    }

    pub fn has_value(&self, name: &str) -> bool {
        self.value(name).kind != TYPE_NULL
    }

    pub fn has_value_or_empty(&self, name: &str) -> bool {
        let v = self.value(name);
        v.kind != TYPE_NULL || v.data == DATA_NULL_EMPTY
    }

    /// `getString`.
    pub fn string(&self, name: &str) -> Option<String> {
        self.value(name).coerce_to_string()
    }

    /// `getNonResourceString`: a string written in the manifest itself.
    pub fn non_resource_string(&self, name: &str) -> Option<String> {
        let v = self.value(name);
        (v.kind == TYPE_STRING && v.from_xml)
            .then_some(v.string)
            .flatten()
    }

    /// `getNonConfigurationString`: none for a value that depends on a
    /// configuration other than `allowed` (Java's bits).
    pub fn non_config_string(&self, name: &str, allowed: u32) -> Option<String> {
        let v = self.value(name);
        if native_to_java(v.flags) & !allowed != 0 {
            return None;
        }
        v.coerce_to_string()
    }

    /// `getBoolean`.
    pub fn boolean(&self, name: &str, default: bool) -> bool {
        let v = self.value(name);
        match v.kind {
            TYPE_NULL => default,
            TYPE_FIRST_INT..=TYPE_LAST_INT => v.data != 0,
            _ => convert_to_boolean(v.coerce_to_string().as_deref(), default),
        }
    }

    /// `getInt`.
    pub fn int(&self, name: &str, default: i32) -> i32 {
        let v = self.value(name);
        match v.kind {
            TYPE_NULL => default,
            TYPE_FIRST_INT..=TYPE_LAST_INT => v.data as i32,
            _ => {
                let s = v.coerce_to_string();
                convert_to_int(s.as_deref(), default).unwrap_or_else(|| {
                    self.ctx.fail(format!("android:{name}: not an int: {s:?}"));
                    default
                })
            }
        }
    }

    /// `getInteger`.
    pub fn integer(&self, name: &str, default: i32) -> i32 {
        let v = self.value(name);
        match v.kind {
            TYPE_NULL => default,
            TYPE_FIRST_INT..=TYPE_LAST_INT => v.data as i32,
            k => {
                self.ctx.fail(format!(
                    "android:{name}: can't convert type {k:#x} to integer"
                ));
                default
            }
        }
    }

    /// `getFloat`.
    pub fn float(&self, name: &str, default: f32) -> f32 {
        let v = self.value(name);
        match v.kind {
            TYPE_NULL => default,
            TYPE_FLOAT => f32::from_bits(v.data),
            TYPE_FIRST_INT..=TYPE_LAST_INT => v.data as i32 as f32,
            _ => match v.coerce_to_string().map(|s| s.trim().parse::<f32>()) {
                Some(Ok(f)) => f,
                other => {
                    self.ctx
                        .fail(format!("android:{name}: not a float: {other:?}"));
                    default
                }
            },
        }
    }

    /// `getDimensionPixelSize` of a size in pixels, density-independent
    /// pixels or scaled pixels, at `density` (`DisplayMetrics.density`;
    /// the parser's configuration has no font scale); `None` for another
    /// unit, which needs the display's physical dpi.
    pub fn dimension_pixel_size(&self, name: &str, default: i32, density: f32) -> Option<i32> {
        let v = self.value(name);
        match v.kind {
            TYPE_NULL => Some(default),
            TYPE_DIMENSION => {
                let value = complex_to_float(v.data);
                let f = match v.data & 0xf {
                    0 => value,
                    1 | 2 => value * density,
                    _ => return None,
                };
                let res = if f >= 0.0 { f + 0.5 } else { f - 0.5 } as i32;
                Some(match res {
                    0 if value == 0.0 => 0,
                    0 if value > 0.0 => 1,
                    0 => -1,
                    r => r,
                })
            }
            k => {
                self.ctx.fail(format!(
                    "android:{name}: can't convert type {k:#x} to dimension"
                ));
                Some(default)
            }
        }
    }

    /// `getFraction(index, 1, 1, default)`.
    pub fn fraction(&self, name: &str, default: f32) -> f32 {
        let v = self.value(name);
        match v.kind {
            TYPE_NULL => default,
            TYPE_FRACTION => complex_to_float(v.data),
            k => {
                self.ctx.fail(format!(
                    "android:{name}: can't convert type {k:#x} to fraction"
                ));
                default
            }
        }
    }

    /// `getResourceId`.
    pub fn resource_id(&self, name: &str, default: i32) -> i32 {
        let v = self.value(name);
        if v.kind != TYPE_NULL && v.resource_id != 0 {
            v.resource_id as i32
        } else {
            default
        }
    }
}

/// `XmlResourceParser.getAttributeValue(ns, name)`: the raw string, or the
/// typed value coerced to one.
pub fn attr_value(e: &Element, ns: &str, name: &str) -> Option<String> {
    let a = e.attrs.iter().find(|a| a.ns == ns && a.name == name)?;
    match &a.value {
        XmlValue::String(s) => Some(s.clone()),
        _ => coerce_to_string(a.kind, a.data),
    }
}

/// `XmlResourceParser.getAttributeBooleanValue(ns, name, default)`.
pub fn attr_bool(e: &Element, ns: &str, name: &str, default: bool) -> bool {
    match e.attrs.iter().find(|a| a.ns == ns && a.name == name) {
        Some(a) if (TYPE_FIRST_INT..=TYPE_LAST_INT).contains(&a.kind) => a.data != 0,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_as_java() {
        assert_eq!(java_float(1.0), "1.0");
        assert_eq!(java_float(1.86), "1.86");
        assert_eq!(java_float(2.5e7), "2.5E7");
        assert_eq!(convert_to_int(Some("0x10"), 0), Some(16));
        assert_eq!(convert_to_int(Some("-010"), 0), Some(-8));
        assert_eq!(convert_to_int(Some("x"), 0), None);
        assert_eq!(coerce_to_string(TYPE_INT_HEX, 0x1f), Some("0x1f".into()));
        // Locale (native 0x4) is Java bit 2; screen size (0x200) bit 10.
        assert_eq!(native_to_java(0x204), 1 << 2 | 1 << 10);
    }
}
