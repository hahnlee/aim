//! The SystemServer exception of ADR 0013: a system service that a native
//! implementation replaces is not started by SystemServer.
//!
//! SystemServer starts each service with one call,
//! `mSystemServiceManager.startService(Foo.class)`, and has no switch to
//! leave one out. The derived image's `services.jar` is the original's
//! with that call (its `const-class` and `invoke-virtual`) turned into
//! `nop`s, in place: no other byte of the dex moves, the dex checksums and
//! the jar entry's CRC are recomputed. A call whose result SystemServer
//! uses, or a class started anywhere other than exactly once, is refused.
//!
//! The list is `image/native-services`: one `name class` line per service,
//! the binder name the native implementation registers and the
//! SystemServer class it replaces.

use std::path::Path;

use crate::dex::{self, Dex};

const SYSTEM_SERVER: &str = "Lcom/android/server/SystemServer;";
const START_SERVICE: (&str, &str, &str) = (
    "Lcom/android/server/SystemServiceManager;",
    "startService",
    "(Ljava/lang/Class;)Lcom/android/server/SystemService;",
);
const SYSTEM_SERVER_METHODS: [&str; 3] = [
    "startBootstrapServices",
    "startCoreServices",
    "startOtherServices",
];

/// Where the derived image has `image/native-services`, for guest-init.
pub const NATIVE_SERVICES: &str = "/system/etc/aim/native-services";

/// A service of `image/native-services`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeService {
    /// The binder name, under which the native implementation registers.
    pub name: String,
    /// The SystemServer class that is not started (`a.b.Foo`).
    pub class: String,
}

pub fn parse_native_services(text: &str) -> Result<Vec<NativeService>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [name, class] = fields[..] else {
            return Err(format!("line {}: expected `name class`", i + 1));
        };
        out.push(NativeService {
            name: name.into(),
            class: class.into(),
        });
    }
    Ok(out)
}

/// Writes `original` (services.jar) to `out` with SystemServer's start of
/// each of `classes` removed.
pub fn patch_services_jar(original: &Path, out: &Path, classes: &[String]) -> Result<(), String> {
    let mut jar = std::fs::read(original).map_err(|e| format!("{}: {e}", original.display()))?;
    let mut left: Vec<&String> = classes.iter().collect();
    for entry in stored_dex_entries(&jar)? {
        let data = &mut jar[entry.data..entry.data + entry.size];
        let patched = {
            let dex = Dex::parse(data)?;
            let Some(class) = dex.class(SYSTEM_SERVER) else {
                continue;
            };
            let mut edits = Vec::new();
            for name in &left {
                let descriptor = format!("L{};", name.replace('.', "/"));
                let site = start_site(&dex, data, class, &descriptor)?
                    .ok_or_else(|| format!("SystemServer does not start {name} as expected"))?;
                edits.push(site);
            }
            left.clear();
            edits
        };
        for (off, units) in patched {
            data[off..off + 2 * units].fill(0);
        }
        if left.is_empty() {
            dex::fix_checksums(data);
            let crc = crc32fast::hash(data);
            jar[entry.local + 14..entry.local + 18].copy_from_slice(&crc.to_le_bytes());
            jar[entry.central + 16..entry.central + 20].copy_from_slice(&crc.to_le_bytes());
            break;
        }
    }
    if !left.is_empty() {
        return Err(format!("{}: no SystemServer class", original.display()));
    }
    std::fs::write(out, jar).map_err(|e| format!("{}: {e}", out.display()))
}

