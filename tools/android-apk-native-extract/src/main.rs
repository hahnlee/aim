use flate2::read::DeflateDecoder;
use std::collections::HashSet;
use std::env;
#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const ABI_PREFIX: &[u8] = b"lib/arm64-v8a/";
const MAX_ARCHIVE_SIZE: usize = 512 * 1024 * 1024;
// Current Chromium APKs contain more than 4,096 resource entries while still
// using the classic (non-ZIP64) directory format.
const MAX_ARCHIVE_ENTRIES: usize = 8192;
const MAX_NATIVE_FILES: usize = 64;
// libchrome.so is intentionally monolithic and currently about 240 MiB.
const MAX_NATIVE_FILE_SIZE: usize = 256 * 1024 * 1024;
const MAX_NATIVE_TOTAL_SIZE: usize = 256 * 1024 * 1024;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
struct Entry {
    name: Vec<u8>,
    flags: u16,
    method: u16,
    crc32: u32,
    compressed_size: usize,
    uncompressed_size: usize,
    local_offset: usize,
    unix_mode: u16,
}

#[derive(Debug)]
struct NativeEntry {
    leaf: Vec<u8>,
    method: u16,
    crc32: u32,
    uncompressed_size: usize,
    compressed: Vec<u8>,
}

#[derive(Debug)]
struct Archive {
    native: Vec<NativeEntry>,
}

#[derive(Debug)]
struct Extraction {
    files: usize,
    stored: usize,
    deflated: usize,
    bytes: usize,
}

