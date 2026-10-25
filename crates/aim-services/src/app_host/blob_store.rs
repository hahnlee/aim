//! `blob_store`: BlobStoreManagerService for the app host
//! (`android-16.0.0_r1`): data blobs apps commit, share and lease, kept
//! as the original keeps them in `/data/system/blobstore`, the sessions in
//! `sessions_index.xml`, the committed blobs, their committers and their
//! leasees in `blobs_index.xml` (`FastXmlSerializer`, version 6) and each
//! blob's data in `blobs/<id>`. A session (`IBlobStoreSession`) is the
//! caller's own until it commits it; a commit verifies the data against
//! the handle's SHA-256 digest before the blob is committed, on the
//! service's own thread as the original's handler does. Access follows the
//! committers' access modes (private, public, same signature, allowed
//! packages), leases count against the caller's quota, and the limits are
//! DeviceConfig `blobstore`'s. A package whose data is cleared loses its
//! sessions, commits and leases (`handlePackageRemoved`; the app host
//! removes no package or user). Writing a session's data and revocable
//! reads need revocable file descriptors over AppFuse, which the app host
//! does not have; they, the idle maintenance job (a job of the `android`
//! package, #1518), `dump`, the shell command, the storage stats
//! augmenter and the statsd atoms are not implemented (#1938).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aim_android_xml::Element;
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader,
    Result as ParcelResult, StatusCode,
};
use aim_service_aidl::android_app_blob_iblobcommitcallback as callback;
use aim_service_aidl::android_app_blob_iblobstoremanager as bs;
use aim_service_aidl::android_app_blob_iblobstoresession as session;
use aim_service_aidl::android_os_iremotecallback as remote_callback;
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use aim_storage::guest_inode::{self, GuestInode};
use sha2::{Digest, Sha256};

use super::package_writes::{DataCleared, PackageMonitor};
use super::settings::Table;
use super::{AppHost, PER_USER_RANGE, is_isolated, reply};

/// `UserHandle.USER_ALL`.
const USER_ALL: i32 = -1;
use crate::package::query::Query;
use crate::shell::SHELL_COMMAND_TRANSACTION;

/// `BlobStoreConfig.XML_VERSION_*`.
const XML_VERSION_ADD_STRING_DESC: i32 = 2;
const XML_VERSION_ADD_DESC_RES_NAME: i32 = 3;
const XML_VERSION_ADD_COMMIT_TIME: i32 = 4;
const XML_VERSION_ADD_SESSION_CREATION_TIME: i32 = 5;
const XML_VERSION_ALLOW_ACCESS_ACROSS_USERS: i32 = 6;
const XML_VERSION_CURRENT: i32 = XML_VERSION_ALLOW_ACCESS_ACROSS_USERS;

/// `BlobStoreConfig`'s files under `/data/system`.
const ROOT_DIR_NAME: &str = "blobstore";
const BLOBS_DIR_NAME: &str = "blobs";
const SESSIONS_INDEX_FILE_NAME: &str = "sessions_index.xml";
const BLOBS_INDEX_FILE_NAME: &str = "blobs_index.xml";

/// `BlobAccessMode.ACCESS_TYPE_*`.
const ACCESS_TYPE_PRIVATE: i32 = 1 << 0;
const ACCESS_TYPE_PUBLIC: i32 = 1 << 1;
const ACCESS_TYPE_SAME_SIGNATURE: i32 = 1 << 2;
const ACCESS_TYPE_ALLOWLIST: i32 = 1 << 3;

/// `BlobStoreSession.STATE_*`.
const STATE_CLOSED: i32 = 0;
const STATE_OPENED: i32 = 1;
const STATE_ABANDONED: i32 = 2;
const STATE_COMMITTED: i32 = 3;
const STATE_VERIFIED_VALID: i32 = 4;
const STATE_VERIFIED_INVALID: i32 = 5;

/// `BlobStoreManager.COMMIT_RESULT_*` and its limits.
const COMMIT_RESULT_SUCCESS: i32 = 0;
const COMMIT_RESULT_ERROR: i32 = 1;
const MAX_PACKAGE_NAME_LENGTH: usize = 223;
const MAX_CERTIFICATE_LENGTH: usize = 32;
/// `BlobHandle`'s algorithm and limits.
const ALGO_SHA_256: &str = "SHA-256";
const LIMIT_BLOB_TAG_LENGTH: usize = 128;
const LIMIT_BLOB_LABEL_LENGTH: usize = 100;

/// `BlobStoreConfig.DeviceConfigProperties`: DeviceConfig `blobstore`'s
/// keys and their defaults.
const TOTAL_BYTES_PER_APP_LIMIT_FLOOR: (&str, i64) = ("total_bytes_per_app_limit_floor", 300 << 20);
const TOTAL_BYTES_PER_APP_LIMIT_FRACTION: (&str, f32) =
    ("total_bytes_per_app_limit_fraction", 0.01);
const LEASE_ACQUISITION_WAIT_DURATION_MS: (&str, i64) =
    ("lease_acquisition_wait_time_ms", 6 * 3_600_000);
const COMMIT_COOL_OFF_DURATION_MS: (&str, i64) = ("commit_cool_off_duration_ms", 48 * 3_600_000);
const USE_REVOCABLE_FD_FOR_READS: (&str, bool) = ("use_revocable_fd_for_reads", false);
const DELETE_ON_LAST_LEASE_DELAY_MS: (&str, i64) = ("delete_on_last_lease_delay_ms", 6 * 3_600_000);
const MAX_ACTIVE_SESSIONS: (&str, i32) = ("max_active_sessions", 250);
const MAX_COMMITTED_BLOBS: (&str, i32) = ("max_committed_blobs", 1000);
const MAX_LEASED_BLOBS: (&str, i32) = ("max_leased_blobs", 500);
const MAX_BLOB_ACCESS_PERMITTED_PACKAGES: (&str, i32) = ("max_permitted_pks", 300);
const LEASE_DESC_CHAR_LIMIT: (&str, i32) = ("lease_desc_char_limit", 300);

const DUMP: &str = "android.permission.DUMP";
/// `Process.SYSTEM_UID`; `Process.FIRST_SDK_SANDBOX_UID`, `LAST_SDK_SANDBOX_UID`.
const SYSTEM_UID: i32 = 1000;
const SDK_SANDBOX_UIDS: std::ops::RangeInclusive<i32> = 20_000..=29_999;
/// `UserHandle.USER_CURRENT`.
const USER_CURRENT: i32 = -2;
/// `PackageManager.SIGNATURE_MATCH`, `CERT_INPUT_SHA256`.
const SIGNATURE_MATCH: i32 = 0;
const CERT_INPUT_SHA256: i32 = 1;
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");

/// What system_server makes in `/data/system` (umask 077).
const SYSTEM_DIR: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o700),
};
const SYSTEM_FILE: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o600),
};

type Result<T> = std::result::Result<T, Exception>;
/// A call's answer: its result, or the parcel unread.
type CallResult = std::result::Result<Result<()>, StatusCode>;

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn npe(message: &str) -> Exception {
    Exception::new(EX_NULL_POINTER, message)
}

fn illegal_state(message: String) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

/// `new ParcelableException(cause)` of a `class` thrown with `message`:
/// written as `writeParcelable` writes it, its message the cause's
/// `toString`.
fn parcelable_exception(class: &str, message: &str) -> Exception {
    let mut p = Parcel::new();
    p.write_string16(Some("android.os.ParcelableException"));
    p.write_string16(Some(class));
    p.write_string16(Some(message));
    Exception::parcelable(Some(&format!("{class}: {message}")), &p)
        .expect("a message has no objects")
}

fn limit_exceeded(message: &str) -> Exception {
    parcelable_exception("android.os.LimitExceededException", message)
}

/// `ExceptionUtils.wrap` of the `IOException` `ErrnoException
/// .rethrowAsIOException` makes of a failed `open`.
fn io_exception(e: &std::io::Error) -> Exception {
    let errno = e.raw_os_error().unwrap_or(libc::EIO);
    parcelable_exception(
        "java.io.IOException",
        &format!("open failed: {} ({})", errno_name(errno), strerror(errno)),
    )
}

/// `OsConstants.errnoName` of what `open` of a blob's file can fail with.
fn errno_name(errno: i32) -> &'static str {
    match errno {
        libc::ENOENT => "ENOENT",
        libc::EACCES => "EACCES",
        libc::EISDIR => "EISDIR",
        libc::EMFILE => "EMFILE",
        libc::ENFILE => "ENFILE",
        libc::ENOMEM => "ENOMEM",
        _ => "EIO",
    }
}

/// `strerror` as bionic words it.
fn strerror(errno: i32) -> &'static str {
    match errno {
        libc::ENOENT => "No such file or directory",
        libc::EACCES => "Permission denied",
        libc::EISDIR => "Is a directory",
        libc::EMFILE => "Too many open files",
        libc::ENFILE => "Too many open files in system",
        libc::ENOMEM => "Out of memory",
        _ => "I/O error",
    }
}

/// `UserHandle.getUserId`.
fn user_of(uid: i32) -> i32 {
    uid / PER_USER_RANGE as i32
}

/// The length of a Java string: its UTF-16 units.
fn java_length(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `String.hashCode`.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// `TextUtils.trimToLengthWithEllipsis`.
fn trim_with_ellipsis(text: &str, size: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= size {
        return text.to_owned();
    }
    let mut size = size;
    if (0xd800..0xdc00).contains(&units[size - 1]) && (0xdc00..0xe000).contains(&units[size]) {
        size -= 1;
    }
    String::from_utf16_lossy(&units[..size]) + "..."
}

/// `ResourceId.isValid`.
fn valid_resource_id(id: i32) -> bool {
    id != -1 && id as u32 & 0xff00_0000 != 0 && id as u32 & 0x00ff_0000 != 0
}

/// `Base64.encodeToString(bytes, Base64.DEFAULT)`: lines of 76 characters,
/// each ended by a newline.
fn base64_default(bytes: &[u8]) -> String {
    let encoded = aim_android_xml::base64::encode(bytes);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 76 + 1);
    for line in encoded.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(line).expect("Base64 is ASCII"));
        out.push('\n');
    }
    out
}

/// `Base64.decode(value, Base64.DEFAULT)`, which skips line breaks.
fn base64_decode(value: &str) -> Option<Vec<u8>> {
    let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    aim_android_xml::base64::decode(&compact)
}

/// An element as `FastXmlSerializer` writes it, without indentation.
struct Tag {
    name: &'static str,
    attrs: Vec<(&'static str, String)>,
    children: Vec<Tag>,
}

impl Tag {
    fn new(name: &'static str) -> Self {
        Tag {
            name,
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    fn attr(mut self, name: &'static str, value: impl ToString) -> Self {
        self.attrs.push((name, value.to_string()));
        self
    }

    /// `XmlUtils.writeStringAttribute`: nothing for null.
    fn optional(self, name: &'static str, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.attr(name, v),
            None => self,
        }
    }

    fn child(mut self, child: Tag) -> Self {
        self.children.push(child);
        self
    }

    fn write(&self, out: &mut String) {
        out.push('<');
        out.push_str(self.name);
        for (name, value) in &self.attrs {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            escape(value, out);
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str(" />\n");
            return;
        }
        out.push_str(">\n");
        for child in &self.children {
            child.write(out);
        }
        out.push_str("</");
        out.push_str(self.name);
        out.push_str(">\n");
    }

    /// The document `startDocument(null, true)` begins.
    fn document(&self) -> String {
        let mut out = String::from("<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n");
        self.write(&mut out);
        out
    }
}

/// `FastXmlSerializer`'s escapes: control characters as references, and
/// `"`, `&`, `<` and `>`.
fn escape(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if (c as u32) < 32 => out.push_str(&format!("&#{};", c as u32)),
            c => out.push(c),
        }
    }
}

/// `XmlUtils.readIntAttribute` and `readLongAttribute`: the attribute,
/// which must be a number.
fn number<T: std::str::FromStr>(e: &Element, name: &str) -> std::result::Result<T, String> {
    let value = e.string(name);
    value
        .as_deref()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("problem parsing {name}={value:?} as a number"))
}

/// `android.app.blob.BlobHandle`.
#[derive(Clone, Debug, PartialEq)]
struct BlobHandle {
    algorithm: Option<String>,
    digest: Option<Vec<u8>>,
    label: Option<String>,
    expiry_time_millis: i64,
    tag: Option<String>,
}

impl ReadParcelable for BlobHandle {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(BlobHandle {
            algorithm: r.read_string16()?,
            digest: aim_service_aidl::read_byte_array(r)?,
            label: crate::clip::char_sequence(r)?,
            expiry_time_millis: r.read_i64()?,
            tag: r.read_string16()?,
        })
    }
}

