//! The framework AIDL of the native system services (ADR 0013), as Rust:
//! part of `aidl-gen`.
//!
//! The platform's Java AIDL interfaces (IClipboard, IActivityManager, ...)
//! have no NDK or Rust backend: their methods take Java-only parcelables.
//! This generator reads the pinned `.aidl` files of
//! `crates/aim-services/sources.lock` and writes, per interface, its
//! descriptor, every method's transaction code, and for the methods the
//! lock selects the parcel (de)serialization of their arguments and
//! replies over `aim_binder_host::parcel`. A Java-only parcelable is a type
//! parameter implementing `ReadParcelable` or `WriteParcelable`, which its
//! owner provides.
//!
//! The codes follow AIDL's rule (first call transaction plus the method's
//! index, or its explicit id) and are then checked against the image
//! itself: the `TRANSACTION_*` constants of each interface's Java stub in
//! the pinned image's jars, all of them, so a code cannot drift. The few
//! binder interfaces the framework writes by hand (`IContentProvider`)
//! have no AIDL: the codes a native service calls are read from the
//! interface's own constants in the image (`CONSTANTS`).

use crate::fetch::Source;
use crate::lockfile::Lock;
use crate::log::Log;
use aim_android_image::dex::{Dex, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

pub const LOCK: &str = "crates/aim-services/sources.lock";

/// `android.os.IBinder.FIRST_CALL_TRANSACTION`.
const FIRST_CALL: u32 = 1;

pub fn out() -> PathBuf {
    aim_paths::generated().join("service-aidl")
}

#[derive(Debug, Clone, PartialEq)]
struct Type {
    name: String,
    args: Vec<Type>,
    array: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Param {
    direction: String,
    ty: Type,
    name: String,
}

#[derive(Debug, Clone, PartialEq)]
struct Method {
    name: String,
    oneway: bool,
    ret: Type,
    params: Vec<Param>,
    code: u32,
}

#[derive(Debug, PartialEq)]
struct Interface {
    package: String,
    name: String,
    oneway: bool,
    methods: Vec<Method>,
}

impl Interface {
    fn descriptor(&self) -> String {
        format!("{}.{}", self.package, self.name)
    }
}

fn tokens(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if text[char_offset(&chars, i)..].starts_with("//") {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if text[char_offset(&chars, i)..].starts_with("/*") {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
            out.push(chars[start..i.min(chars.len())].iter().collect());
        } else if c.is_alphanumeric() || c == '_' || c == '.' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else {
            out.push(c.to_string());
            i += 1;
        }
    }
    Ok(out)
}

fn char_offset(chars: &[char], i: usize) -> usize {
    chars[..i].iter().map(|c| c.len_utf8()).sum()
}

struct Parser {
    t: Vec<String>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> &str {
        self.t.get(self.i).map_or("", String::as_str)
    }

    fn next(&mut self) -> Result<String, String> {
        let t = self.t.get(self.i).cloned().ok_or("unexpected end")?;
        self.i += 1;
        Ok(t)
    }

    fn expect(&mut self, want: &str) -> Result<(), String> {
        let got = self.next()?;
        if got != want {
            return Err(format!("expected `{want}`, found `{got}`"));
        }
        Ok(())
    }

    /// Skips annotations (`@Name` with optional balanced arguments).
    fn annotations(&mut self) -> Result<(), String> {
        while self.peek() == "@" {
            self.i += 2;
            if self.peek() == "(" {
                self.balanced("(", ")")?;
            }
        }
        Ok(())
    }

    fn balanced(&mut self, open: &str, close: &str) -> Result<(), String> {
        self.expect(open)?;
        let mut depth = 1;
        while depth > 0 {
            let t = self.next()?;
            if t == open {
                depth += 1;
            } else if t == close {
                depth -= 1;
            }
        }
        Ok(())
    }

    fn ty(&mut self) -> Result<Type, String> {
        self.annotations()?;
        let name = self.next()?;
        let mut args = Vec::new();
        if self.peek() == "<" {
            self.i += 1;
            loop {
                args.push(self.ty()?);
                match self.next()?.as_str() {
                    "," => continue,
                    ">" => break,
                    other => return Err(format!("bad type argument list at `{other}`")),
                }
            }
        }
        let mut array = false;
        while self.peek() == "[" {
            self.expect("[")?;
            self.expect("]")?;
            array = true;
        }
        Ok(Type { name, args, array })
    }
}

fn parse(text: &str) -> Result<Interface, String> {
    let mut p = Parser {
        t: tokens(text)?,
        i: 0,
    };
    let mut package = String::new();
    loop {
        p.annotations()?;
        match p.peek() {
            "package" => {
                p.i += 1;
                package = p.next()?;
                p.expect(";")?;
            }
            "import" => while p.next()? != ";" {},
            "oneway" | "interface" => break,
            other => return Err(format!("expected an interface, found `{other}`")),
        }
    }
    let oneway = p.peek() == "oneway";
    if oneway {
        p.i += 1;
    }
    p.expect("interface")?;
    let name = p.next()?;
    p.expect("{")?;
    let mut methods = Vec::new();
    loop {
        p.annotations()?;
        match p.peek() {
            "}" => break,
            "const" => while p.next()? != ";" {},
            "parcelable" | "enum" | "union" | "interface" => {
                while p.peek() != "{" {
                    p.i += 1;
                }
                p.balanced("{", "}")?;
            }
            _ => {
                let method_oneway = p.peek() == "oneway";
                if method_oneway {
                    p.i += 1;
                }
                let ret = p.ty()?;
                let name = p.next()?;
                p.expect("(")?;
                let mut params = Vec::new();
                while p.peek() != ")" {
                    p.annotations()?;
                    // Empty when implicit (`in`).
                    let direction = match p.peek() {
                        "in" | "out" | "inout" => p.next()?,
                        _ => String::new(),
                    };
                    let ty = p.ty()?;
                    params.push(Param {
                        direction,
                        ty,
                        name: p.next()?,
                    });
                    if p.peek() == "," {
                        p.i += 1;
                    }
                }
                p.expect(")")?;
                let code = if p.peek() == "=" {
                    p.i += 1;
                    let id: u32 = p.next()?.parse().map_err(|_| "bad transaction id")?;
                    FIRST_CALL + id
                } else {
                    FIRST_CALL + methods.len() as u32
                };
                p.expect(";")?;
                methods.push(Method {
                    name,
                    oneway: oneway || method_oneway,
                    ret,
                    params,
                    code,
                });
            }
        }
    }
    Ok(Interface {
        package,
        name,
        oneway,
        methods,
    })
}

fn snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        let lower_before = i > 0 && chars[i - 1].is_lowercase();
        let upper_run_ends = i > 0
            && chars[i - 1].is_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if c.is_uppercase() && (lower_before || upper_run_ends) {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    const KEYWORDS: [&str; 12] = [
        "type", "in", "ref", "match", "mod", "fn", "use", "loop", "box", "where", "move", "self",
    ];
    if KEYWORDS.contains(&out.as_str()) {
        format!("r#{out}")
    } else {
        out
    }
}

/// A method's name as a type: its words capitalized, without the
/// underscores some names have (`checkGrantUriPermission_ignoreNonSystem`).
fn camel(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

/// How a type is marshalled.
enum Kind {
    Void,
    /// Rust type, read expression, write statement (with `{v}` the value).
    Plain(&'static str, &'static str, &'static str),
    /// A Java-only parcelable, as the named type parameter.
    Parcelable(String),
}

fn kind(ty: &Type) -> Result<Kind, String> {
    let base = ty.name.rsplit('.').next().unwrap_or(&ty.name);
    let string_list = (ty.name == "String" && ty.array)
        || (ty.name == "List" && ty.args.first().is_some_and(|a| a.name == "String"));
    Ok(match (base, ty.array) {
        ("void", false) => Kind::Void,
        _ if string_list => Kind::Plain(
            "Option<Vec<Option<String>>>",
            "read_string_list(r)?",
            "write_string_list(p, {v}.as_deref());",
        ),
        ("int", true) => Kind::Plain(
            "Option<Vec<i32>>",
            "read_int_array(r)?",
            "write_int_array(p, {v}.as_deref());",
        ),
        ("int", false) => Kind::Plain("i32", "r.read_i32()?", "p.write_i32({v});"),
        ("long", false) => Kind::Plain("i64", "r.read_i64()?", "p.write_i64({v});"),
        ("float", false) => Kind::Plain("f32", "r.read_f32()?", "p.write_f32({v});"),
        ("boolean", false) => Kind::Plain("bool", "r.read_bool()?", "p.write_bool({v});"),
        ("String", false) => Kind::Plain(
            "Option<String>",
            "r.read_string16()?",
            "p.write_string16({v}.as_deref());",
        ),
        ("IBinder", false) => {
            Kind::Plain("Option<Binder>", "r.read_binder()?", "p.write_binder({v});")
        }
        (b, false)
            if b.len() > 1
                && b.starts_with('I')
                && b.chars().nth(1).is_some_and(char::is_uppercase)
                && ty.args.is_empty() =>
        {
            Kind::Plain("Option<Binder>", "r.read_binder()?", "p.write_binder({v});")
        }
        (b, false) if ty.args.is_empty() && b.chars().next().is_some_and(char::is_uppercase) => {
            Kind::Parcelable(b.to_string())
        }
        _ => return Err(format!("type `{}` is not supported", ty.name)),
    })
}

fn signature(m: &Method) -> String {
    let ty = |t: &Type| {
        let mut s = t.name.clone();
        if !t.args.is_empty() {
            let args: Vec<String> = t.args.iter().map(|a| a.name.clone()).collect();
            s += &format!("<{}>", args.join(", "));
        }
        if t.array {
            s += "[]";
        }
        s
    };
    let params: Vec<String> = m
        .params
        .iter()
        .map(|p| {
            format!("{} {} {}", p.direction, ty(&p.ty), p.name)
                .trim_start()
                .to_string()
        })
        .collect();
    format!(
        "{}{} {}({})",
        if m.oneway { "oneway " } else { "" },
        ty(&m.ret),
        m.name,
        params.join(", ")
    )
}

/// The Rust of one method: its argument struct and reply functions.
fn method_code(m: &Method) -> Result<String, String> {
    let mut generics = Vec::new();
    let mut fields = Vec::new();
    for p in &m.params {
        if !matches!(p.direction.as_str(), "" | "in") {
            return Err(format!(
                "{}: `{}` parameters are not supported",
                m.name, p.direction
            ));
        }
        match kind(&p.ty)? {
            Kind::Void => return Err(format!("{}: void parameter", m.name)),
            Kind::Plain(ty, read, write) => fields.push((
                snake(&p.name),
                ty.to_string(),
                read.to_string(),
                write.to_string(),
            )),
            Kind::Parcelable(t) => {
                if !generics.contains(&t) {
                    generics.push(t.clone());
                }
                fields.push((
                    snake(&p.name),
                    format!("Option<{t}>"),
                    "read_typed(r)?".into(),
                    "write_typed(p, {v}.as_ref());".into(),
                ));
            }
        }
    }
    let params = if generics.is_empty() {
        String::new()
    } else {
        format!("<{}>", generics.join(", "))
    };
    let bounds = |t: &str| {
        if generics.is_empty() {
            String::new()
        } else {
            let b: Vec<String> = generics.iter().map(|g| format!("{g}: {t}")).collect();
            format!("<{}>", b.join(", "))
        }
    };
    let name = camel(&m.name);
    let mut s = String::new();
    writeln!(s, "    /// `{}`", signature(m)).unwrap();
    writeln!(s, "    #[derive(Debug)]").unwrap();
    writeln!(s, "    pub struct {name}{params} {{").unwrap();
    for (field, ty, _, _) in &fields {
        writeln!(s, "        pub {field}: {ty},").unwrap();
    }
    writeln!(s, "    }}\n").unwrap();
    writeln!(s, "    impl{} {name}{params} {{", bounds("ReadParcelable")).unwrap();
    writeln!(
        s,
        "        /// A call's data: its interface token, then the arguments."
    )
    .unwrap();
    writeln!(
        s,
        "        pub fn read(r: &mut Reader<'_>) -> Result<Self> {{"
    )
    .unwrap();
    writeln!(s, "            r.enforce_interface(DESCRIPTOR)?;").unwrap();
    writeln!(s, "            Ok(Self {{").unwrap();
    for (field, _, read, _) in &fields {
        writeln!(s, "                {field}: {read},").unwrap();
    }
    writeln!(s, "            }})\n        }}\n    }}\n").unwrap();
    writeln!(s, "    impl{} {name}{params} {{", bounds("WriteParcelable")).unwrap();
    writeln!(s, "        pub fn write(&self, p: &mut Parcel) {{").unwrap();
    writeln!(s, "            p.write_interface_token(DESCRIPTOR);").unwrap();
    for (field, _, _, write) in &fields {
        writeln!(
            s,
            "            {}",
            write.replace("{v}", &format!("self.{field}"))
        )
        .unwrap();
    }
    writeln!(s, "        }}\n    }}\n").unwrap();
    if !m.oneway {
        let fname = snake(&m.name);
        match kind(&m.ret)? {
            Kind::Void => {
                writeln!(s, "    pub fn write_{fname}_reply(p: &mut Parcel) {{").unwrap();
                writeln!(s, "        p.write_no_exception();\n    }}\n").unwrap();
                writeln!(
                    s,
                    "    pub fn read_{fname}_reply(r: &mut Reader<'_>) -> Result<Returned<()>> {{"
                )
                .unwrap();
                writeln!(s, "        r.read_exception()\n    }}\n").unwrap();
            }
            Kind::Plain(ty, read, write) => {
                let arg = if ty.starts_with("Option<Vec") || ty == "Option<String>" {
                    format!("result: &{ty}")
                } else {
                    format!("result: {ty}")
                };
                writeln!(
                    s,
                    "    pub fn write_{fname}_reply(p: &mut Parcel, {arg}) {{"
                )
                .unwrap();
                writeln!(s, "        p.write_no_exception();").unwrap();
                writeln!(s, "        {}\n    }}\n", write.replace("{v}", "result")).unwrap();
                writeln!(
                    s,
                    "    pub fn read_{fname}_reply(r: &mut Reader<'_>) -> Result<Returned<{ty}>> {{"
                )
                .unwrap();
                writeln!(s, "        Ok(match r.read_exception()? {{").unwrap();
                writeln!(s, "            Ok(()) => Ok({read}),").unwrap();
                writeln!(s, "            Err(e) => Err(e),\n        }})\n    }}\n").unwrap();
            }
            Kind::Parcelable(t) => {
                writeln!(s, "    pub fn write_{fname}_reply<{t}: WriteParcelable>(p: &mut Parcel, result: Option<&{t}>) {{").unwrap();
                writeln!(s, "        p.write_no_exception();").unwrap();
                writeln!(s, "        write_typed(p, result);\n    }}\n").unwrap();
                writeln!(s, "    pub fn read_{fname}_reply<{t}: ReadParcelable>(r: &mut Reader<'_>) -> Result<Returned<Option<{t}>>> {{").unwrap();
                writeln!(s, "        Ok(match r.read_exception()? {{").unwrap();
                writeln!(s, "            Ok(()) => Ok(read_typed(r)?),").unwrap();
                writeln!(s, "            Err(e) => Err(e),\n        }})\n    }}\n").unwrap();
            }
        }
    }
    Ok(s)
}

fn module_name(descriptor: &str) -> String {
    descriptor.replace('.', "_").to_lowercase()
}

fn interface_code(iface: &Interface, selected: &[String], origin: &str) -> Result<String, String> {
    let mut s = String::new();
    let descriptor = iface.descriptor();
    writeln!(s, "/// `{descriptor}` ({origin})").unwrap();
    writeln!(s, "pub mod {} {{", module_name(&descriptor)).unwrap();
    writeln!(s, "    #[allow(unused_imports)]\n    use super::*;\n").unwrap();
    writeln!(s, "    pub const DESCRIPTOR: &str = \"{descriptor}\";\n").unwrap();
    for m in &iface.methods {
        writeln!(
            s,
            "    pub const {}: u32 = {};",
            snake(&m.name).trim_start_matches("r#").to_uppercase(),
            m.code
        )
        .unwrap();
    }
    writeln!(s, "\n    /// Every method's code and name.").unwrap();
    writeln!(s, "    pub const METHODS: &[(u32, &str)] = &[").unwrap();
    for m in &iface.methods {
        writeln!(s, "        ({}, \"{}\"),", m.code, m.name).unwrap();
    }
    writeln!(s, "    ];\n").unwrap();
    let all = selected.iter().any(|m| m == "*");
    for name in selected.iter().filter(|m| *m != "*") {
        if !iface.methods.iter().any(|m| &m.name == name) {
            return Err(format!("{descriptor} has no method `{name}`"));
        }
    }
    for m in iface
        .methods
        .iter()
        .filter(|m| all || selected.contains(&m.name))
    {
        s += &method_code(m).map_err(|e| format!("{descriptor}.{e}"))?;
    }
    writeln!(s, "}}\n").unwrap();
    Ok(s)
}

const PRELUDE: &str = r#"// @generated by `cargo aim` (aidl-gen, crates/aim-build/src/nodes/service_aidl.rs)
// from the pinned AIDL of crates/aim-services/sources.lock. Do not edit.

#![allow(clippy::all, dead_code)]

use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, Result};

/// A reply: the callee's result, or the exception it threw.
pub type Returned<T> = std::result::Result<T, Exception>;

/// A Java-only parcelable read as its `CREATOR` reads it.
pub trait ReadParcelable: Sized {
    fn read_from(r: &mut Reader<'_>) -> Result<Self>;
}

/// A Java-only parcelable written as its `writeToParcel` writes it.
pub trait WriteParcelable {
    fn write_to(&self, p: &mut Parcel);
}

/// `readTypedObject`: 0 for null, else 1 and the object.
pub fn read_typed<T: ReadParcelable>(r: &mut Reader<'_>) -> Result<Option<T>> {
    Ok(match r.read_i32()? {
        0 => None,
        _ => Some(T::read_from(r)?),
    })
}

pub fn write_typed<T: WriteParcelable>(p: &mut Parcel, value: Option<&T>) {
    match value {
        None => p.write_i32(0),
        Some(v) => {
            p.write_i32(1);
            v.write_to(p);
        }
    }
}

/// `createIntArray`.
pub fn read_int_array(r: &mut Reader<'_>) -> Result<Option<Vec<i32>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    (0..n).map(|_| r.read_i32()).collect::<Result<Vec<_>>>().map(Some)
}

pub fn write_int_array(p: &mut Parcel, value: Option<&[i32]>) {
    match value {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            v.iter().for_each(|x| p.write_i32(*x));
        }
    }
}

/// `createStringArray` / `createStringArrayList`.
pub fn read_string_list(r: &mut Reader<'_>) -> Result<Option<Vec<Option<String>>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    (0..n).map(|_| r.read_string16()).collect::<Result<Vec<_>>>().map(Some)
}

pub fn write_string_list(p: &mut Parcel, value: Option<&[Option<String>]>) {
    match value {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            v.iter().for_each(|s| p.write_string16(s.as_deref()));
        }
    }
}

"#;

/// The `TRANSACTION_*` codes and `DESCRIPTOR` of `descriptor`'s stub in
/// `jar` (a path in the image).
fn stub_codes(
    image: &Path,
    jar: &str,
    descriptor: &str,
) -> Result<(BTreeMap<String, u32>, String), String> {
    let path = image.join(jar);
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    dex_stub_codes(
        &aim_android_image::system_server::dex_files(&bytes)?,
        descriptor,
    )
    .map_err(|e| format!("{jar}: {e}"))
}

/// The `TRANSACTION_*` codes and `DESCRIPTOR` of `descriptor`'s stub in
/// `dexes`.
fn dex_stub_codes(
    dexes: &[&[u8]],
    descriptor: &str,
) -> Result<(BTreeMap<String, u32>, String), String> {
    let class = format!("L{};", descriptor.replace('.', "/"));
    let stub = format!("L{}$Stub;", descriptor.replace('.', "/"));
    let (mut codes, mut found_descriptor) = (BTreeMap::new(), None);
    for dex in dexes {
        let dex = Dex::parse(dex)?;
        for name in [&class, &stub] {
            let Some(def) = dex.class(name) else { continue };
            for (field, value) in dex.static_values(def)? {
                match (field.strip_prefix("TRANSACTION_"), value) {
                    (Some(method), Value::Int(code)) if name == &stub => {
                        codes.insert(method.to_string(), code as u32);
                    }
                    (None, Value::String(s)) if field == "DESCRIPTOR" => found_descriptor = Some(s),
                    _ => {}
                }
            }
        }
    }
    let found = found_descriptor.ok_or_else(|| format!("no stub of {descriptor}"))?;
    Ok((codes, found))
}

/// Our own interfaces (`OWN_INTERFACES`): descriptor, parsed AIDL, the
/// methods to marshal.
fn own_interfaces(lock: &Lock) -> Result<Vec<(Interface, Vec<String>, String)>, String> {
    let mut out = Vec::new();
    for entry in lock.array("OWN_INTERFACES") {
        let fields: Vec<&str> = entry.split('|').collect();
        let [descriptor, file, methods] = fields[..] else {
            return Err(format!("{LOCK}: bad OWN_INTERFACES entry `{entry}`"));
        };
        let path = aim_paths::root().join(file);
        let text = fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
        let iface = parse(&text).map_err(|e| format!("{file}: {e}"))?;
        if iface.descriptor() != descriptor {
            return Err(format!("{file}: declares {}", iface.descriptor()));
        }
        out.push((
            iface,
            methods.split(',').map(str::to_string).collect(),
            file.to_string(),
        ));
    }
    Ok(out)
}

/// Checks the Java stubs of our own interfaces in `dex` against the codes
/// generated for the Rust side.
pub fn check_own_stubs(dex: &Path) -> Result<(), String> {
    let lock = Lock::read(&aim_paths::root().join(LOCK))?;
    let bytes = fs::read(dex).map_err(|e| format!("{}: {e}", dex.display()))?;
    for (iface, _, file) in own_interfaces(&lock)? {
        let descriptor = iface.descriptor();
        let (stub, found) = dex_stub_codes(&[&bytes], &descriptor)?;
        let ours: BTreeMap<String, u32> = iface
            .methods
            .iter()
            .map(|m| (m.name.clone(), m.code))
            .collect();
        if found != descriptor || stub != ours {
            return Err(format!(
                "{file}: the Java stub's codes {stub:?} are not the generated {ours:?}"
            ));
        }
    }
    Ok(())
}

/// The module of a hand-written interface: its descriptor and the
/// transaction codes `names`, read from the constants of `descriptor`'s
/// class in `jar`.
fn constants_code(
    image: &Path,
    jar: &str,
    descriptor: &str,
    names: &str,
) -> Result<String, String> {
    let path = image.join(jar);
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let class = format!("L{};", descriptor.replace('.', "/"));
    let mut values = BTreeMap::new();
    for dex in aim_android_image::system_server::dex_files(&bytes)? {
        let dex = Dex::parse(dex)?;
        if let Some(def) = dex.class(&class) {
            values.extend(dex.static_values(def)?);
        }
    }
    match values.get("descriptor") {
        Some(Value::String(d)) if d == descriptor => {}
        _ => return Err(format!("{jar}: {descriptor} does not declare itself")),
    }
    let mut s = String::new();
    writeln!(s, "/// `{descriptor}` (constants of {jar})").unwrap();
    writeln!(s, "pub mod {} {{", module_name(descriptor)).unwrap();
    writeln!(s, "    pub const DESCRIPTOR: &str = \"{descriptor}\";").unwrap();
    for name in names.split(',') {
        let Some(Value::Int(code)) = values.get(name) else {
            return Err(format!("{jar}: {descriptor} has no int constant `{name}`"));
        };
        writeln!(s, "    pub const {name}: u32 = {};", *code as u32).unwrap();
    }
    writeln!(s, "}}\n").unwrap();
    Ok(s)
}

/// Fetches the pinned AIDL, generates the Rust and checks the codes
/// against the image.
pub fn run(log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&aim_paths::root().join(LOCK))?;
    let tag = lock.get("AOSP_TAG")?;
    let mut files = Vec::new();
    for entry in lock.array("SOURCE_FILES") {
        let source = Source::parse(entry, tag)?;
        files.push((
            source.file(log)?,
            format!("{}/{}", source.project, source.path),
        ));
    }
    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let mut code = PRELUDE.to_string();
    for entry in lock.array("INTERFACES") {
        let fields: Vec<&str> = entry.split('|').collect();
        let [descriptor, jar, methods] = fields[..] else {
            return Err(format!("{LOCK}: bad INTERFACES entry `{entry}`"));
        };
        let file = format!("/{}.aidl", descriptor.replace('.', "/"));
        let (path, origin) = files
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(&file))
            .ok_or_else(|| format!("{LOCK}: no SOURCE_FILES entry for {descriptor}"))?;
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let iface = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if iface.descriptor() != descriptor {
            return Err(format!(
                "{}: declares {}",
                path.display(),
                iface.descriptor()
            ));
        }
        let (stub, found) = stub_codes(&image, jar, descriptor)?;
        let ours: BTreeMap<String, u32> = iface
            .methods
            .iter()
            .map(|m| (m.name.clone(), m.code))
            .collect();
        if found != descriptor || stub != ours {
            return Err(format!(
                "{descriptor}: the pinned AIDL does not match the image's {jar} (AIDL {ours:?}, stub {stub:?})"
            ));
        }
        let selected: Vec<String> = methods.split(',').map(str::to_string).collect();
        code += &interface_code(&iface, &selected, origin)?;
    }
    for (iface, selected, file) in own_interfaces(&lock)? {
        code += &interface_code(&iface, &selected, &file)?;
    }
    for entry in lock.array("CONSTANTS") {
        let fields: Vec<&str> = entry.split('|').collect();
        let [descriptor, jar, names] = fields[..] else {
            return Err(format!("{LOCK}: bad CONSTANTS entry `{entry}`"));
        };
        code += &constants_code(&image, jar, descriptor, names)?;
    }
    let dir = out();
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::write(dir.join("lib.rs"), code).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
package android.content;

import android.content.ClipData;
// A comment (with parens)
/** {@hide} */
interface IClipboard {
    const int FLAG = 1 << 0;
    void setPrimaryClip(in ClipData clip, String callingPackage, int userId);
    @EnforcePermission(allOf={"A", "B"})
    @nullable ClipData getPrimaryClip(String pkg);
    oneway void ping(IOnChanged listener) = 7;
    int[] ids(boolean all);
    parcelable Nested { int x; }
}
"#;

    #[test]
    fn parses_methods_and_codes() {
        let iface = parse(SAMPLE).unwrap();
        assert_eq!(iface.descriptor(), "android.content.IClipboard");
        let codes: Vec<(&str, u32, bool)> = iface
            .methods
            .iter()
            .map(|m| (m.name.as_str(), m.code, m.oneway))
            .collect();
        assert_eq!(
            codes,
            [
                ("setPrimaryClip", 1, false),
                ("getPrimaryClip", 2, false),
                ("ping", 8, true),
                ("ids", 4, false)
            ]
        );
        assert_eq!(iface.methods[0].params[0].ty.name, "ClipData");
    }

    #[test]
    fn names() {
        assert_eq!(
            camel("checkGrantUriPermission_ignoreNonSystem"),
            "CheckGrantUriPermissionIgnoreNonSystem"
        );
        assert_eq!(snake("setPrimaryClip"), "set_primary_clip");
        assert_eq!(snake("getUIDState"), "get_uid_state");
        assert_eq!(snake("type"), "r#type");
        assert_eq!(camel("setPrimaryClip"), "SetPrimaryClip");
    }

    #[test]
    fn generates_typed_methods() {
        let iface = parse(SAMPLE).unwrap();
        let code = interface_code(&iface, &["*".into()], "test").unwrap();
        assert!(code.contains("pub const SET_PRIMARY_CLIP: u32 = 1;"));
        assert!(code.contains("pub struct SetPrimaryClip<ClipData> {"));
        assert!(code.contains("pub fn read_get_primary_clip_reply<ClipData: ReadParcelable>"));
        assert!(code.contains("impl<ClipData: WriteParcelable> SetPrimaryClip<ClipData> {"));
        assert!(!code.contains("fn read_ping_reply"));
        assert!(code.contains("pub fn write_ids_reply(p: &mut Parcel, result: &Option<Vec<i32>>)"));
    }
}