fn le16(bytes: &[u8], offset: usize, what: &str) -> Result<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| format!("{what} is outside the archive"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn le32(bytes: &[u8], offset: usize, what: &str) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| format!("{what} is outside the archive"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn checked_add(left: usize, right: usize, what: &str) -> Result<usize> {
    left.checked_add(right)
        .ok_or_else(|| format!("{what} overflows addressable size"))
}

fn safe_zip_name(name: &[u8]) -> Result<()> {
    if name.is_empty() || name[0] == b'/' || name.contains(&0) || name.contains(&b'\\') {
        return Err("ZIP entry has an absolute, empty, NUL, or backslash path".to_owned());
    }
    let directory = name.ends_with(b"/");
    let body = if directory {
        &name[..name.len() - 1]
    } else {
        name
    };
    if body.is_empty()
        || body
            .split(|byte| *byte == b'/')
            .any(|part| part.is_empty() || part == b"." || part == b"..")
    {
        return Err("ZIP entry has an empty, dot, or parent path component".to_owned());
    }
    Ok(())
}

fn find_eocd(bytes: &[u8]) -> Result<usize> {
    if bytes.len() < 22 {
        return Err("archive is shorter than a ZIP EOCD".to_owned());
    }
    let floor = bytes.len().saturating_sub(22 + u16::MAX as usize);
    for offset in (floor..=bytes.len() - 22).rev() {
        if le32(bytes, offset, "EOCD signature")? != EOCD_SIGNATURE {
            continue;
        }
        let comment_length = le16(bytes, offset + 20, "EOCD comment length")? as usize;
        if checked_add(offset + 22, comment_length, "EOCD comment")? == bytes.len() {
            return Ok(offset);
        }
    }
    Err("valid terminal ZIP EOCD was not found".to_owned())
}

fn parse_central(bytes: &[u8]) -> Result<(Vec<Entry>, usize)> {
    let eocd = find_eocd(bytes)?;
    let disk = le16(bytes, eocd + 4, "EOCD disk")?;
    let central_disk = le16(bytes, eocd + 6, "EOCD central disk")?;
    let disk_entries = le16(bytes, eocd + 8, "EOCD disk entry count")?;
    let total_entries = le16(bytes, eocd + 10, "EOCD total entry count")?;
    if disk != 0 || central_disk != 0 || disk_entries != total_entries {
        return Err("multi-disk ZIP archives are not supported".to_owned());
    }
    let count = total_entries as usize;
    if count > MAX_ARCHIVE_ENTRIES {
        return Err(format!("ZIP exceeds the {MAX_ARCHIVE_ENTRIES}-entry cap"));
    }
    let central_size = le32(bytes, eocd + 12, "EOCD central size")? as usize;
    let central_offset = le32(bytes, eocd + 16, "EOCD central offset")? as usize;
    if central_size == u32::MAX as usize || central_offset == u32::MAX as usize {
        return Err("ZIP64 archives are not supported".to_owned());
    }
    let central_end = checked_add(central_offset, central_size, "central directory")?;
    if central_end != eocd || central_end > bytes.len() {
        return Err("central directory bounds do not end at the EOCD".to_owned());
    }

    let mut offset = central_offset;
    let mut entries = Vec::with_capacity(count);
    let mut names = HashSet::with_capacity(count);
    for _ in 0..count {
        if le32(bytes, offset, "central signature")? != CENTRAL_SIGNATURE {
            return Err("central directory signature mismatch".to_owned());
        }
        let made_by = le16(bytes, offset + 4, "central creator")?;
        let flags = le16(bytes, offset + 8, "central flags")?;
        let method = le16(bytes, offset + 10, "central compression method")?;
        let crc32 = le32(bytes, offset + 16, "central CRC")?;
        let compressed_u32 = le32(bytes, offset + 20, "central compressed size")?;
        let uncompressed_u32 = le32(bytes, offset + 24, "central uncompressed size")?;
        let name_length = le16(bytes, offset + 28, "central name length")? as usize;
        let extra_length = le16(bytes, offset + 30, "central extra length")? as usize;
        let comment_length = le16(bytes, offset + 32, "central comment length")? as usize;
        let start_disk = le16(bytes, offset + 34, "central start disk")?;
        let external = le32(bytes, offset + 38, "central external attributes")?;
        let local_u32 = le32(bytes, offset + 42, "central local offset")?;
        if compressed_u32 == u32::MAX || uncompressed_u32 == u32::MAX || local_u32 == u32::MAX {
            return Err("ZIP64 entries are not supported".to_owned());
        }
        if start_disk != 0 {
            return Err("multi-disk ZIP entry is not supported".to_owned());
        }
        if flags & 0x2041 != 0 {
            return Err("encrypted or masked ZIP entries are not supported".to_owned());
        }
        let name_start = checked_add(offset, 46, "central fixed header")?;
        let name_end = checked_add(name_start, name_length, "central name")?;
        let record_end = checked_add(
            checked_add(name_end, extra_length, "central extra")?,
            comment_length,
            "central comment",
        )?;
        if record_end > central_end {
            return Err("central directory record exceeds its declared bounds".to_owned());
        }
        let name = bytes[name_start..name_end].to_vec();
        safe_zip_name(&name)?;
        if !names.insert(name.clone()) {
            return Err("duplicate ZIP entry name is forbidden".to_owned());
        }
        let unix_mode = if made_by >> 8 == 3 {
            (external >> 16) as u16
        } else {
            0
        };
        if unix_mode & 0o170000 == 0o120000 {
            return Err("ZIP symlink entries are forbidden".to_owned());
        }
        entries.push(Entry {
            name,
            flags,
            method,
            crc32,
            compressed_size: compressed_u32 as usize,
            uncompressed_size: uncompressed_u32 as usize,
            local_offset: local_u32 as usize,
            unix_mode,
        });
        offset = record_end;
    }
    if offset != central_end {
        return Err("central directory entry count/size mismatch".to_owned());
    }
    Ok((entries, central_offset))
}

fn parse_archive(bytes: &[u8]) -> Result<Archive> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE_SIZE {
        return Err(format!(
            "APK is outside the 1..={MAX_ARCHIVE_SIZE} byte archive cap"
        ));
    }
    let (entries, central_offset) = parse_central(bytes)?;
    let mut native = Vec::new();
    let mut native_names = HashSet::new();
    let mut native_total = 0_usize;
    let mut local_ranges = Vec::<(usize, usize)>::with_capacity(entries.len());

    for entry in entries {
        let offset = entry.local_offset;
        if le32(bytes, offset, "local signature")? != LOCAL_SIGNATURE {
            return Err("local header signature mismatch".to_owned());
        }
        let local_flags = le16(bytes, offset + 6, "local flags")?;
        let local_method = le16(bytes, offset + 8, "local method")?;
        let local_crc = le32(bytes, offset + 14, "local CRC")?;
        let local_compressed = le32(bytes, offset + 18, "local compressed size")?;
        let local_uncompressed = le32(bytes, offset + 22, "local uncompressed size")?;
        let name_length = le16(bytes, offset + 26, "local name length")? as usize;
        let extra_length = le16(bytes, offset + 28, "local extra length")? as usize;
        let name_start = checked_add(offset, 30, "local fixed header")?;
        let name_end = checked_add(name_start, name_length, "local name")?;
        let data_start = checked_add(name_end, extra_length, "local extra")?;
        let data_end = checked_add(data_start, entry.compressed_size, "local data")?;
        if data_end > central_offset {
            return Err("local header/data crosses the central directory".to_owned());
        }
        if bytes.get(name_start..name_end) != Some(entry.name.as_slice())
            || local_flags != entry.flags
            || local_method != entry.method
        {
            return Err("central/local filename, flags, or method mismatch".to_owned());
        }
        if entry.flags & 0x0008 == 0
            && (local_crc != entry.crc32
                || local_compressed as usize != entry.compressed_size
                || local_uncompressed as usize != entry.uncompressed_size)
        {
            return Err("central/local CRC or size mismatch".to_owned());
        }
        local_ranges.push((offset, data_end));

        if !entry.name.starts_with(ABI_PREFIX) || entry.name == ABI_PREFIX {
            continue;
        }
        let leaf = &entry.name[ABI_PREFIX.len()..];
        if leaf.is_empty() || leaf.contains(&b'/') || !leaf.ends_with(b".so") {
            return Err("arm64-v8a native entry must be a direct *.so child".to_owned());
        }
        if entry.unix_mode & 0o170000 != 0 && entry.unix_mode & 0o170000 != 0o100000 {
            return Err("arm64-v8a native entry is not a regular file".to_owned());
        }
        if entry.method != 0 && entry.method != 8 {
            return Err("arm64-v8a native entry must use stored or deflate compression".to_owned());
        }
        if entry.compressed_size > MAX_NATIVE_FILE_SIZE
            || entry.uncompressed_size == 0
            || entry.uncompressed_size > MAX_NATIVE_FILE_SIZE
        {
            return Err(format!(
                "native library is outside the 1..={MAX_NATIVE_FILE_SIZE} byte file cap"
            ));
        }
        if native.len() >= MAX_NATIVE_FILES {
            return Err(format!(
                "APK exceeds the {MAX_NATIVE_FILES}-native-library cap"
            ));
        }
        native_total = checked_add(native_total, entry.uncompressed_size, "native total size")?;
        if native_total > MAX_NATIVE_TOTAL_SIZE {
            return Err(format!(
                "APK exceeds the {MAX_NATIVE_TOTAL_SIZE}-byte native-library total cap"
            ));
        }
        if !native_names.insert(leaf.to_vec()) {
            return Err("two APK entries map to the same native sibling filename".to_owned());
        }
        native.push(NativeEntry {
            leaf: leaf.to_vec(),
            method: entry.method,
            crc32: entry.crc32,
            uncompressed_size: entry.uncompressed_size,
            compressed: bytes[data_start..data_end].to_vec(),
        });
    }
    local_ranges.sort_unstable();
    for pair in local_ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err("ZIP local header/data ranges overlap".to_owned());
        }
    }
    if native.is_empty() {
        return Err("APK has no lib/arm64-v8a/*.so entries".to_owned());
    }
    Ok(Archive { native })
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn decode(entry: &NativeEntry) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(entry.uncompressed_size);
    match entry.method {
        0 => output.extend_from_slice(&entry.compressed),
        8 => {
            let mut decoder = DeflateDecoder::new(Cursor::new(&entry.compressed));
            decoder
                .by_ref()
                .take((MAX_NATIVE_FILE_SIZE + 1) as u64)
                .read_to_end(&mut output)
                .map_err(|error| format!("deflate stream failed: {error}"))?;
            if decoder.total_in() != entry.compressed.len() as u64 {
                return Err("deflate stream did not consume its exact ZIP data range".to_owned());
            }
        }
        _ => unreachable!("method checked while parsing"),
    }
    if output.len() != entry.uncompressed_size {
        return Err("native library decompressed size does not match central directory".to_owned());
    }
    if crc32(&output) != entry.crc32 {
        return Err("native library CRC32 mismatch".to_owned());
    }
    Ok(output)
}

