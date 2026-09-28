//! What PackageManager keeps per user about each package
//! (`/data/system/users/0/package-restrictions.xml`): whether the package
//! is installed and enabled for the user, and the components enabled or
//! disabled at run time. Android writes it in its binary XML ("ABX",
//! `BinaryXmlSerializer`).

use std::collections::HashMap;
use std::path::Path;

/// `PackageManager.COMPONENT_ENABLED_STATE_*` that switch a package off.
const DISABLED_STATES: [i64; 3] = [2, 3, 4];

/// One package's state for the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Package {
    pub installed: bool,
    pub enabled: bool,
    pub enabled_components: Vec<String>,
    pub disabled_components: Vec<String>,
}

/// The packages' states, by package name.
pub type Restrictions = HashMap<String, Package>;

/// Read `path`; empty when it is missing or unreadable (every package
/// installed and enabled, as PackageManager assumes then).
pub fn read(path: &Path) -> Restrictions {
    std::fs::read(path)
        .ok()
        .and_then(|b| parse(&b))
        .unwrap_or_default()
}

#[derive(Debug, PartialEq)]
enum Attr {
    Str(String),
    Int(i64),
    Bool(bool),
    Other,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
    pool: Vec<String>,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_be_bytes([s[0], s[1]]))
    }

    fn utf(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn interned(&mut self) -> Option<String> {
        match self.u16()? {
            0xffff => {
                let s = self.utf()?;
                self.pool.push(s.clone());
                Some(s)
            }
            i => self.pool.get(i as usize).cloned(),
        }
    }

    /// A value of type `ty` (the token's high nibble).
    fn value(&mut self, ty: u8) -> Option<Attr> {
        Some(match ty {
            1 => Attr::Other,
            2 => Attr::Str(self.utf()?),
            3 => Attr::Str(self.interned()?),
            4 | 5 => {
                let n = self.u16()? as usize;
                self.take(n)?;
                Attr::Other
            }
            6 | 7 => {
                let s = self.take(4)?;
                Attr::Int(i32::from_be_bytes([s[0], s[1], s[2], s[3]]) as i64)
            }
            8 | 9 => {
                let s = self.take(8)?;
                Attr::Int(i64::from_be_bytes(s.try_into().ok()?))
            }
            10 => {
                self.take(4)?;
                Attr::Other
            }
            11 => {
                self.take(8)?;
                Attr::Other
            }
            12 => Attr::Bool(true),
            13 => Attr::Bool(false),
            _ => return None,
        })
    }
}

/// Parse an ABX document into package states.
fn parse(b: &[u8]) -> Option<Restrictions> {
    if b.get(..4)? != b"ABX\0" {
        return None;
    }
    let mut r = Reader {
        b,
        at: 4,
        pool: Vec::new(),
    };
    let mut out = Restrictions::new();
    // The open elements' names, and the package being read.
    let mut path: Vec<String> = Vec::new();
    let mut current: Option<(String, Package)> = None;
    while r.at < b.len() {
        let token = r.take(1)?[0];
        let (event, ty) = (token & 0x0f, token >> 4);
        match event {
            // START_TAG
            2 => path.push(r.interned()?),
            // END_TAG
            3 => {
                let name = r.interned()?;
                if name == "pkg"
                    && let Some((n, p)) = current.take()
                {
                    out.insert(n, p);
                }
                path.pop();
            }
            // ATTRIBUTE
            15 => {
                let name = r.interned()?;
                let value = r.value(ty)?;
                let tag = path.last().map(String::as_str).unwrap_or_default();
                match (tag, name.as_str(), value) {
                    ("pkg", "name", Attr::Str(n)) => {
                        current = Some((
                            n,
                            Package {
                                installed: true,
                                enabled: true,
                                ..Default::default()
                            },
                        ))
                    }
                    ("pkg", "inst", Attr::Bool(v)) => {
                        if let Some((_, p)) = &mut current {
                            p.installed = v;
                        }
                    }
                    ("pkg", "enabled", Attr::Int(v)) => {
                        if let Some((_, p)) = &mut current {
                            p.enabled = !DISABLED_STATES.contains(&v);
                        }
                    }
                    ("item", "name", Attr::Str(c)) => {
                        let list = path.get(path.len().wrapping_sub(2)).map(String::as_str);
                        if let Some((_, p)) = &mut current {
                            match list {
                                Some("enabled-components") => p.enabled_components.push(c),
                                Some("disabled-components") => p.disabled_components.push(c),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            // START_DOCUMENT, END_DOCUMENT
            0 | 1 => {}
            // Text and the like: a value of the token's type.
            _ => {
                r.value(ty)?;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ABX writing, as `BinaryXmlSerializer` does, for the test.
    #[derive(Default)]
    struct W {
        b: Vec<u8>,
        pool: Vec<String>,
    }

    impl W {
        fn interned(&mut self, s: &str) {
            match self.pool.iter().position(|p| p == s) {
                Some(i) => self.b.extend_from_slice(&(i as u16).to_be_bytes()),
                None => {
                    self.b.extend_from_slice(&0xffffu16.to_be_bytes());
                    self.b.extend_from_slice(&(s.len() as u16).to_be_bytes());
                    self.b.extend_from_slice(s.as_bytes());
                    self.pool.push(s.into());
                }
            }
        }
        fn start(&mut self, t: &str) {
            self.b.push(2 | 3 << 4);
            self.interned(t);
        }
        fn end(&mut self, t: &str) {
            self.b.push(3 | 3 << 4);
            self.interned(t);
        }
        fn attr_str(&mut self, n: &str, v: &str) {
            self.b.push(15 | 2 << 4);
            self.interned(n);
            self.b.extend_from_slice(&(v.len() as u16).to_be_bytes());
            self.b.extend_from_slice(v.as_bytes());
        }
        fn attr_int(&mut self, n: &str, v: i32) {
            self.b.push(15 | 6 << 4);
            self.interned(n);
            self.b.extend_from_slice(&v.to_be_bytes());
        }
        fn attr_bool(&mut self, n: &str, v: bool) {
            self.b.push(15 | if v { 12 } else { 13 } << 4);
            self.interned(n);
        }
    }

    #[test]
    fn packages_and_components() {
        let mut w = W::default();
        w.b.extend_from_slice(b"ABX\0");
        w.b.push(1 << 4); // START_DOCUMENT
        w.start("package-restrictions");
        w.start("pkg");
        w.attr_str("name", "org.gone");
        w.attr_bool("inst", false);
        w.end("pkg");
        w.start("pkg");
        w.attr_str("name", "org.app");
        w.attr_int("enabled", 1);
        w.start("disabled-components");
        w.start("item");
        w.attr_str("name", "org.app.Launcher");
        w.end("item");
        w.end("disabled-components");
        w.end("pkg");
        w.start("pkg");
        w.attr_str("name", "org.off");
        w.attr_int("enabled", 3);
        w.end("pkg");
        w.end("package-restrictions");
        w.b.push(1 | 1 << 4);
        let r = parse(&w.b).unwrap();
        assert!(!r["org.gone"].installed);
        assert_eq!(r["org.app"].disabled_components, ["org.app.Launcher"]);
        assert!(r["org.app"].enabled && r["org.app"].installed);
        assert!(!r["org.off"].enabled);
        assert_eq!(parse(b"<?xml"), None);
    }
}