impl WriteParcelable for BlobHandle {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.algorithm.as_deref());
        aim_service_aidl::write_byte_array(p, self.digest.as_deref());
        crate::clip::write_char_sequence(p, self.label.as_deref());
        p.write_i64(self.expiry_time_millis);
        p.write_string16(self.tag.as_deref());
    }
}

impl BlobHandle {
    /// `assertIsValid`.
    fn assert_valid(&self) -> Result<()> {
        let algorithm = self.algorithm.as_deref().ok_or_else(|| npe(""))?;
        if algorithm != ALGO_SHA_256 {
            return Err(Exception::illegal_argument(format!(
                "{algorithm}is not supported [{ALGO_SHA_256}]"
            )));
        }
        match &self.digest {
            None => return Err(npe("digest must not be null")),
            Some(d) if d.is_empty() => return Err(Exception::illegal_argument("digest is empty")),
            Some(_) => {}
        }
        let label = self.label.as_deref().unwrap_or_default();
        if label.is_empty() {
            return Err(Exception::illegal_argument("label must not be null"));
        }
        if java_length(label) > LIMIT_BLOB_LABEL_LENGTH {
            return Err(Exception::illegal_argument("label too long"));
        }
        if self.expiry_time_millis < 0 {
            return Err(Exception::illegal_argument(
                "expiryTimeMillis must not be negative",
            ));
        }
        let tag = self.tag.as_deref().unwrap_or_default();
        if tag.is_empty() {
            return Err(Exception::illegal_argument("tag must not be null"));
        }
        if java_length(tag) > LIMIT_BLOB_TAG_LENGTH {
            return Err(Exception::illegal_argument("tag too long"));
        }
        Ok(())
    }

    /// `isExpired`.
    fn expired(&self) -> bool {
        self.expiry_time_millis != 0 && self.expiry_time_millis < now_millis()
    }

    /// `safeDigest`.
    fn safe_digest(digest: &[u8]) -> String {
        let s = aim_android_xml::base64::encode(digest);
        format!("{}..{}", &s[..2], &s[s.len() - 2..])
    }

    /// `toString`, as the security exceptions print it.
    fn describe(&self) -> String {
        format!(
            "BlobHandle {{algo:{},digest:{},label:{},expiryMs:{},tag:{}}}",
            self.algorithm.as_deref().unwrap_or("null"),
            Self::safe_digest(self.digest.as_deref().unwrap_or_default()),
            self.label.as_deref().unwrap_or("null"),
            self.expiry_time_millis,
            self.tag.as_deref().unwrap_or("null"),
        )
    }

    /// `writeToXml`.
    fn to_xml(&self) -> Tag {
        Tag::new("bh")
            .optional("al", self.algorithm.as_deref())
            .optional("dg", self.digest.as_deref().map(base64_default).as_deref())
            .optional("lbl", self.label.as_deref())
            .attr("ex", self.expiry_time_millis)
            .optional("tg", self.tag.as_deref())
    }

    /// `createFromXml`, valid as `create` asserts it.
    fn from_xml(e: &Element) -> std::result::Result<Self, String> {
        let digest = e
            .string("dg")
            .filter(|v| !v.is_empty())
            .map(|v| base64_decode(&v).ok_or("bad digest"))
            .transpose()?;
        let handle = BlobHandle {
            algorithm: e.string("al").map(Into::into),
            digest,
            label: e.string("lbl").map(Into::into),
            expiry_time_millis: number(e, "ex")?,
            tag: e.string("tg").map(Into::into),
        };
        handle.assert_valid().map_err(|x| x.message)?;
        Ok(handle)
    }
}

/// `android.app.blob.LeaseInfo`.
struct LeaseInfo {
    package: String,
    expiry_time_millis: i64,
    description_res_id: i32,
    description: Option<String>,
}

impl WriteParcelable for LeaseInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.package));
        p.write_i64(self.expiry_time_millis);
        p.write_i32(self.description_res_id);
        crate::clip::write_char_sequence(p, self.description.as_deref());
    }
}

/// `android.app.blob.BlobInfo`.
struct BlobInfo {
    id: i64,
    expiry_time_millis: i64,
    label: Option<String>,
    size: i64,
    leases: Vec<LeaseInfo>,
}

impl WriteParcelable for BlobInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.id);
        p.write_i64(self.expiry_time_millis);
        crate::clip::write_char_sequence(p, self.label.as_deref());
        p.write_i64(self.size);
        // `writeList`: each a length-prefixed `VAL_PARCELABLE`.
        p.write_i32(self.leases.len() as i32);
        for lease in &self.leases {
            p.write_i32(crate::bundle::VAL_PARCELABLE);
            let length = p.position();
            p.write_i32(-1);
            let start = p.position();
            p.write_string16(Some("android.app.blob.LeaseInfo"));
            lease.write_to(p);
            p.set_i32_at(length, (p.position() - start) as i32);
        }
    }
}

/// A `RemoteCallback`: its binder.
struct RemoteCallback(Option<Binder>);

impl ReadParcelable for RemoteCallback {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        r.read_binder().map(RemoteCallback)
    }
}

/// A `CharSequence` argument.
struct Text(Option<String>);

impl ReadParcelable for Text {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        crate::clip::char_sequence(r).map(Text)
    }
}

/// A `ParcelFileDescriptor` of a file of this host, without a comm fd.
struct SentFd(aim_binder_driver::File);

impl WriteParcelable for SentFd {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
        p.write_file(self.0.clone());
    }
}

/// `BlobAccessMode`.
#[derive(Clone, Debug, PartialEq)]
struct AccessMode {
    kind: i32,
    /// Its `PackageIdentifier`s, a set.
    allowed: Vec<(String, Vec<u8>)>,
}

impl Default for AccessMode {
    fn default() -> Self {
        AccessMode {
            kind: ACCESS_TYPE_PRIVATE,
            allowed: Vec::new(),
        }
    }
}

impl AccessMode {
    /// `allowPackageAccess`.
    fn allow_package(&mut self, package: String, certificate: Vec<u8>) {
        self.kind |= ACCESS_TYPE_ALLOWLIST;
        if !self
            .allowed
            .iter()
            .any(|(p, c)| *p == package && *c == certificate)
        {
            self.allowed.push((package, certificate));
        }
    }

    /// `isPackageAccessAllowed`.
    fn package_allowed(&self, package: &str, certificate: &[u8]) -> bool {
        self.kind & ACCESS_TYPE_ALLOWLIST != 0
            && self
                .allowed
                .iter()
                .any(|(p, c)| p == package && c == certificate)
    }

    /// `writeToXml`.
    fn to_xml(&self) -> Tag {
        self.allowed
            .iter()
            .fold(Tag::new("am").attr("t", self.kind), |t, (p, c)| {
                t.child(Tag::new("wl").attr("p", p).attr("ct", base64_default(c)))
            })
    }

    /// `createFromXml`.
    fn from_xml(e: &Element) -> std::result::Result<Self, String> {
        let mut mode = AccessMode {
            kind: number(e, "t")?,
            allowed: Vec::new(),
        };
        for wl in e.children().filter(|c| c.name == "wl") {
            let package = wl.string("p").map(Into::into).unwrap_or_default();
            let certificate = wl
                .string("ct")
                .filter(|v| !v.is_empty())
                .map(|v| base64_decode(&v).ok_or("bad certificate"))
                .transpose()?
                .unwrap_or_default();
            mode.allow_package(package, certificate);
        }
        Ok(mode)
    }
}

/// `BlobMetadata.Committer`.
#[derive(Clone, Debug, PartialEq)]
struct Committer {
    package: String,
    uid: i32,
    access: AccessMode,
    commit_time_ms: i64,
}

impl Committer {
    fn to_xml(&self) -> Tag {
        Tag::new("c")
            .attr("p", &self.package)
            .attr("u", self.uid)
            .attr("cmt", self.commit_time_ms)
            .child(self.access.to_xml())
    }

    /// `createFromXml`: none without an access mode.
    fn from_xml(e: &Element, version: i32) -> std::result::Result<Option<Self>, String> {
        let package = e.string("p").map(Into::into).unwrap_or_default();
        let uid = number(e, "u")?;
        let commit_time_ms = if version >= XML_VERSION_ADD_COMMIT_TIME {
            number(e, "cmt")?
        } else {
            0
        };
        let Some(access) = e.children().find(|c| c.name == "am") else {
            eprintln!("app host: blob_store: blobAccessMode should be available");
            return Ok(None);
        };
        Ok(Some(Committer {
            package,
            uid,
            access: AccessMode::from_xml(access)?,
            commit_time_ms,
        }))
    }
}

/// `BlobMetadata.Leasee`.
#[derive(Clone, Debug, PartialEq)]
struct Leasee {
    package: String,
    uid: i32,
    description_res_entry_name: Option<String>,
    description: Option<String>,
    expiry_time_millis: i64,
}

impl Leasee {
    /// `isStillValid`.
    fn valid(&self) -> bool {
        self.expiry_time_millis == 0 || self.expiry_time_millis >= now_millis()
    }

    fn to_xml(&self) -> Tag {
        Tag::new("l")
            .attr("p", &self.package)
            .attr("u", self.uid)
            .optional("rn", self.description_res_entry_name.as_deref())
            .attr("ex", self.expiry_time_millis)
            .optional("d", self.description.as_deref())
    }

    fn from_xml(e: &Element, version: i32) -> std::result::Result<Self, String> {
        Ok(Leasee {
            package: e.string("p").map(Into::into).unwrap_or_default(),
            uid: number(e, "u")?,
            description_res_entry_name: if version >= XML_VERSION_ADD_DESC_RES_NAME {
                e.string("rn").map(Into::into)
            } else {
                None
            },
            expiry_time_millis: number(e, "ex")?,
            description: if version >= XML_VERSION_ADD_STRING_DESC {
                e.string("d").map(Into::into)
            } else {
                None
            },
        })
    }
}

/// `BlobMetadata`.
#[derive(Clone, Debug, PartialEq)]
struct Blob {
    id: i64,
    handle: BlobHandle,
    committers: Vec<Committer>,
    leasees: Vec<Leasee>,
}

impl Blob {
    /// `addOrReplaceCommitter`.
    fn add_or_replace_committer(&mut self, committer: Committer) {
        self.committers
            .retain(|c| !(c.uid == committer.uid && c.package == committer.package));
        self.committers.push(committer);
    }

    fn committer(&self, package: &str, uid: i32) -> Option<&Committer> {
        self.committers
            .iter()
            .find(|c| c.uid == uid && c.package == package)
    }

    /// `isALeasee`: a valid lease of the caller's.
    fn leasee(&self, package: &str, uid: i32) -> Option<&Leasee> {
        self.leasees
            .iter()
            .find(|l| l.uid == uid && l.package == package && l.valid())
    }

    fn remove_leasee(&mut self, package: &str, uid: i32) {
        self.leasees
            .retain(|l| !(l.uid == uid && l.package == package));
    }

    /// `hasValidLeases`.
    fn has_valid_leases(&self) -> bool {
        self.leasees.iter().any(Leasee::valid)
    }

    /// `hasACommitterOrLeaseeInUser`.
    fn in_user(&self, user: i32) -> bool {
        self.committers.iter().any(|c| user_of(c.uid) == user)
            || self.leasees.iter().any(|l| user_of(l.uid) == user)
    }

    /// `shouldBeDeleted`.
    fn should_be_deleted(&self, config: &Config, respect_lease_wait_time: bool) -> bool {
        if self.handle.expired() {
            return true;
        }
        let waited = || {
            self.committers
                .iter()
                .all(|c| c.commit_time_ms + config.lease_acquisition_wait_ms < now_millis())
        };
        (!respect_lease_wait_time || waited()) && !self.has_valid_leases()
    }

    fn to_xml(&self) -> Tag {
        let tag = Tag::new("b")
            .attr("id", self.id)
            .child(self.handle.to_xml());
        let tag = self.committers.iter().fold(tag, |t, c| t.child(c.to_xml()));
        self.leasees.iter().fold(tag, |t, l| t.child(l.to_xml()))
    }