struct StageGuard(Option<PathBuf>);

impl StageGuard {
    fn path(&self) -> &Path {
        self.0.as_deref().expect("live stage has a path")
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        let Some(path) = self.0.as_ref() else {
            return;
        };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o600));
            }
        }
        let _ = fs::remove_dir_all(path);
    }
}

fn make_stage(parent: &Path) -> Result<StageGuard> {
    for attempt in 0..128_u32 {
        let candidate = parent.join(format!(
            ".aim-apk-extract.{}.{}",
            std::process::id(),
            attempt
        ));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&candidate) {
            Ok(()) => return Ok(StageGuard(Some(candidate))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not create extraction staging dir: {error}")),
        }
    }
    Err("could not allocate a unique extraction staging dir".to_owned())
}

#[cfg(target_os = "macos")]
fn publish_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    const RENAME_EXCL: u32 = 0x0000_0004;
    unsafe extern "C" {
        fn renamex_np(old: *const i8, new: *const i8, flags: u32) -> i32;
    }
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
    })?;
    // SAFETY: both C strings remain live for the call and RENAME_EXCL is a
    // Darwin-defined flag. renamex_np does not retain either pointer.
    if unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn publish_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    fs::rename(source, destination)
}

fn extract(archive: Archive, destination: &Path, root: &[u8]) -> Result<Extraction> {
    let leaf = destination
        .file_name()
        .ok_or_else(|| "destination must name a new directory".to_owned())?;
    if leaf == OsStr::new(".") || leaf == OsStr::new("..") {
        return Err("destination must name a new directory".to_owned());
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("could not canonicalize destination parent: {error}"))?;
    if !fs::metadata(&parent)
        .map_err(|error| format!("could not inspect destination parent: {error}"))?
        .is_dir()
    {
        return Err("destination parent is not a directory".to_owned());
    }
    let destination = parent.join(leaf);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err("destination already exists; replacement is forbidden".to_owned());
    }
    if !archive.native.iter().any(|entry| entry.leaf == root) {
        return Err("requested root SONAME is absent from lib/arm64-v8a".to_owned());
    }

    let mut stage = make_stage(&parent)?;
    let mut stored = 0;
    let mut deflated = 0;
    let mut total = 0;
    for entry in &archive.native {
        let bytes = decode(entry)?;
        let output = stage.path().join(OsString::from_vec(entry.leaf.clone()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&output)
            .map_err(|error| format!("could not create native sibling: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("could not write native sibling: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("could not sync native sibling: {error}"))?;
        fs::set_permissions(&output, fs::Permissions::from_mode(0o400))
            .map_err(|error| format!("could not make native sibling read-only: {error}"))?;
        total += bytes.len();
        if entry.method == 0 {
            stored += 1;
        } else {
            deflated += 1;
        }
    }
    fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o500))
        .map_err(|error| format!("could not seal extraction directory: {error}"))?;
    File::open(stage.path())
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("could not sync extraction directory: {error}"))?;
    publish_exclusive(stage.path(), &destination)
        .map_err(|error| format!("could not atomically publish extraction directory: {error}"))?;
    stage.disarm();
    // Publication is already atomic. A parent-directory fsync is best-effort on
    // filesystems that reject directory syncing; that does not make the visible
    // tree partial or writable.
    let _ = File::open(&parent).and_then(|directory| directory.sync_all());
    Ok(Extraction {
        files: archive.native.len(),
        stored,
        deflated,
        bytes: total,
    })
}

