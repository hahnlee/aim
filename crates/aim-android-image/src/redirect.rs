//! The redirect edit of ADR 0013's SystemServer exception: named call sites
//! of the original `services.jar` call a static method of the device's own
//! `aim-services.jar` instead (#702 D1, #668).
//!
//! A redirect names the methods of a class that make the call (`caller`,
//! every method of that name), the method they call (`call`, as the dex
//! names it: the class of the reference and the method's name) and the
//! static method that takes its place (`to`). Each call becomes an
//! `invoke-static` of `to` with the same registers: `to` takes the
//! receiver of an instance call first, then the call's arguments, and
//! returns what the call returns, so the target's signature is the call's
//! with the receiver's class prepended. The dex gets the target's method id
//! ([`reindex::add_methods`]); nothing else of it changes.
//!
//! Checked symbolically, as the `nop` edit is: a redirect whose caller
//! makes the call another number of times than it says, a call that names
//! two overloads, a constructor or `super` call, or a target that is not a
//! public static method of a public class of `aim-services.jar` with that
//! signature fails the build.
//!
//! The list is `image/system-server-redirects`: one `caller call to calls
//! reason` line per redirect.
//!
//! [`reindex::add_methods`]: crate::reindex::add_methods

use crate::dex::{self, Dex};
use crate::reindex::{self, MethodRef};
use crate::system_server::stored_dex_entries;

const ACC_PUBLIC: u32 = 0x1;
const ACC_STATIC: u32 = 0x8;
const INVOKE_STATIC: u16 = 0x71;
const INVOKE_STATIC_RANGE: u16 = 0x77;

/// A line of `image/system-server-redirects`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// The calling class and method name (`a.b.Foo.start`).
    pub caller: String,
    /// The called method as the dex refers to it (`a.b.Bar.run`).
    pub call: String,
    /// The static method of aim-services.jar that takes its place.
    pub to: String,
    /// How many calls the caller's methods of that name make.
    pub calls: usize,
    pub reason: String,
}

pub fn parse(text: &str) -> Result<Vec<Redirect>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let bad = || format!("line {}: expected `caller call to calls reason`", i + 1);
        let [caller, call, to, calls, reason @ ..] = &fields[..] else {
            return Err(bad());
        };
        let calls: usize = calls.parse().map_err(|_| bad())?;
        if calls == 0 || reason.is_empty() || [caller, call, to].iter().any(|m| !m.contains('.')) {
            return Err(bad());
        }
        out.push(Redirect {
            caller: caller.to_string(),
            call: call.to_string(),
            to: to.to_string(),
            calls,
            reason: reason.join(" "),
        });
    }
    Ok(out)
}

/// `a.b.Foo.name` as a class descriptor and a name.
fn member(name: &str) -> (String, &str) {
    let (class, method) = name.rsplit_once('.').unwrap();
    (format!("L{};", class.replace('.', "/")), method)
}

/// A call to redirect: the byte offset of its instruction and the method
/// it calls.
struct Site {
    at: usize,
    unit: u16,
    method: MethodRef,
}

/// The calls of `r.call` in `r.caller`'s methods.
fn sites(dex: &Dex, data: &[u8], r: &Redirect) -> Result<Vec<Site>, String> {
    let (class, method) = member(&r.caller);
    let (call_class, call_name) = member(&r.call);
    let def = dex
        .class(&class)
        .ok_or_else(|| format!("no class {class}"))?;
    let mut out = Vec::new();
    for code in dex.methods_named(def, method)? {
        let insns = dex::units(data, &code)?;
        let mut pc = 0;
        while pc < insns.len() {
            let unit = insns[pc];
            if matches!(unit & 0xff, 0x6e..=0x72 | 0x74..=0x78) {
                let called = dex.method(u32::from(insns[pc + 1]))?;
                if called.0 == call_class && called.1 == call_name {
                    out.push(Site {
                        at: code.insns_off + 2 * pc,
                        unit,
                        method: called,
                    });
                }
            }
            pc += dex::instruction_units(&insns, pc)?;
        }
    }
    Ok(out)
}