    /// `createFromXml`: none without a handle.
    fn from_xml(e: &Element, version: i32) -> std::result::Result<Option<Self>, String> {
        let id = number(e, "id")?;
        if version < XML_VERSION_ALLOW_ACCESS_ACROSS_USERS {
            number::<i32>(e, "us")?;
        }
        let mut handle = None;
        let (mut committers, mut leasees) = (Vec::<Committer>::new(), Vec::<Leasee>::new());
        for c in e.children() {
            match c.name.as_str() {
                "bh" => handle = Some(BlobHandle::from_xml(c)?),
                // Sets: of equal accessors, the first.
                "c" => {
                    if let Some(committer) = Committer::from_xml(c, version)?
                        && !committers
                            .iter()
                            .any(|x| x.uid == committer.uid && x.package == committer.package)
                    {
                        committers.push(committer);
                    }
                }
                "l" => {
                    let leasee = Leasee::from_xml(c, version)?;
                    if !leasees
                        .iter()
                        .any(|x| x.uid == leasee.uid && x.package == leasee.package)
                    {
                        leasees.push(leasee);
                    }
                }
                _ => {}
            }
        }
        let Some(handle) = handle else {
            eprintln!("app host: blob_store: blobHandle should be available");
            return Ok(None);
        };
        Ok(Some(Blob {
            id,
            handle,
            committers,
            leasees,
        }))
    }
}

/// `BlobStoreSession`'s state.
struct SessionState {
    state: i32,
    access: AccessMode,
    commit_callback: Option<Strong>,
}

/// `BlobStoreSession`: a node of its own, the same each time it is opened.
struct Session {
    id: i64,
    handle: BlobHandle,
    owner_uid: i32,
    owner_package: String,
    creation_time_ms: i64,
    file: PathBuf,
    state: Mutex<SessionState>,
    store: Weak<BlobStore>,
    binder: Mutex<Option<Binder>>,
}

/// `stateToString`.
fn state_name(state: i32) -> &'static str {
    match state {
        STATE_OPENED => "<opened>",
        STATE_CLOSED => "<closed>",
        STATE_ABANDONED => "<abandoned>",
        STATE_COMMITTED => "<committed>",
        STATE_VERIFIED_VALID => "<verified_valid>",
        STATE_VERIFIED_INVALID => "<verified_invalid>",
        _ => "<unknown>",
    }
}

impl Session {
    fn describe(&self) -> String {
        format!(
            "BlobStoreSession {{id:{},handle:{},uid:{},pkg:{}}}",
            self.id,
            self.handle.describe(),
            self.owner_uid,
            self.owner_package
        )
    }

    /// `isFinalized`.
    fn finalized(state: i32) -> bool {
        state == STATE_COMMITTED || state == STATE_ABANDONED
    }

    /// `open`.
    fn open(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if Self::finalized(s.state) {
            return Err(illegal_state(format!(
                "Not allowed to open session with state: {}",
                state_name(s.state)
            )));
        }
        s.state = STATE_OPENED;
        Ok(())
    }

    /// `assertCallerIsOwner`.
    fn assert_owner(&self, uid: i32) -> Result<()> {
        if uid != self.owner_uid {
            return Err(Exception::security(format!(
                "{} is not the session owner",
                self.owner_uid
            )));
        }
        Ok(())
    }

    /// The state, which must be opened for `what`.
    fn opened(&self, what: &str) -> Result<MutexGuard<'_, SessionState>> {
        let s = self.state.lock().unwrap();
        if s.state != STATE_OPENED {
            return Err(illegal_state(format!(
                "Not allowed to {what} in state: {}",
                state_name(s.state)
            )));
        }
        Ok(s)
    }

    /// `getSize`: the session file's length.
    fn size(&self) -> i64 {
        std::fs::metadata(&self.file).map_or(0, |m| m.len() as i64)
    }

    /// `closeSession`: the new state, the store told of it when asked.
    fn close_session(self: &Arc<Self>, uid: i32, state: i32, send_callback: bool) -> Result<()> {
        self.assert_owner(uid)?;
        let mut s = self.state.lock().unwrap();
        if s.state != STATE_OPENED {
            if state == STATE_CLOSED {
                // Closing a session already deleted or abandoned.
                return Ok(());
            }
            return Err(illegal_state(format!(
                "Not allowed to delete or abandon a session with state: {}",
                state_name(s.state)
            )));
        }
        s.state = state;
        drop(s);
        if send_callback && let Some(store) = self.store.upgrade() {
            let session = self.clone();
            store.post(move |store| store.on_state_changed(&session));
        }
        Ok(())
    }

    /// `sendCommitCallbackResult`.
    fn send_commit_result(&self, result: i32) {
        let Some(cb) = self.state.lock().unwrap().commit_callback.take() else {
            eprintln!(
                "app host: blob_store: no commit callback for {}",
                self.describe()
            );
            return;
        };
        let mut p = Parcel::new();
        callback::OnResult { result }.write(&mut p);
        if let Err(e) = cb.transact(callback::ON_RESULT, &p, true) {
            eprintln!("app host: blob_store: Error sending the callback result: {e}");
        }
    }

    /// `computeDigest` and `verifyBlobData`: valid when the data's digest
    /// is the handle's, else invalid with the error sent.
    fn verify(&self) {
        let digest = (|| {
            let mut file = std::fs::File::open(&self.file)?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0; 8192];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            Ok::<_, std::io::Error>(hasher.finalize().to_vec())
        })();
        let digest = match digest {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("app host: blob_store: Error computing the digest: {e}");
                None
            }
        };
        let expected = self.handle.digest.as_deref().unwrap_or_default();
        if digest.as_deref() == Some(expected) {
            self.state.lock().unwrap().state = STATE_VERIFIED_VALID;
            return;
        }
        eprintln!(
            "app host: blob_store: Digest of the data ({}) didn't match the given BlobHandle.digest ({})",
            digest
                .as_deref()
                .map_or("null".into(), BlobHandle::safe_digest),
            BlobHandle::safe_digest(expected)
        );
        self.state.lock().unwrap().state = STATE_VERIFIED_INVALID;
        self.send_commit_result(COMMIT_RESULT_ERROR);
    }

    fn to_xml(&self) -> Tag {
        Tag::new("s")
            .attr("id", self.id)
            .attr("p", &self.owner_package)
            .attr("u", self.owner_uid)
            .attr("crt", self.creation_time_ms)
            .child(self.handle.to_xml())
            .child(self.state.lock().unwrap().access.to_xml())
    }
}

/// What a session's node serves.
struct SessionBinder(Arc<Session>);

impl SessionBinder {
    fn call(&self, call: &mut Call<'_>, p: &mut Parcel) -> CallResult {
        let session = &self.0;
        let uid = call.sender_euid as i32;
        match call.code {
            session::OPEN_WRITE => {
                let a = session::OpenWrite::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    if a.offset_bytes < 0 {
                        return Err(Exception::illegal_argument(
                            "offsetBytes must not be negative",
                        ));
                    }
                    session.assert_owner(uid)?;
                    drop(session.opened("write")?);
                    Err(self.unimplemented("openWrite: a revocable file descriptor"))
                })())
            }
            session::OPEN_READ => {
                session::OpenRead::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    drop(session.opened("read")?);
                    let store = self.store()?;
                    if store.config()?.use_revocable_fd_for_reads {
                        return Err(self.unimplemented("openRead: a revocable file descriptor"));
                    }
                    let fd = open_read(&session.file)?;
                    session::write_open_read_reply(p, Some(&fd));
                    Ok(())
                })())
            }
            session::GET_SIZE => {
                session::GetSize::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                session::write_get_size_reply(p, session.size());
                Ok(Ok(()))
            }
            session::ALLOW_PACKAGE_ACCESS => {
                let a = session::AllowPackageAccess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    if java_length(&package) > MAX_PACKAGE_NAME_LENGTH {
                        return Err(Exception::illegal_argument(format!(
                            "packageName is longer than {MAX_PACKAGE_NAME_LENGTH} chars"
                        )));
                    }
                    let certificate = a
                        .certificate
                        .ok_or_else(|| npe("certificate must not be null"))?;
                    if certificate.len() > MAX_CERTIFICATE_LENGTH {
                        return Err(Exception::illegal_argument(format!(
                            "certificate is longer than {MAX_CERTIFICATE_LENGTH} chars"
                        )));
                    }
                    let max = self.store()?.config()?.max_permitted_packages;
                    let mut s = session.opened("change access type")?;
                    if s.access.allowed.len() >= max.max(0) as usize {
                        return Err(limit_exceeded(&format!(
                            "Too many packages permitted to access the blob: {}",
                            s.access.allowed.len()
                        )));
                    }
                    s.access.allow_package(package, certificate);
                    session::write_allow_package_access_reply(p);
                    Ok(())
                })())
            }
            session::ALLOW_SAME_SIGNATURE_ACCESS => {
                session::AllowSameSignatureAccess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    session.opened("change access type")?.access.kind |= ACCESS_TYPE_SAME_SIGNATURE;
                    session::write_allow_same_signature_access_reply(p);
                    Ok(())
                })())
            }
            session::ALLOW_PUBLIC_ACCESS => {
                session::AllowPublicAccess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    session.opened("change access type")?.access.kind |= ACCESS_TYPE_PUBLIC;
                    session::write_allow_public_access_reply(p);
                    Ok(())
                })())
            }
            session::IS_PACKAGE_ACCESS_ALLOWED => {
                let a =
                    session::IsPackageAccessAllowed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    let certificate = match a.certificate {
                        None => return Err(npe("certificate must not be null")),
                        Some(c) if c.is_empty() => {
                            return Err(Exception::illegal_argument("certificate is empty"));
                        }
                        Some(c) => c,
                    };
                    let allowed = session
                        .opened("get access type")?
                        .access
                        .package_allowed(&package, &certificate);
                    session::write_is_package_access_allowed_reply(p, allowed);
                    Ok(())
                })())
            }
            session::IS_SAME_SIGNATURE_ACCESS_ALLOWED => {
                session::IsSameSignatureAccessAllowed::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    let kind = session.opened("get access type")?.access.kind;
                    session::write_is_same_signature_access_allowed_reply(
                        p,
                        kind & ACCESS_TYPE_SAME_SIGNATURE != 0,
                    );
                    Ok(())
                })())
            }
            session::IS_PUBLIC_ACCESS_ALLOWED => {
                session::IsPublicAccessAllowed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.assert_owner(uid)?;
                    let kind = session.opened("get access type")?.access.kind;
                    session::write_is_public_access_allowed_reply(
                        p,
                        kind & ACCESS_TYPE_PUBLIC != 0,
                    );
                    Ok(())
                })())
            }
            session::CLOSE => {
                session::Close::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.close_session(uid, STATE_CLOSED, false)?;
                    session::write_close_reply(p);
                    Ok(())
                })())
            }
            session::ABANDON => {
                session::Abandon::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    session.close_session(uid, STATE_ABANDONED, true)?;
                    session::write_abandon_reply(p);
                    Ok(())
                })())
            }
            session::COMMIT => {
                let a = session::Commit::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let store = self.store()?;
                    session.state.lock().unwrap().commit_callback = match a.callback {
                        Some(Binder::Handle(h)) => Some(store.host.process.strong(h)),
                        _ => None,
                    };
                    session.close_session(uid, STATE_COMMITTED, true)?;
                    session::write_commit_reply(p);
                    Ok(())
                })())
            }
            code => {
                let name = session::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Ok(Err(self.unimplemented(name)))
            }
        }
    }

    fn store(&self) -> Result<Arc<BlobStore>> {
        self.0
            .store
            .upgrade()
            .ok_or_else(|| illegal_state("blob_store: the service is gone".into()))
    }

    fn unimplemented(&self, name: &str) -> Exception {
        match self.0.store.upgrade() {
            Some(store) => store
                .host
                .unimplemented_named("blob_store", &format!("{name} (#1938)")),
            None => illegal_state(format!("blob_store: {name} (#1938)")),
        }
    }
}

impl Service for SessionBinder {
    fn descriptor(&self) -> &str {
        session::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = self.call(call, &mut p)?;
        reply(result.map(|()| p))
    }
}

/// `openReadInternal` and `openForRead` without a revocable descriptor:
/// the file, read-only.
fn open_read(path: &Path) -> Result<SentFd> {
    let file = std::fs::File::open(path).map_err(|e| io_exception(&e))?;
    let sent = aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file))
        .ok_or_else(|| illegal_state("blob_store: the file's descriptor".into()))?;
    Ok(SentFd(sent))
}

/// `BlobStoreConfig.DeviceConfigProperties`, as DeviceConfig `blobstore`
/// sets them now.
struct Config {
    total_bytes_per_app_limit_floor: i64,
    total_bytes_per_app_limit_fraction: f32,
    lease_acquisition_wait_ms: i64,
    commit_cool_off_ms: i64,
    use_revocable_fd_for_reads: bool,
    delete_on_last_lease_delay_ms: i64,
    max_active_sessions: i32,
    max_committed_blobs: i32,
    max_leased_blobs: i32,
    max_permitted_packages: i32,
    lease_desc_char_limit: i32,
}

