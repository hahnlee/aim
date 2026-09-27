//! `darwin-artctl native-coverage`: every native method of the pinned jars
//! (the darwin-art-native-inventory list) with its state in this process.
//!
//! - `registered`: its JNI entry point is bound (RegisterNatives, or an
//!   earlier by-name lookup).
//! - `by-name`: unbound, but an exported `Java_...` symbol would bind it on
//!   first call.
//! - `missing`: neither; a call throws UnsatisfiedLinkError.
//! - `unloaded`: its class is not visible to the system server class loader
//!   (a standalone system server jar) or failed to load.
//!
//! Classes are loaded without initialization; ART's binding record comes from
//! compat/art/native_registration_audit.cc.

use crate::jni_env::{Env, native};
use darwin_art_native_inventory::{ANDROID16_NATIVES, InventoryEntry, inventory};
use jni_sys::{JNIEnv, jclass, jobject, jstring, jvalue};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{CString, c_void};
use std::fmt::Write;
use std::os::raw::c_char;

unsafe extern "C" {
    fn darwin_art_class_native_registrations(
        env: *mut JNIEnv,
        class: jclass,
        out: *mut c_char,
        capacity: usize,
    ) -> usize;
}

/// JNI's name mangling (JNI specification, "Resolving Native Method Names").
fn mangle(text: &str) -> String {
    let mut out = String::new();
    for unit in text.encode_utf16() {
        match unit {
            0x2f => out.push('_'),
            0x5f => out.push_str("_1"),
            0x3b => out.push_str("_2"),
            0x5b => out.push_str("_3"),
            unit if (unit as u8).is_ascii_alphanumeric() && unit < 0x80 => {
                out.push(unit as u8 as char)
            }
            unit => {
                let _ = write!(out, "_0{unit:04x}");
            }
        }
    }
    out
}

/// The short and long JNI symbol names of a native method.
fn jni_symbols(class: &str, name: &str, signature: &str) -> [String; 2] {
    let internal = class.trim_start_matches('L').trim_end_matches(';');
    let short = format!("Java_{}_{}", mangle(internal), mangle(name));
    let parameters = signature
        .strip_prefix('(')
        .and_then(|rest| rest.split(')').next())
        .unwrap_or("");
    let long = format!("{short}__{}", mangle(parameters));
    [short, long]
}

fn exported(symbol: &str) -> bool {
    let Ok(symbol) = CString::new(symbol) else {
        return false;
    };
    !unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) }.is_null()
}

/// ART's binding state of each native method a class declares.
fn bindings(env: Env, class: jclass) -> HashMap<(String, String), bool> {
    let env_pointer = env.raw();
    let length = unsafe {
        darwin_art_class_native_registrations(env_pointer, class, std::ptr::null_mut(), 0)
    };
    let mut buffer = vec![0u8; length + 1];
    unsafe {
        darwin_art_class_native_registrations(
            env_pointer,
            class,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    buffer.truncate(length);
    String::from_utf8_lossy(&buffer)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_owned();
            let signature = fields.next()?.to_owned();
            Some(((name, signature), fields.next()? == "R"))
        })
        .collect()
}

/// `Class.forName(name, false, loader)`, or null (exception cleared).
fn load(env: Env, loader: jobject, class: &str) -> jclass {
    let dotted = class
        .trim_start_matches('L')
        .trim_end_matches(';')
        .replace('/', ".");
    let class_class = env.find_class(c"java/lang/Class");
    let for_name = env.static_method_id(
        class_class,
        c"forName",
        c"(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;",
    );
    let name = env.new_string_utf(&dotted);
    let loaded = env.call_static_object(
        class_class,
        for_name,
        &[jvalue { l: name }, jvalue { z: 0 }, jvalue { l: loader }],
    );
    env.delete_local_ref(name);
    env.delete_local_ref(class_class);
    if env.exception_check() {
        env.exception_clear();
        return std::ptr::null_mut();
    }
    loaded
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    Registered,
    ByName,
    Missing,
    Unloaded,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            State::Registered => "registered",
            State::ByName => "by-name",
            State::Missing => "missing",
            State::Unloaded => "unloaded",
        }
    }
}