/// The target of the calls `sites` make: `to` with the call's signature,
/// the receiver first for an instance call.
fn target(r: &Redirect, sites: &[Site]) -> Result<MethodRef, String> {
    let what = format!("{} in {}", r.call, r.caller);
    if sites.len() != r.calls {
        return Err(format!("{what}: {} calls, not {}", sites.len(), r.calls));
    }
    let first = &sites[0];
    if sites.iter().any(|s| s.method != first.method) {
        return Err(format!("{what}: calls more than one method of that name"));
    }
    let op = first.unit & 0xff;
    if sites.iter().any(|s| s.unit & 0xff != op) {
        return Err(format!("{what}: calls of more than one kind"));
    }
    let (class, name, sig) = &first.method;
    if name == "<init>" || matches!(op, 0x6f | 0x75) {
        return Err(format!("{what}: a constructor or super call"));
    }
    let sig = if matches!(op, 0x71 | 0x77) {
        sig.clone()
    } else {
        format!("({class}{}", &sig[1..])
    };
    let (to_class, to_name) = member(&r.to);
    Ok((to_class, to_name.to_string(), sig))
}

/// Whether `targets` (the dex files of aim-services.jar) have `m` as a
/// public static method of a public class.
fn check_target(targets: &[&[u8]], m: &MethodRef) -> Result<(), String> {
    let (class, name, sig) = m;
    for data in targets {
        let dex = Dex::parse(data)?;
        let Some(def) = dex.class(class) else {
            continue;
        };
        if def.access & ACC_PUBLIC == 0 {
            return Err(format!("{class} is not public"));
        }
        for (index, access) in dex.direct_methods(def)? {
            if &dex.method(index)? == m {
                if access & (ACC_PUBLIC | ACC_STATIC) != ACC_PUBLIC | ACC_STATIC {
                    return Err(format!("{class}.{name}{sig} is not public static"));
                }
                return Ok(());
            }
        }
        return Err(format!("{class} has no static {name}{sig}"));
    }
    Err(format!("no class {class} for {name}{sig}"))
}

/// `jar` (services.jar) with each of `redirects` made, its targets checked
/// against `targets` (the dex files of aim-services.jar).
pub fn redirect_jar(
    jar: &[u8],
    redirects: &[Redirect],
    targets: &[&[u8]],
) -> Result<Vec<u8>, String> {
    let entries = stored_dex_entries(jar)?;
    let mut replaced = Vec::new();
    let mut left: Vec<&Redirect> = redirects.iter().collect();
    for entry in &entries {
        let data = &jar[entry.data..entry.data + entry.size];
        let dex = Dex::parse(data)?;
        let (here, rest): (Vec<&Redirect>, Vec<&Redirect>) = left
            .into_iter()
            .partition(|r| dex.class(&member(&r.caller).0).is_some());
        left = rest;
        if here.is_empty() {
            continue;
        }
        let mut methods = Vec::new();
        for r in &here {
            let to = target(r, &sites(&dex, data, r)?)?;
            check_target(targets, &to)?;
            methods.push(to);
        }
        let (mut out, indices) = reindex::add_methods(data, &methods)?;
        let edits: Vec<Vec<Site>> = {
            let dex = Dex::parse(&out)?;
            here.iter()
                .map(|r| sites(&dex, &out, r))
                .collect::<Result<_, _>>()?
        };
        for (sites, index) in edits.into_iter().zip(indices) {
            for site in sites {
                let op = if site.unit & 0xff >= 0x74 {
                    INVOKE_STATIC_RANGE
                } else {
                    INVOKE_STATIC
                };
                let unit = site.unit & 0xff00 | op;
                out[site.at..site.at + 2].copy_from_slice(&unit.to_le_bytes());
                out[site.at + 2..site.at + 4].copy_from_slice(&(index as u16).to_le_bytes());
            }
        }
        dex::fix_checksums(&mut out);
        replaced.push((entry.local, out));
    }
    if let Some(r) = left.first() {
        return Err(format!("{}: no such class in the jar", r.caller));
    }
    replace_entries(jar, &replaced)
}