impl Config {
    fn read(value: impl Fn(&str) -> Option<String>) -> Self {
        // `Properties.getLong` and the rest: the default for a value that
        // does not parse.
        let long = |(k, d): (&str, i64)| value(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let int = |(k, d): (&str, i32)| value(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let float = |(k, d): (&str, f32)| {
            value(k)
                .and_then(|v| aim_android_xml::double::parse_float(&v))
                .unwrap_or(d)
        };
        // `Boolean.parseBoolean`.
        let boolean = |(k, d): (&str, bool)| value(k).map_or(d, |v| v.eq_ignore_ascii_case("true"));
        Config {
            total_bytes_per_app_limit_floor: long(TOTAL_BYTES_PER_APP_LIMIT_FLOOR),
            total_bytes_per_app_limit_fraction: float(TOTAL_BYTES_PER_APP_LIMIT_FRACTION),
            lease_acquisition_wait_ms: long(LEASE_ACQUISITION_WAIT_DURATION_MS),
            commit_cool_off_ms: long(COMMIT_COOL_OFF_DURATION_MS),
            use_revocable_fd_for_reads: boolean(USE_REVOCABLE_FD_FOR_READS),
            delete_on_last_lease_delay_ms: long(DELETE_ON_LAST_LEASE_DELAY_MS),
            max_active_sessions: int(MAX_ACTIVE_SESSIONS),
            max_committed_blobs: int(MAX_COMMITTED_BLOBS),
            max_leased_blobs: int(MAX_LEASED_BLOBS),
            max_permitted_packages: int(MAX_BLOB_ACCESS_PERMITTED_PACKAGES),
            lease_desc_char_limit: int(LEASE_DESC_CHAR_LIMIT),
        }
    }

    /// `getAdjustedCommitTimeMs`.
    fn adjusted_commit_time(&self, old: i64, new: i64) -> i64 {
        if old == 0 || old + self.commit_cool_off_ms < now_millis() {
            new
        } else {
            old
        }
    }

    /// `getAppDataBytesLimit` of a data volume of `total` bytes (the
    /// product a float, as Java's `long * float` is).
    fn app_data_bytes_limit(&self, total: i64) -> i64 {
        let fraction = (total as f32 * self.total_bytes_per_app_limit_fraction) as i64;
        self.total_bytes_per_app_limit_floor.max(fraction)
    }
}

/// The sessions and blobs, as the original's `mBlobsLock` guards them.
#[derive(Default)]
struct Store {
    /// By user, then id.
    sessions: BTreeMap<i32, BTreeMap<i64, Arc<Session>>>,
    blobs: Vec<Blob>,
    active_ids: HashSet<i64>,
    known_ids: HashSet<i64>,
}

impl Store {
    fn add_session(&mut self, session: Arc<Session>) {
        let id = session.id;
        self.sessions
            .entry(user_of(session.owner_uid))
            .or_default()
            .insert(id, session);
        self.active_ids.insert(id);
        self.known_ids.insert(id);
    }

    fn add_blob(&mut self, blob: Blob) {
        self.active_ids.insert(blob.id);
        self.known_ids.insert(blob.id);
        self.blobs.push(blob);
    }

    fn blob(&self, handle: &BlobHandle) -> Option<&Blob> {
        self.blobs.iter().find(|b| b.handle == *handle)
    }

    fn blob_mut(&mut self, handle: &BlobHandle) -> Option<&mut Blob> {
        self.blobs.iter_mut().find(|b| b.handle == *handle)
    }

    fn user_sessions(&self, user: i32) -> impl Iterator<Item = &Arc<Session>> {
        self.sessions
            .get(&user)
            .into_iter()
            .flat_map(|s| s.values())
    }

    fn remove_session(&mut self, session: &Session) {
        if let Some(sessions) = self.sessions.get_mut(&user_of(session.owner_uid)) {
            sessions.remove(&session.id);
        }
    }
}

/// A session as the index keeps it.
#[derive(Debug, PartialEq)]
struct Persisted {
    id: i64,
    package: String,
    uid: i32,
    creation_time_ms: i64,
    handle: BlobHandle,
    access: AccessMode,
}

/// The root of an index, which must be `tag`, and its version.
fn index_root(bytes: &[u8], tag: &str) -> std::result::Result<(Element, i32), String> {
    let root = aim_android_xml::read(bytes)?;
    if root.name != tag {
        return Err(format!(
            "Unexpected start tag: found {}, expected {tag}",
            root.name
        ));
    }
    let version = number(&root, "v")?;
    Ok((root, version))
}

/// `BlobStoreSession.createFromXml` of each session of the index, those
/// before an error that ends the reading, and the error.
fn parse_sessions(bytes: &[u8]) -> (Vec<Persisted>, Option<String>) {
    let mut sessions = Vec::new();
    let result = (|| {
        let (root, version) = index_root(bytes, "ss")?;
        for e in root.children().filter(|e| e.name == "s") {
            let id = number(e, "id")?;
            let package = e.string("p").map(Into::into).unwrap_or_default();
            let uid = number(e, "u")?;
            let creation_time_ms = if version >= XML_VERSION_ADD_SESSION_CREATION_TIME {
                number(e, "crt")?
            } else {
                now_millis()
            };
            let (mut handle, mut access) = (None, None);
            for c in e.children() {
                match c.name.as_str() {
                    "bh" => handle = Some(BlobHandle::from_xml(c)?),
                    "am" => access = Some(AccessMode::from_xml(c)?),
                    _ => {}
                }
            }
            let Some(handle) = handle else {
                eprintln!("app host: blob_store: blobHandle should be available");
                continue;
            };
            let Some(persisted_access) = access else {
                eprintln!("app host: blob_store: blobAccessMode should be available");
                continue;
            };
            // `mBlobAccessMode.allow(blobAccessMode)` of a private mode.
            let mut access = AccessMode::default();
            access.kind |= persisted_access.kind;
            if persisted_access.kind & ACCESS_TYPE_ALLOWLIST != 0 {
                for (p, c) in persisted_access.allowed {
                    access.allow_package(p, c);
                }
            }
            sessions.push(Persisted {
                id,
                package,
                uid,
                creation_time_ms,
                handle,
                access,
            });
        }
        Ok(())
    })();
    (sessions, result.err())
}

/// `BlobMetadata.createFromXml` of each blob of the index, with its
/// version; those before an error that ends the reading, and the error.
fn parse_blobs(bytes: &[u8]) -> (i32, Vec<Blob>, Option<String>) {
    let mut blobs = Vec::new();
    let mut version = 0;
    let result = (|| {
        let (root, v) = index_root(bytes, "bs")?;
        version = v;
        for e in root.children().filter(|e| e.name == "b") {
            if let Some(blob) = Blob::from_xml(e, version)? {
                blobs.push(blob);
            }
        }
        Ok::<(), String>(())
    })();
    (version, blobs, result.err())
}

/// A unit of the service's handler.
type Job = Box<dyn FnOnce(&Arc<BlobStore>) + Send>;

pub struct BlobStore {
    host: Arc<AppHost>,
    store: Mutex<Option<Store>>,
    handler: Mutex<Sender<Job>>,
    me: Weak<BlobStore>,
}

impl BlobStore {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let this = Arc::new_cyclic(|me| BlobStore {
            host,
            store: Mutex::new(None),
            handler: Mutex::new(tx),
            me: me.clone(),
        });
        let me = Arc::downgrade(&this);
        std::thread::Builder::new()
            .name("BlobStore".into())
            .spawn(move || {
                for job in rx {
                    let Some(store) = me.upgrade() else { return };
                    job(&store);
                }
            })
            .expect("the blob store's thread");
        // `registerReceiverAsUser(new PackageChangedReceiver(), UserHandle.ALL, ...)`.
        let receiver = this.clone() as Arc<dyn PackageMonitor>;
        this.host
            .package_monitors
            .register(USER_ALL, Arc::downgrade(&receiver));
        this
    }

    /// `mHandler.post`.
    fn post(&self, job: impl FnOnce(&Arc<BlobStore>) + Send + 'static) {
        let _ = self.handler.lock().unwrap().send(Box::new(job));
    }

    /// `mHandler.postDelayed`.
    fn post_delayed(&self, delay: Duration, job: impl FnOnce(&Arc<BlobStore>) + Send + 'static) {
        let me = self.me.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            if let Some(store) = me.upgrade() {
                store.post(job);
            }
        });
    }

    fn config(&self) -> Result<Config> {
        let inputs = self.host.inputs()?;
        Ok(Config::read(|key| {
            inputs
                .settings
                .get(Table::Config, &format!("blobstore/{key}"))
        }))
    }

    /// `BlobStoreConfig.getBlobStoreRootDir`.
    fn root_dir(&self) -> Result<PathBuf> {
        Ok(self.host.data()?.join("system").join(ROOT_DIR_NAME))
    }

    /// `prepareBlobStoreRootDir`, `prepareBlobsDir`: made as `mkdir` makes
    /// them.
    fn prepare_dir(path: &Path) -> std::io::Result<()> {
        match std::fs::create_dir(path) {
            Ok(()) => guest_inode::record(path, SYSTEM_DIR),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// `BlobStoreConfig.getBlobFile`.
    fn blob_file(&self, id: i64) -> Result<PathBuf> {
        Ok(self.root_dir()?.join(BLOBS_DIR_NAME).join(id.to_string()))
    }

    /// The store, read at its first use as the original reads it at
    /// `PHASE_THIRD_PARTY_APPS_CAN_START`.
    fn lock(&self) -> Result<MutexGuard<'_, Option<Store>>> {
        let mut guard = self.store.lock().unwrap();
        if guard.is_none() {
            *guard = Some(self.read()?);
        }
        Ok(guard)
    }

    fn with<T>(&self, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut guard = self.lock()?;
        f(guard.as_mut().expect("read"))
    }

    /// `getAllPackages` of the app host's one user: each app uid's package
    /// as `getInstalledApplications` lists them, the last of a shared uid's
    /// in the settings' order (by name hash) winning.
    fn known_packages(&self) -> Result<BTreeMap<i32, String>> {
        let (state, _) = self.host.packages()?;
        let mut packages: Vec<_> = state.packages.values().collect();
        packages.sort_by_key(|ps| java_hash(&ps.name));
        Ok(packages
            .into_iter()
            .map(|ps| (ps.app_id, ps.name.clone()))
            .collect())
    }

    /// `readBlobSessionsLocked` and `readBlobsInfoLocked`.
    fn read(&self) -> Result<Store> {
        let mut store = Store::default();
        let root = self.root_dir()?;
        if !root.exists() {
            return Ok(store);
        }
        let known = self.known_packages()?;
        let known = |uid: i32, package: &str| {
            user_of(uid) == 0
                && known
                    .get(&(uid % PER_USER_RANGE as i32))
                    .is_some_and(|p| p == package)
        };
        let sessions = root.join(SESSIONS_INDEX_FILE_NAME);
        match super::device_policy::read_atomic_bytes(&sessions) {
            Ok(None) => eprintln!(
                "app host: blob_store: Sessions index file not available: {}",
                sessions.display()
            ),
            Ok(Some(bytes)) => {
                if let Err(e) = self.read_sessions(&bytes, &mut store, &known) {
                    eprintln!("app host: blob_store: Error reading sessions data: {e}");
                }
            }
            Err(e) => eprintln!("app host: blob_store: Error reading sessions data: {e}"),
        }
        let blobs = root.join(BLOBS_INDEX_FILE_NAME);
        match super::device_policy::read_atomic_bytes(&blobs) {
            Ok(None) => eprintln!(
                "app host: blob_store: Blobs index file not available: {}",
                blobs.display()
            ),
            Ok(Some(bytes)) => {
                if let Err(e) = self.read_blobs(&bytes, &mut store, &known) {
                    eprintln!("app host: blob_store: Error reading blobs data: {e}");
                }
            }
            Err(e) => eprintln!("app host: blob_store: Error reading blobs data: {e}"),
        }
        Ok(store)
    }

    /// The sessions of the index, of known packages; another's file goes.
    fn read_sessions(
        &self,
        bytes: &[u8],
        store: &mut Store,
        known: &dyn Fn(i32, &str) -> bool,
    ) -> std::result::Result<(), String> {
        let (sessions, error) = parse_sessions(bytes);
        for persisted in sessions {
            let file = self.blob_file(persisted.id).map_err(|e| e.message)?;
            if !known(persisted.uid, &persisted.package) {
                // An unknown package, or the session is not this package's.
                let _ = std::fs::remove_file(&file);
                continue;
            }
            store.add_session(self.new_session(persisted, file));
        }
        error.map_or(Ok(()), Err)
    }

    /// The blobs of the index, without unknown packages' committers and
    /// leasees; a user's copy of an index older than access across users
    /// merged into the first, its data gone.
    fn read_blobs(
        &self,
        bytes: &[u8],
        store: &mut Store,
        known: &dyn Fn(i32, &str) -> bool,
    ) -> std::result::Result<(), String> {
        let (version, blobs, error) = parse_blobs(bytes);
        for mut blob in blobs {
            blob.committers.retain(|c| known(c.uid, &c.package));
            blob.leasees.retain(|l| known(l.uid, &l.package));
            if version >= XML_VERSION_ALLOW_ACCESS_ACROSS_USERS {
                store.add_blob(blob);
            } else if let Some(existing) = store.blob_mut(&blob.handle) {
                // `addCommittersAndLeasees`.
                for c in blob.committers {
                    if existing.committer(&c.package, c.uid).is_none() {
                        existing.committers.push(c);
                    }
                }
                for l in blob.leasees {
                    if !existing
                        .leasees
                        .iter()
                        .any(|x| x.uid == l.uid && x.package == l.package)
                    {
                        existing.leasees.push(l);
                    }
                }
                let _ = std::fs::remove_file(self.blob_file(blob.id).map_err(|e| e.message)?);
            } else {
                store.add_blob(blob);
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// A session served as a node of its own.
    fn new_session(&self, p: Persisted, file: PathBuf) -> Arc<Session> {
        let session = Arc::new(Session {
            id: p.id,
            handle: p.handle,
            owner_uid: p.uid,
            owner_package: p.package,
            creation_time_ms: p.creation_time_ms,
            file,
            state: Mutex::new(SessionState {
                state: STATE_CLOSED,
                access: p.access,
                commit_callback: None,
            }),
            store: self.me.clone(),
            binder: Mutex::new(None),
        });
        let binder = self
            .host
            .process
            .add_service(Arc::new(SessionBinder(session.clone())));
        *session.binder.lock().unwrap() = Some(binder);
        session
    }

    /// `AtomicFile.startWrite` and `finishWrite` of an index under the
    /// root, made if it is not there.
    fn write_index(&self, name: &str, document: &str) -> std::io::Result<()> {
        let root = self
            .root_dir()
            .map_err(|e| std::io::Error::other(e.message))?;
        Self::prepare_dir(&root)?;
        let path = root.join(name);
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        let backup = PathBuf::from(backup);
        if backup.exists() {
            std::fs::rename(&backup, &path)?;
        }
        let mut new = path.as_os_str().to_owned();
        new.push(".new");
        let new = PathBuf::from(new);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&new)?;
        std::io::Write::write_all(&mut file, document.as_bytes())?;
        crate::fsync(&file)?;
        guest_inode::record(&new, SYSTEM_FILE)?;
        std::fs::rename(&new, path)
    }

    /// `writeBlobSessionsLocked`.
    fn write_sessions(&self, store: &Store) {
        let root = store
            .sessions
            .values()
            .flat_map(|s| s.values())
            .fold(Tag::new("ss").attr("v", XML_VERSION_CURRENT), |t, s| {
                t.child(s.to_xml())
            });
        if let Err(e) = self.write_index(SESSIONS_INDEX_FILE_NAME, &root.document()) {
            eprintln!("app host: blob_store: Error writing sessions data: {e}");
        }
    }

    /// `writeBlobsInfoLocked`.
    fn write_blobs(&self, store: &Store) -> std::io::Result<()> {
        let root = store
            .blobs
            .iter()
            .fold(Tag::new("bs").attr("v", XML_VERSION_CURRENT), |t, b| {
                t.child(b.to_xml())
            });
        self.write_index(BLOBS_INDEX_FILE_NAME, &root.document())
            .inspect_err(|e| eprintln!("app host: blob_store: Error writing blobs data: {e}"))
    }

    /// `deleteSessionLocked`: the session's file goes.
    fn delete_session(store: &mut Store, session: &Session) {
        let _ = std::fs::remove_file(&session.file);
        store.active_ids.remove(&session.id);
    }

    /// `deleteBlobLocked`: the blob's file goes.
    fn delete_blob(&self, store: &mut Store, id: i64) {
        if let Ok(file) = self.blob_file(id) {
            let _ = std::fs::remove_file(file);
        }
        store.active_ids.remove(&id);
    }

    /// `onStateChangedInternal`, on the handler.
    fn on_state_changed(self: &Arc<Self>, session: &Arc<Session>) {
        let state = session.state.lock().unwrap().state;
        match state {
            STATE_ABANDONED | STATE_VERIFIED_INVALID => {
                if let Err(e) = self.with(|store| {
                    Self::delete_session(store, session);
                    store.remove_session(session);
                    Ok(())
                }) {
                    eprintln!(
                        "app host: blob_store: deleting {}: {}",
                        session.describe(),
                        e.message
                    );
                }
            }
            STATE_COMMITTED => {
                // `computeDigest` on the background thread, then
                // `verifyBlobData` on the handler.
                session.verify();
                let session = session.clone();
                self.post(move |store| store.on_state_changed(&session));
            }
            STATE_VERIFIED_VALID => {
                let result = self.commit(session);
                if let Err(e) = result {
                    eprintln!(
                        "app host: blob_store: committing {}: {}",
                        session.describe(),
                        e.message
                    );
                }
            }
            _ => eprintln!(
                "app host: blob_store: Invalid session state: {}",
                state_name(state)
            ),
        }
        if let Err(e) = self.with(|store| {
            self.write_sessions(store);
            Ok(())
        }) {
            eprintln!("app host: blob_store: {}", e.message);
        }
    }

    /// `onStateChangedInternal`'s `STATE_VERIFIED_VALID`: the session's
    /// data committed as its handle's blob, or the error sent.
    fn commit(&self, session: &Arc<Session>) -> Result<()> {
        let config = self.config()?;
        self.with(|store| {
            let committed = store
                .blobs
                .iter()
                .filter(|b| b.committer(&session.owner_package, session.owner_uid).is_some())
                .count();
            if committed >= config.max_committed_blobs.max(0) as usize {
                eprintln!(
                    "app host: blob_store: Failed to commit: too many committed blobs. count: {committed}; blob: {}",
                    session.describe()
                );
                session.send_commit_result(COMMIT_RESULT_ERROR);
                Self::delete_session(store, session);
                store.remove_session(session);
                return Ok(());
            }
            if store.blob(&session.handle).is_none() {
                store.add_blob(Blob {
                    id: session.id,
                    handle: session.handle.clone(),
                    committers: Vec::new(),
                    leasees: Vec::new(),
                });
            }
            let blob = store.blob_mut(&session.handle).expect("added");
            let existing = blob
                .committer(&session.owner_package, session.owner_uid)
                .cloned();
            let existing_time = existing.as_ref().map_or(0, |c| c.commit_time_ms);
            let committer = Committer {
                package: session.owner_package.clone(),
                uid: session.owner_uid,
                access: session.state.lock().unwrap().access.clone(),
                commit_time_ms: config.adjusted_commit_time(existing_time, now_millis()),
            };
            blob.add_or_replace_committer(committer);
            let blob_id = blob.id;
            if self.write_blobs(store).is_ok() {
                session.send_commit_result(COMMIT_RESULT_SUCCESS);
            } else {
                let blob = store.blob_mut(&session.handle).expect("added");
                match existing {
                    None => blob
                        .committers
                        .retain(|c| !(c.uid == session.owner_uid && c.package == session.owner_package)),
                    Some(c) => blob.add_or_replace_committer(c),
                }
                session.send_commit_result(COMMIT_RESULT_ERROR);
                // A new blob goes when its commit fails; a recommit's stays.
                if session.id == blob_id {
                    self.delete_blob(store, blob_id);
                    store.blobs.retain(|b| b.handle != session.handle);
                }
            }
            // A recommit's data is redundant.
            if session.id != blob_id {
                Self::delete_session(store, session);
            }
            store.remove_session(session);
            Ok(())
        })
    }

    // The calls' checks.

    /// `verifyCallingPackage`.
    fn verify_calling_package(&self, uid: i32, package: Option<&str>) -> Result<()> {
        let found = match package {
            Some(package) => {
                let (state, resolution) = self.host.packages()?;
                let query = Query {
                    state: &state,
                    filter: &resolution.apps_filter,
                    calling_uid: SYSTEM_UID,
                };
                query
                    .package_uid_internal(package, 0, user_of(uid), SYSTEM_UID)
                    .map_err(|e| self.host.unimplemented_named("blob_store", e.0))?
            }
            None => -1,
        };
        if found != uid {
            return Err(Exception::security(format!(
                "Specified calling package [{}] does not match the calling uid {uid}",
                package.unwrap_or("null")
            )));
        }
        Ok(())
    }

    /// `isAllowedBlobStoreAccess`: not an SDK sandbox, an isolated process
    /// or an instant app.
    fn allowed_access(&self, uid: i32, package: &str) -> Result<bool> {
        if SDK_SANDBOX_UIDS.contains(&(uid % PER_USER_RANGE as i32)) || is_isolated(uid) {
            return Ok(false);
        }
        let (state, _) = self.host.packages()?;
        let instant = state
            .packages
            .get(package)
            .and_then(|ps| ps.users.get(&user_of(uid)))
            .is_some_and(|u| u.instant_app);
        Ok(!instant)
    }

    fn enforce_allowed(&self, uid: i32, package: &str, what: &str) -> Result<()> {
        if !self.allowed_access(uid, package)? {
            return Err(Exception::security(format!(
                "Caller not allowed to {what}; callingUid={uid}, callingPackage={package}"
            )));
        }
        Ok(())
    }

    /// The handle of a call, valid; and its package (`requireNonNull`).
    fn handle_and_package(
        handle: Option<BlobHandle>,
        package: Option<String>,
    ) -> Result<(BlobHandle, String)> {
        let handle = handle.ok_or_else(|| npe("blobHandle must not be null"))?;
        handle.assert_valid()?;
        let package = package.ok_or_else(|| npe("packageName must not be null"))?;
        Ok((handle, package))
    }

    /// `BlobMetadata.isAccessAllowedForCaller`. Another user's committers
    /// cannot be among the app host's one user's blobs, so access across
    /// users (ACCESS_BLOBS_ACROSS_USERS) never comes into it.
    fn access_allowed(&self, blob: &Blob, package: &str, uid: i32) -> Result<bool> {
        if blob.handle.expired() {
            return Ok(false);
        }
        if blob.leasee(package, uid).is_some() {
            return Ok(true);
        }
        for committer in blob
            .committers
            .iter()
            .filter(|c| user_of(c.uid) == user_of(uid))
        {
            if committer.uid == uid && committer.package == package {
                return Ok(true);
            }
            if self.mode_allows(&committer.access, package, uid, committer.uid)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `BlobAccessMode.isAccessAllowedForCaller`.
    fn mode_allows(
        &self,
        mode: &AccessMode,
        package: &str,
        uid: i32,
        committer_uid: i32,
    ) -> Result<bool> {
        if mode.kind & ACCESS_TYPE_PUBLIC != 0 {
            return Ok(true);
        }
        let (state, resolution) = self.host.packages()?;
        let not_modelled = |e: crate::package::apps_filter::NotModelled| {
            self.host.unimplemented_named("blob_store", e.0)
        };
        if mode.kind & ACCESS_TYPE_SAME_SIGNATURE != 0 {
            let system = Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: SYSTEM_UID,
            };
            // `checkUidSignaturesForAllUsers`, as the system.
            if system
                .check_uid_signatures_captured(uid, committer_uid, true)
                .map_err(not_modelled)??
                == SIGNATURE_MATCH
            {
                return Ok(true);
            }
        }
        if mode.kind & ACCESS_TYPE_ALLOWLIST != 0 {
            // `hasSigningCertificate` as the caller.
            let caller = Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: uid,
            };
            for (allowed, certificate) in &mode.allowed {
                if allowed == package
                    && caller
                        .package_certificate(package, Some(certificate), CERT_INPUT_SHA256)
                        .map_err(not_modelled)?
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn not_allowed(handle: &BlobHandle, uid: i32, package: &str) -> Exception {
        Exception::security(format!(
            "Caller not allowed to access {}; callingUid={uid}, callingPackage={package}",
            handle.describe()
        ))
    }

    /// The blob of `handle` the caller may access.
    fn accessible<'a>(
        &self,
        store: &'a mut Store,
        handle: &BlobHandle,
        uid: i32,
        package: &str,
    ) -> Result<&'a mut Blob> {
        let index = store.blobs.iter().position(|b| b.handle == *handle);
        match index {
            Some(i) if self.access_allowed(&store.blobs[i], package, uid)? => {
                Ok(&mut store.blobs[i])
            }
            _ => Err(Self::not_allowed(handle, uid, package)),
        }
    }

    /// `verifyCallerIsSystemUid`: an admin user's app of the system uid.
    fn verify_caller_is_system_uid(&self, uid: i32, operation: &str) -> Result<()> {
        if uid % PER_USER_RANGE as i32 != SYSTEM_UID
            || !super::user_service::is_admin(&self.host, user_of(uid))?
        {
            return Err(Exception::security(format!(
                "Only admin user's app with system uidare allowed to call #{operation}"
            )));
        }
        Ok(())
    }

    // Leases' descriptions, from the leasee's resources.

    /// The package of a user, if it is installed there
    /// (`getResourcesForApplication` finds it).
    fn package_of_user(
        &self,
        package: &str,
        user: i32,
    ) -> Result<Option<crate::package::pkg::AndroidPackage>> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .packages
            .get(package)
            .filter(|ps| ps.users.get(&user).is_some_and(|u| u.installed))
            .and_then(|ps| ps.pkg.as_deref().cloned()))
    }

    /// `Leasee`'s constructor: the entry name of `res_id` and, without a
    /// description, its string, in the leasee's resources.
    fn lease_description(
        &self,
        package: &str,
        uid: i32,
        res_id: i32,
        description: Option<String>,
    ) -> Result<(Option<String>, Option<String>)> {
        let pkg = match self.package_of_user(package, user_of(uid))? {
            Some(pkg) if valid_resource_id(res_id) => pkg,
            _ => return Ok((None, description)),
        };
        let inputs = self.host.platform()?;
        let found = self
            .host
            .framework()?
            .resource_names_and_strings(self.host.root()?, &inputs.data, &pkg, &[res_id as u32])
            .map_err(|e| illegal_state(format!("blob_store: {e}")))?;
        let (name, string) = found.into_iter().next().unwrap_or_default();
        let not_found = |what: &str| {
            Exception::illegal_argument(format!(
                "android.content.res.Resources$NotFoundException: {what} #0x{:x}",
                res_id as u32
            ))
        };
        let Some((_, _, entry)) = name else {
            return Err(not_found("Unable to find resource ID"));
        };
        let description = match description {
            Some(d) => Some(d),
            None => Some(string.ok_or_else(|| not_found("String resource ID"))?),
        };
        Ok((Some(entry), description))
    }

    /// `getDescriptionResourceId`: the id of a lease's description by its
    /// entry name, in the leasee's resources now.
    fn description_res_id(&self, leasee: &Leasee) -> Result<i32> {
        let Some(name) = &leasee.description_res_entry_name else {
            return Ok(0);
        };
        let Some(pkg) = self.package_of_user(&leasee.package, user_of(leasee.uid))? else {
            return Ok(0);
        };
        let inputs = self.host.platform()?;
        self.host
            .framework()?
            .string_identifier(self.host.root()?, &inputs.data, &pkg, name)
            .map(|id| id as i32)
            .map_err(|e| illegal_state(format!("blob_store: {e}")))
    }

    // The calls.

    /// `createSessionInternal`.
    fn create_session(&self, handle: BlobHandle, uid: i32, package: String) -> Result<i64> {
        let config = self.config()?;
        self.with(|store| {
            let count = store
                .user_sessions(user_of(uid))
                .filter(|s| s.owner_uid == uid && s.owner_package == package)
                .count();
            if count >= config.max_active_sessions.max(0) as usize {
                return Err(limit_exceeded(&format!(
                    "Too many active sessions for the caller: {count}"
                )));
            }
            let id = generate_session_id(&store.known_ids)?;
            let file = self.blob_file(id)?;
            let persisted = Persisted {
                id,
                package,
                uid,
                creation_time_ms: now_millis(),
                handle,
                access: AccessMode::default(),
            };
            store.add_session(self.new_session(persisted, file));
            self.write_sessions(store);
            Ok(id)
        })
    }

    /// `openSessionInternal`: the caller's session, opened.
    fn open_session(&self, id: i64, uid: i32, package: &str) -> Result<Arc<Session>> {
        let session = self.with(|store| {
            store
                .sessions
                .get(&user_of(uid))
                .and_then(|s| s.get(&id))
                .filter(|s| {
                    s.owner_uid == uid
                        && s.owner_package == package
                        && !Session::finalized(s.state.lock().unwrap().state)
                })
                .cloned()
                .ok_or_else(|| Exception::security(format!("Session not found: {id}")))
        })?;
        session.open()?;
        Ok(session)
    }

    /// `acquireLeaseInternal`.
    fn acquire_lease(
        &self,
        handle: &BlobHandle,
        res_id: i32,
        description: Option<String>,
        expiry: i64,
        uid: i32,
        package: &str,
    ) -> Result<()> {
        let config = self.config()?;
        let total = self.host.data_space()?.0;
        self.with(|store| {
            let leases = store
                .blobs
                .iter()
                .filter(|b| b.leasee(package, uid).is_some())
                .count();
            if leases >= config.max_leased_blobs.max(0) as usize {
                return Err(limit_exceeded(&format!(
                    "Too many leased blobs for the caller: {leases}"
                )));
            }
            if expiry != 0 && handle.expiry_time_millis != 0 && expiry > handle.expiry_time_millis {
                return Err(Exception::illegal_argument(
                    "Lease expiry cannot be later than blobs expiry time",
                ));
            }
            let used: i64 = store
                .blobs
                .iter()
                .filter(|b| b.leasee(package, uid).is_some())
                .map(|b| self.blob_size(b.id))
                .sum();
            let remaining = (config.app_data_bytes_limit(total) - used).max(0);
            let blob = self.accessible(store, handle, uid, package)?;
            if self.blob_size(blob.id) > remaining {
                return Err(limit_exceeded(
                    "Total amount of data with an active lease is exceeding the max limit",
                ));
            }
            let (res_name, description) =
                self.lease_description(package, uid, res_id, description)?;
            let blob = self.accessible(store, handle, uid, package)?;
            blob.remove_leasee(package, uid);
            blob.leasees.push(Leasee {
                package: package.to_owned(),
                uid,
                description_res_entry_name: res_name,
                description,
                expiry_time_millis: expiry,
            });
            let _ = self.write_blobs(store);
            Ok(())
        })
    }

    /// `BlobMetadata.getSize`: its file's length.
    fn blob_size(&self, id: i64) -> i64 {
        self.blob_file(id)
            .ok()
            .and_then(|f| std::fs::metadata(f).ok())
            .map_or(0, |m| m.len() as i64)
    }

    /// `releaseLeaseInternal`: the lease goes, and the blob with it once
    /// the deletion delay has passed if no lease is left.
    fn release_lease(&self, handle: &BlobHandle, uid: i32, package: &str) -> Result<()> {
        let config = self.config()?;
        self.with(|store| {
            let blob = self.accessible(store, handle, uid, package)?;
            blob.remove_leasee(package, uid);
            if !blob.has_valid_leases() {
                let (handle, id) = (handle.clone(), blob.id);
                let delay =
                    Duration::from_millis(config.delete_on_last_lease_delay_ms.max(0) as u64);
                self.post_delayed(delay, move |this| {
                    let deleted = this.with(|store| {
                        let Some(index) = store
                            .blobs
                            .iter()
                            .position(|b| b.handle == handle && b.id == id)
                        else {
                            return Ok(());
                        };
                        let config = this.config()?;
                        if store.blobs[index].should_be_deleted(&config, true) {
                            this.delete_blob(store, id);
                            store.blobs.remove(index);
                        }
                        let _ = this.write_blobs(store);
                        Ok(())
                    });
                    if let Err(e) = deleted {
                        eprintln!(
                            "app host: blob_store: blob {id} without leases: {}",
                            e.message
                        );
                    }
                });
            }
            let _ = self.write_blobs(store);
            Ok(())
        })
    }

    /// `handlePackageRemoved`, as `PackageChangedReceiver` runs it for a
    /// package whose data was cleared: its sessions deleted; it is no
    /// longer a committer or leasee of any blob, and a blob it leaves
    /// deletable is deleted (a committer's once the lease wait is over).
    fn handle_package_removed(&self, package: &str, uid: i32) -> Result<()> {
        let config = self.config()?;
        self.with(|store| {
            let owned: Vec<Arc<Session>> = store
                .sessions
                .get(&user_of(uid))
                .into_iter()
                .flat_map(|s| s.values())
                .filter(|s| s.owner_uid == uid && s.owner_package == package)
                .cloned()
                .collect();
            for session in owned {
                Self::delete_session(store, &session);
                store.remove_session(&session);
            }
            self.write_sessions(store);
            let mut deleted = Vec::new();
            store.blobs.retain_mut(|blob| {
                let committer = blob.committer(package, uid).is_some();
                if committer {
                    blob.committers
                        .retain(|c| !(c.uid == uid && c.package == package));
                }
                blob.remove_leasee(package, uid);
                let delete = blob.should_be_deleted(&config, committer);
                if delete {
                    deleted.push(blob.id);
                }
                !delete
            });
            for id in deleted {
                self.delete_blob(store, id);
            }
            let _ = self.write_blobs(store);
            Ok(())
        })
    }

    /// `queryBlobsForUserInternal`.
    fn query_blobs(&self, user: i32) -> Result<Vec<BlobInfo>> {
        let blobs = self.with(|store| Ok(store.blobs.clone()))?;
        let mut infos = Vec::new();
        for blob in blobs.iter().filter(|b| b.in_user(user)) {
            let mut leases = Vec::new();
            for leasee in blob
                .leasees
                .iter()
                .filter(|l| l.valid() && user_of(l.uid) == user)
            {
                leases.push(LeaseInfo {
                    package: leasee.package.clone(),
                    expiry_time_millis: if leasee.expiry_time_millis == 0 {
                        blob.handle.expiry_time_millis
                    } else {
                        leasee.expiry_time_millis
                    },
                    description_res_id: self.description_res_id(leasee)?,
                    description: leasee.description.clone(),
                });
            }
            infos.push(BlobInfo {
                id: blob.id,
                expiry_time_millis: blob.handle.expiry_time_millis,
                label: blob.handle.label.clone(),
                size: self.blob_size(blob.id),
                leases,
            });
        }
        Ok(infos)
    }

    fn call(&self, call: &mut Call<'_>, p: &mut Parcel) -> CallResult {
        let uid = call.sender_euid as i32;
        match call.code {
            bs::CREATE_SESSION => {
                let a =
                    bs::CreateSession::<BlobHandle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let (handle, package) = Self::handle_and_package(a.handle, a.package_name)?;
                    self.verify_calling_package(uid, Some(&package))?;
                    self.enforce_allowed(uid, &package, "create session")?;
                    let id = self.create_session(handle, uid, package)?;
                    bs::write_create_session_reply(p, id);
                    Ok(())
                })())
            }
            bs::OPEN_SESSION => {
                let a = bs::OpenSession::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    if a.session_id <= 0 {
                        return Err(Exception::illegal_argument(format!(
                            "sessionId must be positive: {}",
                            a.session_id
                        )));
                    }
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    self.verify_calling_package(uid, Some(&package))?;
                    let session = self.open_session(a.session_id, uid, &package)?;
                    let binder = *session.binder.lock().unwrap();
                    bs::write_open_session_reply(p, binder);
                    Ok(())
                })())
            }
            bs::ABANDON_SESSION => {
                let a = bs::AbandonSession::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    if a.session_id <= 0 {
                        return Err(Exception::illegal_argument(format!(
                            "sessionId must be positive: {}",
                            a.session_id
                        )));
                    }
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    self.verify_calling_package(uid, Some(&package))?;
                    let session = self.open_session(a.session_id, uid, &package)?;
                    session.open()?;
                    session.close_session(uid, STATE_ABANDONED, true)?;
                    self.with(|store| {
                        self.write_sessions(store);
                        Ok(())
                    })?;
                    bs::write_abandon_session_reply(p);
                    Ok(())
                })())
            }
            bs::OPEN_BLOB => {
                let a = bs::OpenBlob::<BlobHandle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let (handle, package) = Self::handle_and_package(a.handle, a.package_name)?;
                    self.verify_calling_package(uid, Some(&package))?;
                    self.enforce_allowed(uid, &package, "open blob")?;
                    let config = self.config()?;
                    let id =
                        self.with(|store| Ok(self.accessible(store, &handle, uid, &package)?.id))?;
                    if config.use_revocable_fd_for_reads {
                        return Err(self.unimplemented("openBlob: a revocable file descriptor"));
                    }
                    let fd = open_read(&self.blob_file(id)?)?;
                    bs::write_open_blob_reply(p, Some(&fd));
                    Ok(())
                })())
            }
            bs::ACQUIRE_LEASE => {
                let a = bs::AcquireLease::<BlobHandle, Text>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let handle = a.handle.ok_or_else(|| npe("blobHandle must not be null"))?;
                    handle.assert_valid()?;
                    let description = a.description.and_then(|t| t.0);
                    if !valid_resource_id(a.description_res_id) && description.is_none() {
                        return Err(Exception::illegal_argument(format!(
                            "Description must be valid; descriptionId={}, description=null",
                            a.description_res_id
                        )));
                    }
                    if a.lease_timeout_millis < 0 {
                        return Err(Exception::illegal_argument(
                            "leaseExpiryTimeMillis must not be negative",
                        ));
                    }
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    // `getTruncatedLeaseDescription`.
                    let limit = self.config()?.lease_desc_char_limit.max(1) as usize;
                    let description = description.map(|d| {
                        if d.is_empty() {
                            d
                        } else {
                            trim_with_ellipsis(&d, limit)
                        }
                    });
                    self.verify_calling_package(uid, Some(&package))?;
                    self.enforce_allowed(uid, &package, "open blob")?;
                    self.acquire_lease(
                        &handle,
                        a.description_res_id,
                        description,
                        a.lease_timeout_millis,
                        uid,
                        &package,
                    )?;
                    bs::write_acquire_lease_reply(p);
                    Ok(())
                })())
            }
            bs::RELEASE_LEASE => {
                let a =
                    bs::ReleaseLease::<BlobHandle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let (handle, package) = Self::handle_and_package(a.handle, a.package_name)?;
                    self.verify_calling_package(uid, Some(&package))?;
                    self.enforce_allowed(uid, &package, "open blob")?;
                    self.release_lease(&handle, uid, &package)?;
                    bs::write_release_lease_reply(p);
                    Ok(())
                })())
            }
            bs::RELEASE_ALL_LEASES => {
                let a = bs::ReleaseAllLeases::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    self.verify_calling_package(uid, Some(&package))?;
                    self.enforce_allowed(uid, &package, "open blob")?;
                    self.with(|store| {
                        for blob in &mut store.blobs {
                            blob.remove_leasee(&package, uid);
                        }
                        let _ = self.write_blobs(store);
                        Ok(())
                    })?;
                    bs::write_release_all_leases_reply(p);
                    Ok(())
                })())
            }
            bs::GET_REMAINING_LEASE_QUOTA_BYTES => {
                let a =
                    bs::GetRemainingLeaseQuotaBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let package = a.package_name;
                    self.verify_calling_package(uid, package.as_deref())?;
                    let package = package.unwrap_or_default();
                    let config = self.config()?;
                    let total = self.host.data_space()?.0;
                    let used: i64 = self.with(|store| {
                        Ok(store
                            .blobs
                            .iter()
                            .filter(|b| b.leasee(&package, uid).is_some())
                            .map(|b| self.blob_size(b.id))
                            .sum())
                    })?;
                    let remaining = (config.app_data_bytes_limit(total) - used).max(0);
                    bs::write_get_remaining_lease_quota_bytes_reply(p, remaining);
                    Ok(())
                })())
            }
            bs::WAIT_FOR_IDLE => {
                let a = bs::WaitForIdle::<RemoteCallback>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let target = a
                        .callback
                        .and_then(|c| c.0)
                        .ok_or_else(|| npe("remoteCallback must not be null"))?;
                    if self.host.check_permission(Some(DUMP), uid)? != 0 {
                        return Err(Exception::security(format!(
                            "Caller is not allowed to call this; caller={uid}: Neither user {uid} nor current process has {DUMP}."
                        )));
                    }
                    let Binder::Handle(h) = target else {
                        return Err(illegal_state("blob_store: a local callback".into()));
                    };
                    let strong = self.host.process.strong(h);
                    // After what the handler has queued: `sendResult(null)`.
                    self.post(move |_| {
                        let mut data = Parcel::new();
                        remote_callback::SendResult::<super::bind::Bundle> { data: None }
                            .write(&mut data);
                        if let Err(e) = strong.transact(remote_callback::SEND_RESULT, &data, true) {
                            eprintln!("app host: blob_store: waitForIdle's callback: {e}");
                        }
                    });
                    bs::write_wait_for_idle_reply(p);
                    Ok(())
                })())
            }
            bs::QUERY_BLOBS_FOR_USER => {
                let a = bs::QueryBlobsForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    self.verify_caller_is_system_uid(uid, "queryBlobsForUser")?;
                    let user = if a.user_id == USER_CURRENT {
                        0
                    } else {
                        a.user_id
                    };
                    // `ensureNotSpecialUser`.
                    if user < 0 {
                        return Err(Exception::illegal_argument(format!(
                            "Call does not support special user #{user}"
                        )));
                    }
                    let infos: Vec<_> = self.query_blobs(user)?.into_iter().map(Some).collect();
                    bs::write_query_blobs_for_user_reply(p, Some(&infos));
                    Ok(())
                })())
            }
            bs::DELETE_BLOB => {
                let a = bs::DeleteBlob::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    self.verify_caller_is_system_uid(uid, "deleteBlob")?;
                    self.with(|store| {
                        let ids: Vec<i64> = store
                            .blobs
                            .iter()
                            .filter(|b| b.id == a.blob_id)
                            .map(|b| b.id)
                            .collect();
                        for id in ids {
                            self.delete_blob(store, id);
                        }
                        store.blobs.retain(|b| b.id != a.blob_id);
                        let _ = self.write_blobs(store);
                        Ok(())
                    })?;
                    bs::write_delete_blob_reply(p);
                    Ok(())
                })())
            }
            bs::GET_LEASED_BLOBS => {
                let a = bs::GetLeasedBlobs::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let package = a
                        .package_name
                        .ok_or_else(|| npe("packageName must not be null"))?;
                    self.verify_calling_package(uid, Some(&package))?;
                    let handles = self.with(|store| {
                        Ok(store
                            .blobs
                            .iter()
                            .filter(|b| b.leasee(&package, uid).is_some())
                            .map(|b| Some(b.handle.clone()))
                            .collect::<Vec<_>>())
                    })?;
                    bs::write_get_leased_blobs_reply(p, Some(&handles));
                    Ok(())
                })())
            }
            bs::GET_LEASE_INFO => {
                let a =
                    bs::GetLeaseInfo::<BlobHandle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok((|| -> Result<()> {
                    let (handle, package) =
                        Self::handle_and_package(a.blob_handle, a.package_name)?;
                    self.verify_calling_package(uid, Some(&package))?;
                    self.enforce_allowed(uid, &package, "open blob")?;
                    let leasee = self.with(|store| {
                        let blob = self.accessible(store, &handle, uid, &package)?;
                        Ok(blob
                            .leasees
                            .iter()
                            .find(|l| l.valid() && l.uid == uid && l.package == package)
                            .cloned())
                    })?;
                    let info = match leasee {
                        Some(l) => Some(LeaseInfo {
                            package: package.clone(),
                            expiry_time_millis: l.expiry_time_millis,
                            description_res_id: self.description_res_id(&l)?,
                            description: l.description.clone(),
                        }),
                        None => None,
                    };
                    bs::write_get_lease_info_reply(p, info.as_ref());
                    Ok(())
                })())
            }
            DUMP_TRANSACTION => Ok(Err(self.unimplemented("dump"))),
            SHELL_COMMAND_TRANSACTION => Ok(Err(self.unimplemented("its shell command"))),
            code => {
                let name = bs::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Ok(Err(self.unimplemented(name)))
            }
        }
    }

    fn unimplemented(&self, name: &str) -> Exception {
        self.host
            .unimplemented_named("blob_store", &format!("{name} (#1938)"))
    }
}

