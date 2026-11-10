//! An APK: its files, manifest and resources, and resolving a resource
//! against the app's table, the framework's (`framework-res.apk`) and the
//! app's theme.

use std::path::Path;

use std::io::Read;

use android_image_extract::source::{FileSource, ReadAt};
use android_image_extract::zip::{self, Archive, DEFLATE, STORED};

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

    /// The file `name` as an asset: androidfw's ZipAssetsProvider::OpenInternal
    /// (android-16.0.0_r1). An entry libziparchive's FindEntry rejects is
    /// absent, as ApkAssets treats a missing resources.arsc; a stored entry is
    /// mapped and a deflated one inflated to its declared size, neither
    /// checked against its CRC-32. The archive opens as the general ZIP
    /// reader opens it and, as OpenArchive, fails on a name given twice;
    /// OpenArchive's other entry checks (name validity, local header
    /// offsets past the directory) are not applied.
    pub fn asset(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let archive = Archive::open(self.source.as_ref()).map_err(|e| bad(e.to_string()))?;
        // OpenArchive's AddToMap: a name twice fails the whole archive.
        let mut names = std::collections::HashSet::new();
        if !archive.entries.iter().all(|e| names.insert(&e.name)) {
            return Err(bad("duplicate ZIP entry name"));
        }
        let Some(entry) = archive.find(name.as_bytes()) else {
            return Ok(None);
        };
        let Some(offset) = find_entry(&archive, entry)? else {
            return Ok(None);
        };
        let failed = |e: String| bad(format!("{name}: {e}"));
        if entry.size > MAX_ENTRY {
            return Err(failed("entry exceeds the in-memory limit".into()));
        }
        let source = archive.source();
        if entry.method != DEFLATE {
            return source
                .read_vec(offset, entry.size as usize)
                .map(Some)
                .map_err(|e| failed(e.to_string()));
        }
        let compressed = source
            .read_vec(offset, entry.compressed_size as usize)
            .map_err(|e| failed(e.to_string()))?;
        // zip_archive::Inflate: exactly the declared size, the input consumed.
        let mut out = Vec::with_capacity(entry.size as usize);
        let mut decoder = flate2::read::DeflateDecoder::new(compressed.as_slice());
        (&mut decoder)
            .take(entry.size + 1)
            .read_to_end(&mut out)
            .map_err(|e| failed(e.to_string()))?;
        if out.len() as u64 != entry.size {
            return Err(failed(
                "inflated size differs from the declared size".into(),
            ));
        }
        Ok(Some(out))
    }

    /// [`Files::asset`] that must be there, as AssetManager2 opens a file a
    /// resource names.
    pub fn asset_file(&self, name: &str) -> Result<Vec<u8>> {
        self.asset(name)?.ok_or_else(|| bad(format!("no {name}")))
    }
}

/// ParseZip64ExtendedInfoInExtraField for a local header whose sizes are
/// both `UINT32_MAX`: the extended information's uncompressed and
/// compressed sizes, `None` where the original fails. Its data holds those
/// two, or all three fields with the header offset (and a disk number the
/// original ignores), nothing else.
fn zip64_local_sizes(extra: &[u8]) -> Option<(u64, u64)> {
    if extra.len() <= 4 {
        return None;
    }
    let mut at = 0;
    while at < extra.len() - 4 {
        let id = u16::from_le_bytes([extra[at], extra[at + 1]]);
        let mut len = usize::from(u16::from_le_bytes([extra[at + 2], extra[at + 3]]));
        at += 4;
        if len > extra.len() - at {
            return None;
        }
        if id != 1 {
            at += len;
            continue;
        }
        if len == 28 {
            len = 24;
        }
        let read = if len == 24 { 24 } else { 16 };
        if len != read || at + read > extra.len() {
            return None;
        }
        let field = |i: usize| u64::from_le_bytes(extra[i..i + 8].try_into().unwrap());
        return Some((field(at), field(at + 8)));
    }
    None
}