/// `jar` with the data of the stored entries whose local headers are at
/// the given offsets replaced. Every entry after one moves; dex files are
/// a multiple of 4 bytes long, so stored entries stay aligned.
fn replace_entries(jar: &[u8], replaced: &[(usize, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let u16_at = |at: usize| u16::from_le_bytes([jar[at], jar[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(jar[at..at + 4].try_into().unwrap()) as usize;
    let eocd = (0..jar.len().saturating_sub(21))
        .rev()
        .find(|&at| jar[at..at + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or("jar: no end of central directory")?;
    let (count, central) = (u16_at(eocd + 10), u32_at(eocd + 16));
    if count == 0xffff || central == 0xffff_ffff {
        return Err("jar: zip64".into());
    }
    let mut headers = Vec::new();
    let mut at = central;
    for _ in 0..count {
        headers.push(at);
        at += 46 + u16_at(at + 28) + u16_at(at + 30) + u16_at(at + 32);
    }
    let mut locals: Vec<usize> = headers.iter().map(|&h| u32_at(h + 42)).collect();
    locals.sort();
    locals.push(central);

    let mut out = jar[..locals[0]].to_vec();
    let mut moved = std::collections::HashMap::new();
    for pair in locals.windows(2) {
        let (local, end) = (pair[0], pair[1]);
        moved.insert(local, out.len());
        let Some((_, data)) = replaced.iter().find(|(l, _)| *l == local) else {
            out.extend_from_slice(&jar[local..end]);
            continue;
        };
        let start = local + 30 + u16_at(local + 26) + u16_at(local + 28);
        let header = out.len();
        out.extend_from_slice(&jar[local..start]);
        let crc = crc32fast::hash(data).to_le_bytes();
        let size = (data.len() as u32).to_le_bytes();
        out[header + 14..header + 18].copy_from_slice(&crc);
        out[header + 18..header + 22].copy_from_slice(&size);
        out[header + 22..header + 26].copy_from_slice(&size);
        out.extend_from_slice(data);
        out.extend_from_slice(&jar[start + u32_at(local + 18)..end]);
    }
    let new_central = out.len();
    for &h in &headers {
        let len = 46 + u16_at(h + 28) + u16_at(h + 30) + u16_at(h + 32);
        let at = out.len();
        out.extend_from_slice(&jar[h..h + len]);
        let local = u32_at(h + 42);
        if let Some((_, data)) = replaced.iter().find(|(l, _)| *l == local) {
            let size = (data.len() as u32).to_le_bytes();
            out[at + 16..at + 20].copy_from_slice(&crc32fast::hash(data).to_le_bytes());
            out[at + 20..at + 24].copy_from_slice(&size);
            out[at + 24..at + 28].copy_from_slice(&size);
        }
        out[at + 42..at + 46].copy_from_slice(&(moved[&local] as u32).to_le_bytes());
    }
    let tail = out.len();
    out.extend_from_slice(&jar[at..]);
    let eocd = tail + (eocd - at);
    out[eocd + 16..eocd + 20].copy_from_slice(&(new_central as u32).to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_list() {
        let list = parse("# comment\n\na.B.start c.D.run dev.aim.server.E.run 2 the reason (#1)\n")
            .unwrap();
        assert_eq!(
            list,
            [Redirect {
                caller: "a.B.start".into(),
                call: "c.D.run".into(),
                to: "dev.aim.server.E.run".into(),
                calls: 2,
                reason: "the reason (#1)".into(),
            }]
        );
        assert!(parse("a.B.start c.D.run dev.aim.server.E.run 2\n").is_err());
        assert!(parse("a.B.start c.D.run dev.aim.server.E.run two why\n").is_err());
    }
}