fn state(entry: &InventoryEntry<'_>, bound: Option<bool>) -> State {
    match bound {
        None => State::Unloaded,
        Some(true) => State::Registered,
        Some(false) => {
            if jni_symbols(entry.class, entry.name, entry.signature)
                .iter()
                .any(|symbol| exported(symbol))
            {
                State::ByName
            } else {
                State::Missing
            }
        }
    }
}

/// The per-jar summary followed by one `STATE JAR CLASS METHOD SIGNATURE`
/// line per native.
fn report(env: Env, loader: jobject) -> String {
    let mut classes: BTreeMap<&str, Vec<InventoryEntry<'_>>> = BTreeMap::new();
    for entry in inventory(ANDROID16_NATIVES) {
        classes.entry(entry.class).or_default().push(entry);
    }
    let mut rows = Vec::new();
    for (class, entries) in classes {
        let loaded = load(env, loader, class);
        let bound = if loaded.is_null() {
            HashMap::new()
        } else {
            let bound = bindings(env, loaded);
            env.delete_local_ref(loaded);
            bound
        };
        for entry in entries {
            let key = (entry.name.to_owned(), entry.signature.to_owned());
            let binding = if loaded.is_null() {
                None
            } else {
                // A method missing from the loaded class reads as unbound.
                Some(bound.get(&key).copied().unwrap_or(false))
            };
            rows.push((state(&entry, binding), entry));
        }
    }
    let mut summary: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    for (state, entry) in &rows {
        summary.entry(entry.jar).or_default()[*state as usize] += 1;
    }
    let mut out = String::from("# JAR\tregistered\tby-name\tmissing\tunloaded\n");
    let mut totals = [0usize; 4];
    for (jar, counts) in &summary {
        let _ = writeln!(
            out,
            "{jar}\t{}\t{}\t{}\t{}",
            counts[0], counts[1], counts[2], counts[3]
        );
        for (total, count) in totals.iter_mut().zip(counts) {
            *total += count;
        }
    }
    let _ = writeln!(
        out,
        "TOTAL\t{}\t{}\t{}\t{}\n# STATE\tJAR\tCLASS\tMETHOD\tSIGNATURE",
        totals[0], totals[1], totals[2], totals[3]
    );
    for (state, entry) in &rows {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            state.label(),
            entry.jar,
            entry.class,
            entry.name,
            entry.signature
        );
    }
    out
}

unsafe extern "system" fn native_report(
    env: *mut JNIEnv,
    _class: jclass,
    loader: jobject,
) -> jstring {
    let env = unsafe { Env::new(env) };
    let text = report(env, loader);
    env.new_string_utf(&text)
}

/// Register `NativeCoverage.nativeReport(ClassLoader)`.
///
/// # Safety
/// `env` must be the calling thread's JNIEnv and `class` the
/// dev.darwinart.runtime.system.NativeCoverage class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_register_native_coverage(
    env: *mut JNIEnv,
    class: jclass,
) -> bool {
    let env = unsafe { Env::new(env) };
    env.register_natives(
        class,
        &[native(
            c"nativeReport",
            c"(Ljava/lang/ClassLoader;)Ljava/lang/String;",
            native_report as *mut c_void,
        )],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangles_jni_names() {
        assert_eq!(
            jni_symbols("Landroid/os/Debug;", "getMemInfo", "([J)V"),
            [
                "Java_android_os_Debug_getMemInfo".to_owned(),
                "Java_android_os_Debug_getMemInfo___3J".to_owned()
            ]
        );
        assert_eq!(
            jni_symbols(
                "Landroid/net/LocalSocketImpl;",
                "read_native",
                "(Ljava/io/FileDescriptor;)I"
            )[1],
            "Java_android_net_LocalSocketImpl_read_1native__Ljava_io_FileDescriptor_2"
        );
        assert_eq!(mangle("a$b"), "a_00024b");
    }
}
