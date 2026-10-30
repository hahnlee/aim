//! An APK: its files, manifest and resources, and resolving a resource
//! against the app's table, the framework's (`framework-res.apk`) and the
//! app's theme.

use std::path::Path;

use android_image_extract::source::{FileSource, ReadAt};
use android_image_extract::zip::Archive;

use crate::res::{self, Element, Result, Value, bad};
use crate::table::{Config, Entry, Table};

/// The most an entry read into memory may hold (a large app's resource
/// table is tens of megabytes).
const MAX_ENTRY: u64 = 512 << 20;

/// APK file access without decoding resources for a particular consumer.
pub struct Files {
    source: Box<dyn ReadAt + Send + Sync>,
}

impl Files {
    pub fn open(path: &Path) -> Result<Self> {
        let source =
            Box::new(FileSource::open(path).map_err(|e| bad(format!("{}: {e}", path.display())))?);
        Ok(Self { source })
    }

    /// The file `name` of the archive.
    pub fn file(&self, name: &str) -> Result<Vec<u8>> {
        self.file_if_present(name)?
            .ok_or_else(|| bad(format!("no {name}")))
    }

    /// Absence is distinct from decompression, size or CRC failure.
    pub fn file_if_present(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let archive = Archive::open(self.source.as_ref()).map_err(|e| bad(e.to_string()))?;
        let Some(entry) = archive.find(name.as_bytes()) else {
            return Ok(None);
        };
        archive
            .read(entry, MAX_ENTRY)
            .map(Some)
            .map_err(|e| bad(e.to_string()))
    }
}

pub struct Apk {
    files: Files,
    table: Option<Table>,
}

impl Apk {
    pub fn open(path: &Path) -> Result<Apk> {
        let source =
            Box::new(FileSource::open(path).map_err(|e| bad(format!("{}: {e}", path.display())))?);
        Self::from_source(source)
    }

    /// An already-open APK, including a descriptor transferred over Binder.
    pub fn from_source(source: Box<dyn ReadAt + Send + Sync>) -> Result<Apk> {
        let files = Files { source };
        let table = files
            .file_if_present("resources.arsc")?
            .map(|bytes| Table::parse(&bytes))
            .transpose()?;
        Ok(Apk { files, table })
    }

    pub fn file(&self, name: &str) -> Result<Vec<u8>> {
        self.files.file(name)
    }

    /// Code-only APKs may omit resources.arsc; invalid entries fail.
    pub fn file_if_present(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.files.file_if_present(name)
    }

    pub fn manifest(&self) -> Result<Element> {
        res::xml(&self.file("AndroidManifest.xml")?)
    }

    pub fn table(&self) -> Option<&Table> {
        self.table.as_ref()
    }
}

/// An app's resources: its own table, the framework's, a theme, and the
/// night mode their configurations are chosen for.
pub struct Resources<'a> {
    pub app: &'a Apk,
    pub framework: Option<&'a Apk>,
    /// The style whose attributes `?attr` values take.
    pub theme: u32,
    pub night: bool,
}

/// `android:attr/theme`: `<application android:theme>`.
pub const ATTR_THEME: u32 = 0x0101_0000;

impl<'a> Resources<'a> {
    fn table_of(&self, id: u32) -> Option<(&'a Apk, &'a Table)> {
        let package = (id >> 24) as u8;
        [Some(self.app), self.framework]
            .into_iter()
            .flatten()
            .find_map(|apk| {
                let t = apk.table()?;
                t.has_package(package).then_some((apk, t))
            })
    }

    /// Resource `id` in its best configuration, and the APK holding its
    /// files.
    pub fn entry(&self, id: u32) -> Option<(Entry, Config, &'a Apk)> {
        let (apk, table) = self.table_of(id)?;
        let (e, c) = table.get(id, self.night)?;
        Some((e.clone(), c, apk))
    }

    /// `v` with references and theme attributes followed to a plain value,
    /// and the APK a file path in it belongs to.
    pub fn resolve(&self, v: &Value) -> Option<(Value, &'a Apk)> {
        let mut v = v.clone();
        let mut apk = self.app;
        for _ in 0..16 {
            v = match v {
                Value::Ref(0) | Value::Attr(0) | Value::Null => return None,
                Value::Ref(id) => match self.entry(id)? {
                    (Entry::Simple(next), _, a) => {
                        apk = a;
                        next
                    }
                    // A bag stands for itself (a style or an array).
                    (Entry::Bag(..), _, a) => return Some((Value::Ref(id), a)),
                },
                Value::Attr(attr) => self.theme_attr(self.theme, attr)?,
                plain => return Some((plain, apk)),
            };
        }
        None
    }

    /// Attribute `attr` of style `style`, through its parents.
    fn theme_attr(&self, style: u32, attr: u32) -> Option<Value> {
        let mut style = style;
        for _ in 0..32 {
            let (Entry::Bag(parent, map), ..) = self.entry(style)? else {
                return None;
            };
            if let Some((_, v)) = map.iter().find(|(name, _)| *name == attr) {
                return Some(v.clone());
            }
            style = parent;
        }
        None
    }

    /// A color, or a color state list's color for no state, as ARGB.
    pub fn color(&self, v: &Value) -> Option<u32> {
        match self.resolve(v)? {
            (Value::Color(c), _) => Some(c),
            (Value::String(path), apk) if path.ends_with(".xml") => {
                let e = res::xml(&apk.file(&path).ok()?).ok()?;
                if e.name != "selector" {
                    return None;
                }
                let item = e
                    .children
                    .iter()
                    .rev()
                    .find(|i| !i.attrs.iter().any(|a| a.name.starts_with("state_")))?;
                let c = self.color(item.named("color")?)?;
                let alpha = match item.named("alpha").and_then(|a| self.resolve(a)) {
                    Some((Value::Float(a), _)) => a,
                    _ => 1.0,
                };
                let a = ((c >> 24) as f32 * alpha).round() as u32;
                Some(a << 24 | c & 0x00ff_ffff)
            }
            _ => None,
        }
    }

    /// A string resource's text.
    pub fn string(&self, v: &Value) -> Option<String> {
        match self.resolve(v)?.0 {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_access_leaves_resource_decoding_to_its_consumer() {
        let bytes = include_bytes!("fixtures/files.zip").to_vec();
        let files = Files {
            source: Box::new(bytes.clone()),
        };
        assert_eq!(files.file("asset").unwrap(), b"asset bytes");
        assert_eq!(
            files.file("resources.arsc").unwrap(),
            b"not a resource table"
        );
        assert!(files.file_if_present("absent").unwrap().is_none());
        assert!(files.file("absent").is_err());
        // Consumers asking for this reader's resource model still reject it.
        assert!(Apk::from_source(Box::new(bytes)).is_err());
    }

    #[test]
    fn file_access_preserves_archive_and_crc_errors() {
        let mut bytes = include_bytes!("fixtures/files.zip").to_vec();
        let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
        bytes[central + 16] ^= 1;
        let files = Files {
            source: Box::new(bytes),
        };
        assert!(files.file_if_present("resources.arsc").is_err());
        let files = Files {
            source: Box::new(vec![0u8; 32]),
        };
        assert!(files.file_if_present("absent").is_err());
    }
}