fn run() -> Result<()> {
    let arguments = env::args_os().collect::<Vec<_>>();
    if arguments.len() != 4 {
        return Err("usage: android-apk-native-extract APK NEW_DIRECTORY ROOT_SONAME".to_owned());
    }
    let mut apk_file =
        File::open(&arguments[1]).map_err(|error| format!("could not open APK: {error}"))?;
    let declared_length = usize::try_from(
        apk_file
            .metadata()
            .map_err(|error| format!("could not inspect APK: {error}"))?
            .len(),
    )
    .map_err(|_| "APK length does not fit addressable size".to_owned())?;
    if declared_length == 0 || declared_length > MAX_ARCHIVE_SIZE {
        return Err(format!(
            "APK is outside the 1..={MAX_ARCHIVE_SIZE} byte archive cap"
        ));
    }
    let mut apk = Vec::with_capacity(declared_length);
    Read::by_ref(&mut apk_file)
        .take((MAX_ARCHIVE_SIZE + 1) as u64)
        .read_to_end(&mut apk)
        .map_err(|error| format!("could not read APK: {error}"))?;
    if apk.len() != declared_length {
        return Err("APK changed size while its opened descriptor was read".to_owned());
    }
    let archive = parse_archive(&apk)?;
    let root = arguments[3].as_bytes();
    safe_zip_name(root)?;
    if root.contains(&b'/') || !root.ends_with(b".so") {
        return Err("root SONAME must be one direct *.so filename".to_owned());
    }
    let outcome = extract(archive, Path::new(&arguments[2]), root)?;
    println!(
        "apk-native-extract: PASS files={} stored={} deflated={} bytes={} crc=verified mode=dir0500+file0400 publish=atomic root={}",
        outcome.files,
        outcome.stored,
        outcome.deflated,
        outcome.bytes,
        String::from_utf8_lossy(root)
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("apk-native-extract: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    struct ZipInput<'a> {
        name: &'a [u8],
        data: &'a [u8],
        method: u16,
    }

    fn push16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn zip(inputs: &[ZipInput<'_>]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut records = Vec::new();
        for input in inputs {
            let compressed = if input.method == 8 {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
                encoder.write_all(input.data).unwrap();
                encoder.finish().unwrap()
            } else {
                input.data.to_vec()
            };
            let local = output.len() as u32;
            push32(&mut output, LOCAL_SIGNATURE);
            push16(&mut output, 20);
            push16(&mut output, 0);
            push16(&mut output, input.method);
            push16(&mut output, 0);
            push16(&mut output, 0);
            push32(&mut output, crc32(input.data));
            push32(&mut output, compressed.len() as u32);
            push32(&mut output, input.data.len() as u32);
            push16(&mut output, input.name.len() as u16);
            push16(&mut output, 0);
            output.extend_from_slice(input.name);
            output.extend_from_slice(&compressed);
            records.push((input, compressed.len(), local));
        }
        let central = output.len() as u32;
        for (input, compressed, local) in &records {
            push32(&mut output, CENTRAL_SIGNATURE);
            push16(&mut output, 3 << 8 | 20);
            push16(&mut output, 20);
            push16(&mut output, 0);
            push16(&mut output, input.method);
            push16(&mut output, 0);
            push16(&mut output, 0);
            push32(&mut output, crc32(input.data));
            push32(&mut output, *compressed as u32);
            push32(&mut output, input.data.len() as u32);
            push16(&mut output, input.name.len() as u16);
            push16(&mut output, 0);
            push16(&mut output, 0);
            push16(&mut output, 0);
            push16(&mut output, 0);
            push32(&mut output, 0o100644 << 16);
            push32(&mut output, *local);
            output.extend_from_slice(input.name);
        }
        let central_size = output.len() as u32 - central;
        push32(&mut output, EOCD_SIGNATURE);
        push16(&mut output, 0);
        push16(&mut output, 0);
        push16(&mut output, inputs.len() as u16);
        push16(&mut output, inputs.len() as u16);
        push32(&mut output, central_size);
        push32(&mut output, central);
        push16(&mut output, 0);
        output
    }

    #[test]
    fn accepts_stored_and_deflate_as_arbitrary_payload_bytes() {
        let bytes = zip(&[
            ZipInput {
                name: b"lib/arm64-v8a/libroot.so",
                data: b"\0\xffELF-ish",
                method: 0,
            },
            ZipInput {
                name: b"lib/arm64-v8a/libchild.so",
                data: &[0, 1, 2, 3, 255, 0, 99],
                method: 8,
            },
        ]);
        let archive = parse_archive(&bytes).unwrap();
        assert_eq!(archive.native.len(), 2);
        assert_eq!(decode(&archive.native[0]).unwrap(), b"\0\xffELF-ish");
        assert_eq!(
            decode(&archive.native[1]).unwrap(),
            [0, 1, 2, 3, 255, 0, 99]
        );
    }

    #[test]
    fn rejects_traversal_duplicate_crc_and_local_mismatch() {
        let traversal = zip(&[ZipInput {
            name: b"lib/arm64-v8a/../evil.so",
            data: b"x",
            method: 0,
        }]);
        assert!(parse_archive(&traversal).unwrap_err().contains("parent"));

        let duplicate = zip(&[
            ZipInput {
                name: b"lib/arm64-v8a/libx.so",
                data: b"x",
                method: 0,
            },
            ZipInput {
                name: b"lib/arm64-v8a/libx.so",
                data: b"y",
                method: 0,
            },
        ]);
        assert!(parse_archive(&duplicate).unwrap_err().contains("duplicate"));

        let mut bad_crc = zip(&[ZipInput {
            name: b"lib/arm64-v8a/libx.so",
            data: b"x",
            method: 0,
        }]);
        bad_crc[30 + b"lib/arm64-v8a/libx.so".len()] ^= 1;
        let archive = parse_archive(&bad_crc).unwrap();
        assert!(decode(&archive.native[0]).unwrap_err().contains("CRC32"));

        let mut mismatch = zip(&[ZipInput {
            name: b"lib/arm64-v8a/libx.so",
            data: b"x",
            method: 0,
        }]);
        mismatch[8] = 8;
        assert!(parse_archive(&mismatch).unwrap_err().contains("mismatch"));
    }

    #[test]
    fn rejects_central_bounds_and_declared_native_cap() {
        let mut truncated = zip(&[ZipInput {
            name: b"lib/arm64-v8a/libx.so",
            data: b"x",
            method: 0,
        }]);
        truncated.pop();
        assert!(parse_archive(&truncated).is_err());

        let mut oversized = zip(&[ZipInput {
            name: b"lib/arm64-v8a/libx.so",
            data: b"x",
            method: 0,
        }]);
        let eocd = find_eocd(&oversized).unwrap();
        let central = le32(&oversized, eocd + 16, "offset").unwrap() as usize;
        oversized[22..26].copy_from_slice(&((MAX_NATIVE_FILE_SIZE + 1) as u32).to_le_bytes());
        oversized[central + 24..central + 28]
            .copy_from_slice(&((MAX_NATIVE_FILE_SIZE + 1) as u32).to_le_bytes());
        assert!(parse_archive(&oversized).unwrap_err().contains("file cap"));
    }

    #[test]
    fn failed_extraction_is_not_published_and_stage_is_removed() {
        let mut bytes = zip(&[ZipInput {
            name: b"lib/arm64-v8a/libroot.so",
            data: b"root",
            method: 0,
        }]);
        bytes[30 + b"lib/arm64-v8a/libroot.so".len()] ^= 1;
        let archive = parse_archive(&bytes).unwrap();
        let parent =
            env::temp_dir().join(format!("apk-native-extract-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir(&parent).unwrap();
        let destination = parent.join("published");
        assert!(extract(archive, &destination, b"libroot.so").is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn exclusive_publication_never_replaces_an_existing_directory() {
        let parent = env::temp_dir().join(format!(
            "apk-native-extract-exclusive-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir(&parent).unwrap();
        let source = parent.join("source");
        let destination = parent.join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        assert!(publish_exclusive(&source, &destination).is_err());
        assert!(source.is_dir());
        assert!(destination.is_dir());
        fs::remove_dir_all(&parent).unwrap();
    }
}
