//! The classpath fragments `derive_classpath` merges into
//! `BOOTCLASSPATH` and `SYSTEMSERVERCLASSPATH`
//! (`/system/etc/classpaths/*.pb`, then `/apex/*/etc/classpaths/*.pb`
//! sorted by path; `packages/modules/common/proto/classpaths.proto`):
//! `ExportedClasspathsJars { repeated Jar jars = 1; }`,
//! `Jar { string path = 1; Classpath classpath = 2; ... }`.

use std::path::Path;

/// `Classpath.BOOTCLASSPATH`.
pub const BOOTCLASSPATH: u64 = 1;
/// `Classpath.SYSTEMSERVERCLASSPATH`.
pub const SYSTEMSERVERCLASSPATH: u64 = 2;

#[derive(Debug, PartialEq, Eq)]
pub struct Jar {
    pub path: String,
    pub classpath: u64,
}

fn varint(data: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*at).ok_or("classpath: truncated varint")?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("classpath: overlong varint".into())
}

/// Calls `field(number, varint, bytes)` for each field of a message.
fn fields(
    data: &[u8],
    mut field: impl FnMut(u64, u64, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut at = 0;
    while at < data.len() {
        let key = varint(data, &mut at)?;
        match key & 7 {
            0 => field(key >> 3, varint(data, &mut at)?, &[])?,
            2 => {
                let len = varint(data, &mut at)? as usize;
                let bytes = data.get(at..at + len).ok_or("classpath: truncated field")?;
                at += len;
                field(key >> 3, 0, bytes)?;
            }
            wire => return Err(format!("classpath: unexpected wire type {wire}")),
        }
    }
    Ok(())
}

/// The jars of one fragment.
pub fn parse(data: &[u8]) -> Result<Vec<Jar>, String> {
    let mut jars = Vec::new();
    fields(data, |number, _, bytes| {
        if number == 1 {
            let mut jar = Jar {
                path: String::new(),
                classpath: 0,
            };
            fields(bytes, |number, value, bytes| {
                match number {
                    1 => jar.path = String::from_utf8_lossy(bytes).into_owned(),
                    2 => jar.classpath = value,
                    _ => {}
                }
                Ok(())
            })?;
            jars.push(jar);
        }
        Ok(())
    })?;
    Ok(jars)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// `jar` as one more `jars` entry: appended to a fragment, it is its last
/// jar (repeated fields concatenate).
pub fn encode(jar: &Jar) -> Vec<u8> {
    let mut inner = vec![0x0a];
    put_varint(&mut inner, jar.path.len() as u64);
    inner.extend_from_slice(jar.path.as_bytes());
    inner.push(0x10);
    put_varint(&mut inner, jar.classpath);
    let mut out = vec![0x0a];
    put_varint(&mut out, inner.len() as u64);
    out.extend(inner);
    out
}

/// The guest paths of the jars on `classpath` in the image at `root`, in
/// `derive_classpath`'s order.
pub fn jars(root: &Path, name: &str, classpath: u64) -> Result<Vec<String>, String> {
    let mut fragments = vec![root.join("system/etc/classpaths").join(name)];
    let mut apexes: Vec<_> = std::fs::read_dir(root.join("apex"))
        .map_err(|e| format!("{}: {e}", root.join("apex").display()))?
        .filter_map(|e| e.ok().map(|e| e.path().join("etc/classpaths").join(name)))
        .filter(|p| p.is_file())
        .collect();
    apexes.sort();
    fragments.extend(apexes);
    let mut out = Vec::new();
    for fragment in fragments {
        let data = std::fs::read(&fragment).map_err(|e| format!("{}: {e}", fragment.display()))?;
        out.extend(
            parse(&data)?
                .into_iter()
                .filter(|j| j.classpath == classpath)
                .map(|j| j.path),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let first = Jar {
            path: "/system/framework/services.jar".into(),
            classpath: SYSTEMSERVERCLASSPATH,
        };
        let second = Jar {
            path: "/system/framework/ours.jar".into(),
            classpath: SYSTEMSERVERCLASSPATH,
        };
        let mut data = encode(&first);
        data.extend(encode(&second));
        assert_eq!(parse(&data).unwrap(), [first, second]);
        assert!(parse(&[0x0a, 0x05, 0x0a]).is_err());
    }
}