/// `generateNextSessionIdLocked`: a random positive id not known yet.
fn generate_session_id(known: &HashSet<i64>) -> Result<i64> {
    for _ in 0..=32 {
        let mut value = 0i64;
        // SAFETY: arc4random_buf fills the eight bytes of a local.
        unsafe {
            libc::arc4random_buf((&mut value as *mut i64).cast(), std::mem::size_of::<i64>());
        }
        let id = if value == i64::MIN { 0 } else { value.abs() };
        if id != 0 && !known.contains(&id) {
            return Ok(id);
        }
    }
    Err(illegal_state("Failed to allocate session ID".into()))
}

impl PackageMonitor for BlobStore {
    /// `PackageChangedReceiver`, on the service's handler.
    fn package_data_cleared(&self, cleared: &DataCleared<'_>) {
        if cleared.uid == -1 {
            eprintln!(
                "app host: blob_store: uid is missing in {}'s data cleared",
                cleared.package
            );
            return;
        }
        let (package, uid) = (cleared.package.to_string(), cleared.uid);
        self.post(move |store| {
            if let Err(e) = store.handle_package_removed(&package, uid) {
                eprintln!("app host: blob_store: {package}'s data: {}", e.message);
            }
        });
    }
}

impl Service for BlobStore {
    fn descriptor(&self) -> &str {
        bs::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = self.call(call, &mut p)?;
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT;

    fn handle(digest: &[u8]) -> BlobHandle {
        BlobHandle {
            algorithm: Some(ALGO_SHA_256.into()),
            digest: Some(digest.to_vec()),
            label: Some("Label".into()),
            expiry_time_millis: 0,
            tag: Some("tag".into()),
        }
    }

    fn session(id: i64, handle: BlobHandle, file: PathBuf, access: AccessMode) -> Arc<Session> {
        Arc::new(Session {
            id,
            handle,
            owner_uid: 10_100,
            owner_package: "com.example".into(),
            creation_time_ms: 1000,
            file,
            state: Mutex::new(SessionState {
                state: STATE_OPENED,
                access,
                commit_callback: None,
            }),
            store: Weak::new(),
            binder: Mutex::new(None),
        })
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aim-blob-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `Base64.DEFAULT`: lines of 76 characters, each ended by a newline,
    /// read back whatever the breaks.
    #[test]
    fn digests_are_written_in_lines() {
        assert_eq!(
            base64_default(&[1; 32]),
            "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=\n"
        );
        let long = base64_default(&[0; 58]);
        assert_eq!(long, format!("{}\nAA==\n", "A".repeat(76)));
        assert_eq!(base64_decode(&long), Some(vec![0; 58]));
        assert_eq!(base64_default(&[]), "");
    }

    /// `writeBlobSessionsLocked`, as `FastXmlSerializer` writes it, and
    /// `readBlobSessionsLocked` of it.
    #[test]
    fn sessions_index_is_the_originals() {
        let mut access = AccessMode::default();
        access.kind |= ACCESS_TYPE_PUBLIC;
        access.allow_package("com.friend".into(), vec![2; 32]);
        let s = session(7, handle(&[1; 32]), PathBuf::new(), access.clone());
        let root = Tag::new("ss")
            .attr("v", XML_VERSION_CURRENT)
            .child(s.to_xml());
        let document = root.document();
        assert_eq!(
            document,
            "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n\
             <ss v=\"6\">\n\
             <s id=\"7\" p=\"com.example\" u=\"10100\" crt=\"1000\">\n\
             <bh al=\"SHA-256\" dg=\"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=&#10;\" lbl=\"Label\" ex=\"0\" tg=\"tag\" />\n\
             <am t=\"11\">\n\
             <wl p=\"com.friend\" ct=\"AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=&#10;\" />\n\
             </am>\n\
             </s>\n\
             </ss>\n"
        );
        let (sessions, error) = parse_sessions(document.as_bytes());
        assert_eq!(error, None);
        assert_eq!(
            sessions,
            vec![Persisted {
                id: 7,
                package: "com.example".into(),
                uid: 10_100,
                creation_time_ms: 1000,
                handle: handle(&[1; 32]),
                access,
            }]
        );
        let empty = Tag::new("ss").attr("v", XML_VERSION_CURRENT).document();
        assert!(empty.ends_with("<ss v=\"6\" />\n"));
        assert_eq!(parse_sessions(empty.as_bytes()), (Vec::new(), None));
    }

    /// `writeBlobsInfoLocked` and `readBlobsInfoLocked`: a blob with its
    /// handle, committers and leasees.
    #[test]
    fn blobs_index_round_trips() {
        let blob = Blob {
            id: 42,
            handle: handle(&[1; 32]),
            committers: vec![Committer {
                package: "com.example".into(),
                uid: 10_100,
                access: AccessMode {
                    kind: ACCESS_TYPE_PRIVATE | ACCESS_TYPE_SAME_SIGNATURE,
                    allowed: Vec::new(),
                },
                commit_time_ms: 5,
            }],
            leasees: vec![Leasee {
                package: "com.reader".into(),
                uid: 10_101,
                description_res_entry_name: None,
                description: Some("A \"model\" & <more>".into()),
                expiry_time_millis: 0,
            }],
        };
        let document = Tag::new("bs")
            .attr("v", XML_VERSION_CURRENT)
            .child(blob.to_xml())
            .document();
        assert!(document.contains(
            "<b id=\"42\">\n<bh al=\"SHA-256\" dg=\"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=&#10;\" lbl=\"Label\" ex=\"0\" tg=\"tag\" />\n\
             <c p=\"com.example\" u=\"10100\" cmt=\"5\">\n<am t=\"5\" />\n</c>\n\
             <l p=\"com.reader\" u=\"10101\" ex=\"0\" d=\"A &quot;model&quot; &amp; &lt;more&gt;\" />\n</b>\n"
        ));
        assert_eq!(
            parse_blobs(document.as_bytes()),
            (XML_VERSION_CURRENT, vec![blob], None)
        );
    }

    /// An index of an older version: its user, no commit times and no
    /// descriptions; equal accessors kept once, the first.
    #[test]
    fn older_indexes_are_read() {
        let xml = "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n\
                   <bs v=\"1\">\n<b id=\"3\" us=\"0\">\n\
                   <bh al=\"SHA-256\" dg=\"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=&#10;\" lbl=\"L\" ex=\"0\" tg=\"t\" />\n\
                   <c p=\"a\" u=\"10100\" cmt=\"9\">\n<am t=\"1\" />\n</c>\n\
                   <c p=\"a\" u=\"10100\">\n<am t=\"2\" />\n</c>\n\
                   <c p=\"b\" u=\"10101\" />\n\
                   <l p=\"a\" u=\"10100\" rn=\"desc\" ex=\"7\" d=\"D\" />\n\
                   </b>\n</bs>\n";
        let (version, blobs, error) = parse_blobs(xml.as_bytes());
        assert_eq!((version, error), (1, None));
        let b = &blobs[0];
        assert_eq!(b.id, 3);
        // The second of `a` is the same committer; `b` has no access mode.
        assert_eq!(b.committers.len(), 1);
        assert_eq!(
            (b.committers[0].commit_time_ms, b.committers[0].access.kind),
            (0, 1)
        );
        assert_eq!(
            b.leasees[0],
            Leasee {
                package: "a".into(),
                uid: 10_100,
                description_res_entry_name: None,
                description: None,
                expiry_time_millis: 7,
            }
        );
        // Without a user, a blob of version 1 does not read.
        let without_user = xml.replace(" us=\"0\"", "");
        assert!(parse_blobs(without_user.as_bytes()).2.is_some());
    }

    /// An invalid handle ends the reading: the sessions before it stay.
    #[test]
    fn an_invalid_handle_ends_the_reading() {
        let s = session(1, handle(&[1; 32]), PathBuf::new(), AccessMode::default());
        let mut bad = handle(&[1; 32]);
        bad.label = Some(String::new());
        let t = session(2, bad, PathBuf::new(), AccessMode::default());
        let u = session(3, handle(&[2; 32]), PathBuf::new(), AccessMode::default());
        let document = Tag::new("ss")
            .attr("v", XML_VERSION_CURRENT)
            .child(s.to_xml())
            .child(t.to_xml())
            .child(u.to_xml())
            .document();
        let (sessions, error) = parse_sessions(document.as_bytes());
        assert_eq!(sessions.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(error.as_deref(), Some("label must not be null"));
    }

    /// `BlobHandle.assertIsValid`'s checks, in order.
    #[test]
    fn handles_are_checked() {
        let message = |f: fn(&mut BlobHandle)| {
            let mut h = handle(&[1; 32]);
            f(&mut h);
            h.assert_valid().err().map(|e| (e.code, e.message))
        };
        assert_eq!(message(|_| {}), None);
        assert_eq!(
            message(|h| h.algorithm = Some("MD5".into())),
            Some((EX_ILLEGAL_ARGUMENT, "MD5is not supported [SHA-256]".into()))
        );
        assert_eq!(
            message(|h| h.digest = None),
            Some((EX_NULL_POINTER, "digest must not be null".into()))
        );
        assert_eq!(
            message(|h| h.digest = Some(Vec::new())),
            Some((EX_ILLEGAL_ARGUMENT, "digest is empty".into()))
        );
        assert_eq!(
            message(|h| h.label = Some("x".repeat(101))),
            Some((EX_ILLEGAL_ARGUMENT, "label too long".into()))
        );
        assert_eq!(
            message(|h| h.expiry_time_millis = -1),
            Some((
                EX_ILLEGAL_ARGUMENT,
                "expiryTimeMillis must not be negative".into()
            ))
        );
        assert_eq!(
            message(|h| h.tag = None),
            Some((EX_ILLEGAL_ARGUMENT, "tag must not be null".into()))
        );
    }

    /// `BlobHandle.writeToParcel` and its creator; `BlobInfo`'s leases a
    /// `writeList` of length-prefixed parcelables.
    #[test]
    fn parcels_are_the_originals() {
        let h = handle(&[1; 32]);
        let mut p = Parcel::new();
        h.write_to(&mut p);
        assert_eq!(BlobHandle::read_from(&mut p.reader()).unwrap(), h);

        let info = BlobInfo {
            id: 1,
            expiry_time_millis: 2,
            label: Some("L".into()),
            size: 3,
            leases: vec![LeaseInfo {
                package: "p".into(),
                expiry_time_millis: 4,
                description_res_id: 0,
                description: None,
            }],
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!((r.read_i64().unwrap(), r.read_i64().unwrap()), (1, 2));
        assert_eq!(
            crate::clip::char_sequence(&mut r).unwrap().as_deref(),
            Some("L")
        );
        assert_eq!((r.read_i64().unwrap(), r.read_i32().unwrap()), (3, 1));
        assert_eq!(r.read_i32().unwrap(), crate::bundle::VAL_PARCELABLE);
        let length = r.read_i32().unwrap() as usize;
        let start = r.position();
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.app.blob.LeaseInfo")
        );
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p"));
        assert_eq!((r.read_i64().unwrap(), r.read_i32().unwrap()), (4, 0));
        assert_eq!(crate::clip::char_sequence(&mut r).unwrap(), None);
        assert_eq!(r.position() - start, length);
        assert_eq!(r.remaining(), 0);
    }

    /// `computeDigest` and `verifyBlobData`: the data's SHA-256 against
    /// the handle's; no data is no digest.
    #[test]
    fn commits_verify_the_digest() {
        let dir = temp_dir("digest");
        let file = dir.join("9");
        std::fs::write(&file, b"blob data").unwrap();
        let digest = Sha256::digest(b"blob data").to_vec();
        let valid = session(9, handle(&digest), file.clone(), AccessMode::default());
        valid.verify();
        assert_eq!(valid.state.lock().unwrap().state, STATE_VERIFIED_VALID);
        let invalid = session(9, handle(&[1; 32]), file, AccessMode::default());
        invalid.verify();
        assert_eq!(invalid.state.lock().unwrap().state, STATE_VERIFIED_INVALID);
        let missing = session(10, handle(&digest), dir.join("10"), AccessMode::default());
        missing.verify();
        assert_eq!(missing.state.lock().unwrap().state, STATE_VERIFIED_INVALID);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A session's states: only an opened one changes, closing one that is
    /// not open is nothing, and only its owner may.
    #[test]
    fn sessions_close_once() {
        let s = session(1, handle(&[1; 32]), PathBuf::new(), AccessMode::default());
        assert_eq!(
            s.close_session(10_101, STATE_CLOSED, false)
                .unwrap_err()
                .message,
            "10100 is not the session owner"
        );
        s.close_session(10_100, STATE_CLOSED, false).unwrap();
        s.close_session(10_100, STATE_CLOSED, false).unwrap();
        assert_eq!(
            s.close_session(10_100, STATE_ABANDONED, true)
                .unwrap_err()
                .message,
            "Not allowed to delete or abandon a session with state: <closed>"
        );
        s.open().unwrap();
        s.close_session(10_100, STATE_ABANDONED, true).unwrap();
        assert_eq!(
            s.open().unwrap_err().message,
            "Not allowed to open session with state: <abandoned>"
        );
    }

    /// `BlobStoreConfig`'s properties: their defaults for what is unset or
    /// does not parse; the per-app limit the larger of its floor and
    /// fraction.
    #[test]
    fn config_follows_device_config() {
        let config = Config::read(|_| None);
        assert_eq!(config.max_active_sessions, 250);
        assert_eq!(config.lease_acquisition_wait_ms, 6 * 3_600_000);
        assert!(!config.use_revocable_fd_for_reads);
        assert_eq!(config.app_data_bytes_limit(1 << 30), 300 << 20);
        // Of 100 GiB in float arithmetic, 1 GiB; in double it would be
        // 1073741800, as 0.01f is a little less than 0.01.
        assert_eq!(config.app_data_bytes_limit(100 << 30), 1 << 30);
        let config = Config::read(|key| match key {
            "max_active_sessions" => Some("3".into()),
            "max_leased_blobs" => Some("many".into()),
            "use_revocable_fd_for_reads" => Some("TRUE".into()),
            "total_bytes_per_app_limit_fraction" => Some("0.5".into()),
            _ => None,
        });
        assert_eq!(
            (config.max_active_sessions, config.max_leased_blobs),
            (3, 500)
        );
        assert!(config.use_revocable_fd_for_reads);
        assert_eq!(config.app_data_bytes_limit(1 << 30), 1 << 29);
    }

    /// `shouldBeDeleted`: an expired blob, and one without a valid lease
    /// once its committers' lease wait time has passed (or regardless).
    #[test]
    fn blobs_without_leases_go() {
        let config = Config::read(|_| None);
        let now = now_millis();
        let mut blob = Blob {
            id: 1,
            handle: handle(&[1; 32]),
            committers: vec![Committer {
                package: "a".into(),
                uid: 10_100,
                access: AccessMode::default(),
                commit_time_ms: now,
            }],
            leasees: Vec::new(),
        };
        assert!(!blob.should_be_deleted(&config, true));
        assert!(blob.should_be_deleted(&config, false));
        blob.committers[0].commit_time_ms = now - config.lease_acquisition_wait_ms - 1;
        assert!(blob.should_be_deleted(&config, true));
        blob.leasees.push(Leasee {
            package: "b".into(),
            uid: 10_101,
            description_res_entry_name: None,
            description: None,
            expiry_time_millis: 0,
        });
        assert!(!blob.should_be_deleted(&config, true));
        blob.leasees[0].expiry_time_millis = now - 1;
        assert!(blob.should_be_deleted(&config, true));
        blob.leasees[0].expiry_time_millis = 0;
        blob.handle.expiry_time_millis = now - 1;
        assert!(blob.should_be_deleted(&config, true));
    }

    /// The access mode's set of allowed packages and its type flags.
    #[test]
    fn access_modes_allow_packages() {
        let mut mode = AccessMode::default();
        assert!(!mode.package_allowed("p", &[1]));
        mode.allow_package("p".into(), vec![1]);
        mode.allow_package("p".into(), vec![1]);
        assert_eq!(mode.allowed.len(), 1);
        assert_eq!(mode.kind, ACCESS_TYPE_PRIVATE | ACCESS_TYPE_ALLOWLIST);
        assert!(mode.package_allowed("p", &[1]) && !mode.package_allowed("p", &[2]));
    }

    /// `TextUtils.trimToLengthWithEllipsis`, a surrogate pair not split;
    /// `String.hashCode`; `ResourceId.isValid`.
    #[test]
    fn java_helpers() {
        assert_eq!(trim_with_ellipsis("abc", 3), "abc");
        assert_eq!(trim_with_ellipsis("abcd", 3), "abc...");
        assert_eq!(trim_with_ellipsis("ab\u{1F600}c", 3), "ab...");
        assert_eq!((java_hash(""), java_hash("ab")), (0, 97 * 31 + 98));
        assert!(valid_resource_id(0x7f0a_0001));
        assert!(!valid_resource_id(0) && !valid_resource_id(-1) && !valid_resource_id(0x7f00_0001));
    }

    /// The errors a `ParcelableException` carries: its creator, then the
    /// cause's class and message.
    #[test]
    fn limits_are_parcelable_exceptions() {
        let e = limit_exceeded("Too many active sessions for the caller: 250");
        assert_eq!(
            (e.code, e.message.as_str()),
            (
                aim_binder_host::parcel::EX_PARCELABLE,
                "android.os.LimitExceededException: Too many active sessions for the caller: 250"
            )
        );
    }
}