/// libziparchive FindEntry's local-header checks for `entry`: its data
/// offset, or `None` where the original returns an error (the caller's
/// asset is then absent). Reading the host file failing is an error.
fn find_entry(archive: &Archive<'_>, entry: &zip::Entry) -> Result<Option<u64>> {
    const LOCAL_HEADER: u64 = 30;
    let cd = archive.central_offset;
    let source = archive.source();
    let read = |offset: u64, len: u64| {
        source
            .read_vec(offset, len as usize)
            .map_err(|e| bad(e.to_string()))
    };
    let le16 = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let le32 = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let name_len = entry.name.len() as u64;
    // ZipEntry::CopyFromZipEntry64: AssetsProvider takes 32-bit sizes.
    if entry.name.is_empty()
        || entry.size > u64::from(u32::MAX)
        || entry.compressed_size > u64::from(u32::MAX)
        || entry.local_offset.saturating_add(LOCAL_HEADER) >= cd
    {
        return Ok(None);
    }
    let lfh = read(entry.local_offset, LOCAL_HEADER)?;
    let name_at = entry.local_offset + LOCAL_HEADER;
    if lfh[..4] != *b"PK\x03\x04"
        || u64::from(le16(&lfh, 26)) != name_len
        || name_at + name_len > cd
        || read(name_at, name_len)? != entry.name
    {
        return Ok(None);
    }
    let extra_at = name_at + name_len;
    let extra_len = u64::from(le16(&lfh, 28));
    if extra_at > cd.saturating_sub(extra_len) {
        return Ok(None);
    }
    let (mut size, mut compressed) = (u64::from(le32(&lfh, 22)), u64::from(le32(&lfh, 18)));
    if size == u64::from(u32::MAX) || compressed == u64::from(u32::MAX) {
        if size != compressed {
            return Ok(None);
        }
        let Some(sizes) = zip64_local_sizes(&read(extra_at, extra_len)?) else {
            return Ok(None);
        };
        (size, compressed) = sizes;
    }
    // Without a data descriptor (bit 3) the local header must agree.
    if le16(&lfh, 6) & 8 == 0
        && (compressed != entry.compressed_size
            || size != entry.size
            || le32(&lfh, 14) != entry.crc32)
    {
        return Ok(None);
    }
    let data = extra_at + extra_len;
    if data > cd
        || entry.compressed_size > cd - data
        || (entry.method == STORED && entry.size > cd - data)
    {
        return Ok(None);
    }
    Ok(Some(data))
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
            .asset("resources.arsc")?
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

    /// [`Files::asset`].
    pub fn asset(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.files.asset(name)
    }

    /// [`Files::asset_file`].
    pub fn asset_file(&self, name: &str) -> Result<Vec<u8>> {
        self.files.asset_file(name)
    }

    pub fn manifest(&self) -> Result<Element> {
        res::xml(&self.asset_file("AndroidManifest.xml")?)
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

    /// A ZIP of (name, method, declared size, CRC-32, payload) entries.
    fn archive(entries: &[(&str, u16, u32, u32, &[u8])]) -> Vec<u8> {
        let (mut out, mut central) = (Vec::new(), Vec::new());
        for (name, method, size, crc, payload) in entries {
            let offset = out.len() as u32;
            let sizes = [*crc, payload.len() as u32, *size];
            out.extend(b"PK\x03\x04");
            [20u16, 0, *method, 0, 0].map(|v| out.extend(v.to_le_bytes()));
            sizes.map(|v| out.extend(v.to_le_bytes()));
            [name.len() as u16, 0].map(|v| out.extend(v.to_le_bytes()));
            out.extend(name.as_bytes());
            out.extend(*payload);
            central.extend(b"PK\x01\x02");
            [20u16, 20, 0, *method, 0, 0].map(|v| central.extend(v.to_le_bytes()));
            sizes.map(|v| central.extend(v.to_le_bytes()));
            [name.len() as u16, 0, 0, 0, 0].map(|v| central.extend(v.to_le_bytes()));
            [0u32, offset].map(|v| central.extend(v.to_le_bytes()));
            central.extend(name.as_bytes());
        }
        let offset = out.len() as u32;
        out.extend(&central);
        out.extend(b"PK\x05\x06");
        let count = entries.len() as u16;
        [0u16, 0, count, count].map(|v| out.extend(v.to_le_bytes()));
        [central.len() as u32, offset].map(|v| out.extend(v.to_le_bytes()));
        out.extend(0u16.to_le_bytes());
        out
    }

    fn files(bytes: Vec<u8>) -> Files {
        Files {
            source: Box::new(bytes),
        }
    }

    #[test]
    fn assets_follow_find_entry_and_skip_the_crc() {
        let payload = b"invalid!".as_slice();
        let bad_crc = crc32(payload) ^ 1;
        // A stored entry whose declared size overruns the data before the
        // central directory fails FindEntry: the asset is absent.
        let oversize = files(archive(&[(
            "resources.arsc",
            0,
            (512 << 20) + 1,
            bad_crc,
            payload,
        )]));
        assert!(oversize.asset("resources.arsc").unwrap().is_none());
        assert!(oversize.file_if_present("resources.arsc").is_err());
        let apk = Apk::from_source(Box::new(archive(&[(
            "resources.arsc",
            0,
            (512 << 20) + 1,
            bad_crc,
            payload,
        )])))
        .unwrap();
        assert!(apk.table().is_none());
        // A stored entry is mapped as declared, its CRC-32 unchecked.
        let mapped = files(archive(&[("a", 0, 8, bad_crc, payload)]));
        assert_eq!(mapped.asset("a").unwrap().unwrap(), payload);
        assert!(mapped.file_if_present("a").is_err());
        // Its declared size may run past its payload, short of the directory.
        let over = files(archive(&[("a", 0, 12, 0, b"ab"), ("b", 0, 2, 0, b"cd")]));
        assert_eq!(&over.asset("a").unwrap().unwrap()[..2], b"ab");
        // A deflated entry inflates to exactly its declared size.
        let deflated = [0x4b, 0x4c, 0x4a, 0x06, 0x00]; // "abc"
        let crc = crc32(b"abc");
        let good = files(archive(&[("d", 8, 3, crc ^ 1, &deflated)]));
        assert_eq!(good.asset("d").unwrap().unwrap(), b"abc");
        for size in [2, 4] {
            let bad = files(archive(&[("d", 8, size, crc, &deflated)]));
            assert!(bad.asset("d").is_err());
        }
        assert!(
            files(archive(&[("d", 8, 8, 0, b"\x07")]))
                .asset("d")
                .is_err()
        );
        assert!(good.asset("absent").unwrap().is_none());
    }

    #[test]
    fn assets_are_absent_where_the_local_header_disagrees() {
        let payload = b"payload".as_slice();
        let crc = crc32(payload);
        let mut bytes = archive(&[("a", 0, 7, crc, payload)]);
        // The local header's CRC-32 field (without a data descriptor).
        bytes[14] ^= 1;
        assert!(files(bytes).asset("a").unwrap().is_none());
        let mut bytes = archive(&[("a", 0, 7, crc, payload)]);
        bytes[0] = b'X';
        assert!(files(bytes).asset("a").unwrap().is_none());
        // With a data descriptor the local header's sizes are not compared.
        let mut bytes = archive(&[("a", 0, 7, crc, payload)]);
        bytes[6] |= 8;
        bytes[14] ^= 1;
        assert_eq!(files(bytes).asset("a").unwrap().unwrap(), payload);
    }

    #[test]
    fn assets_bound_only_stored_sizes_and_fail_on_duplicate_names() {
        // An unknown method is found and mapped at its declared size; only
        // a stored entry's size is bounded by the central directory.
        let unknown = files(archive(&[("u", 12, 40, 0, b"ab")]));
        assert_eq!(unknown.asset("u").unwrap().unwrap().len(), 40);
        assert!(
            files(archive(&[("u", 0, 40, 0, b"ab")]))
                .asset("u")
                .unwrap()
                .is_none()
        );
        // Past the file it cannot be mapped: a load error, not absence.
        assert!(
            files(archive(&[("u", 12, 4096, 0, b"ab")]))
                .asset("u")
                .is_err()
        );
        // OpenArchive fails the whole archive on a name given twice.
        let twice = files(archive(&[
            ("a", 0, 1, crc32(b"x"), b"x"),
            ("a", 0, 1, crc32(b"y"), b"y"),
        ]));
        assert!(twice.asset("other").is_err());
        // A file a resource names must be there.
        let one = files(archive(&[("res/a.xml", 0, 1, 0, b"x")]));
        assert_eq!(one.asset_file("res/a.xml").unwrap(), b"x");
        assert!(one.asset_file("res/b.xml").is_err());
    }

    #[test]
    fn zip64_local_extra_holds_exactly_the_sizes_or_all_three_fields() {
        let field = |id: u16, data: &[u8]| {
            let mut out = id.to_le_bytes().to_vec();
            out.extend((data.len() as u16).to_le_bytes());
            out.extend(data);
            out
        };
        let sizes: Vec<u8> = [7u64, 5, 99]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .chain([0; 4])
            .collect();
        for len in [16, 24, 28] {
            let extra = [field(0xcafe, b"pad"), field(1, &sizes[..len])].concat();
            assert_eq!(zip64_local_sizes(&extra), Some((7, 5)), "{len}");
        }
        for len in [8, 20, 26] {
            assert_eq!(zip64_local_sizes(&field(1, &sizes[..len])), None, "{len}");
        }
        let mut overrun = field(1, &sizes[..16]);
        overrun[2] = 17;
        assert_eq!(zip64_local_sizes(&overrun), None);
        assert_eq!(zip64_local_sizes(&field(0xcafe, &sizes[..16])), None);
        assert_eq!(zip64_local_sizes(&[1, 0, 0, 0]), None);
    }

    /// CRC-32 (IEEE), for building fixtures.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
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