/// The start of `descriptor` in SystemServer: the byte offset and length
/// in units of its `const-class` + `invoke-virtual startService`, whose
/// result must be unused.
fn start_site(
    dex: &Dex,
    data: &[u8],
    class: &dex::ClassDef,
    descriptor: &str,
) -> Result<Option<(usize, usize)>, String> {
    let Some(type_index) = dex.type_index(descriptor)? else {
        return Ok(None);
    };
    let mut found = Vec::new();
    for method in SYSTEM_SERVER_METHODS {
        for code in dex.methods_named(class, method)? {
            let insns = dex::units(data, &code)?;
            let mut at = 0;
            while at < insns.len() {
                let next = at + dex::instruction_units(&insns, at)?;
                let unit = insns[at];
                if unit & 0xff == 0x1c && u32::from(insns[at + 1]) == type_index {
                    let register = unit >> 8;
                    let invoke = insns.get(next..next + 3).unwrap_or_default();
                    let calls = invoke.len() == 3
                        && invoke[0] & 0xff == 0x6e
                        && invoke[0] >> 12 == 2
                        && (invoke[2] >> 4) & 0xf == register
                        && dex.method(u32::from(invoke[1]))?
                            == (
                                START_SERVICE.0.into(),
                                START_SERVICE.1.into(),
                                START_SERVICE.2.into(),
                            );
                    let result_used = insns
                        .get(next + 3)
                        .is_some_and(|u| (0x0a..=0x0c).contains(&(u & 0xff)));
                    if !calls || result_used {
                        return Err(format!(
                            "SystemServer.{method} loads {descriptor} for something other than an unused startService"
                        ));
                    }
                    found.push((code.insns_off + 2 * at, 5));
                }
                at = next;
            }
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        n => Err(format!("SystemServer starts {descriptor} {n} times")),
    }
}

/// The dex files of a jar, whose `classes*.dex` entries are stored.
pub fn dex_files(jar: &[u8]) -> Result<Vec<&[u8]>, String> {
    Ok(stored_dex_entries(jar)?
        .into_iter()
        .map(|e| &jar[e.data..e.data + e.size])
        .collect())
}

struct Entry {
    /// The local file header.
    local: usize,
    /// The central directory header.
    central: usize,
    data: usize,
    size: usize,
}

/// The jar's `classes*.dex` entries, which must be stored (as the
/// platform's are, so ART can map them).
fn stored_dex_entries(jar: &[u8]) -> Result<Vec<Entry>, String> {
    let u16_at = |at: usize| u16::from_le_bytes([jar[at], jar[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(jar[at..at + 4].try_into().unwrap()) as usize;
    let eocd = (0..jar.len().saturating_sub(21))
        .rev()
        .find(|&at| jar[at..at + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or("jar: no end of central directory")?;
    let (count, mut at) = (u16_at(eocd + 10), u32_at(eocd + 16));
    let mut out = Vec::new();
    for _ in 0..count {
        if jar.get(at..at + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err("jar: bad central directory".into());
        }
        let name_len = u16_at(at + 28);
        let name = String::from_utf8_lossy(&jar[at + 46..at + 46 + name_len]);
        let is_dex = name.starts_with("classes") && name.ends_with(".dex") && !name.contains('/');
        if is_dex {
            let (flags, method) = (u16_at(at + 8), u16_at(at + 10));
            if method != 0 || flags & 8 != 0 {
                return Err(format!("jar: {name} is not a plain stored entry"));
            }
            let local = u32_at(at + 42);
            let data = local + 30 + u16_at(local + 26) + u16_at(local + 28);
            out.push(Entry {
                local,
                central: at,
                data,
                size: u32_at(at + 20),
            });
        }
        at += 46 + name_len + u16_at(at + 30) + u16_at(at + 32);
    }
    out.sort_by_key(|e| e.local);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_list() {
        let list = parse_native_services(
            "# comment\n\nclipboard com.android.server.clipboard.ClipboardService # why\n",
        )
        .unwrap();
        assert_eq!(
            list,
            [NativeService {
                name: "clipboard".into(),
                class: "com.android.server.clipboard.ClipboardService".into()
            }]
        );
        assert!(parse_native_services("clipboard\n").is_err());
    }

    #[test]
    fn instruction_lengths() {
        // const-class, invoke-virtual, return-void, a packed-switch payload
        let insns = [
            0x041c, 0x1c39, 0x206e, 0x4c96, 0x0040, 0x000e, 0x0100, 1, 0, 0, 0, 0,
        ];
        assert_eq!(dex::instruction_units(&insns, 0).unwrap(), 2);
        assert_eq!(dex::instruction_units(&insns, 2).unwrap(), 3);
        assert_eq!(dex::instruction_units(&insns, 5).unwrap(), 1);
        assert_eq!(dex::instruction_units(&insns, 6).unwrap(), 6);
    }
}
