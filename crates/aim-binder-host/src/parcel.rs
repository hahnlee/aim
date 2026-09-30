//! Parcels in libbinder's kernel wire format (`Parcel.cpp`,
//! `android_os_Parcel.cpp` of the pinned tag), for binder processes on the
//! host ([`crate::local`]): native services and the AIDL code generated for
//! them (`crates/aim-services`).
//!
//! Values are little-endian and every write is padded to 4 bytes. A string
//! is its length (-1 for null) then its code units and a NUL; String16 is
//! Java's `writeString` and AIDL's `String`, String8 Java's `writeString8`.
//! A binder is a `flat_binder_object`, recorded in the object offsets,
//! followed by its stability (`Stability::Level`).

use aim_binder_driver::uapi::{
    BINDER_TYPE_BINDER, BINDER_TYPE_FD, BINDER_TYPE_HANDLE, FLAT_BINDER_FLAG_ACCEPTS_FDS,
    FLAT_BINDER_FLAG_PRIORITY_MASK, FLAT_BINDER_OBJECT_SIZE, FlatBinderObject,
};

/// A libbinder `status_t`.
pub type StatusCode = i32;
pub const OK: StatusCode = 0;
pub const BAD_VALUE: StatusCode = -22;
pub const NOT_ENOUGH_DATA: StatusCode = -61;
pub const UNKNOWN_TRANSACTION: StatusCode = -74;
pub const PERMISSION_DENIED: StatusCode = -1;
pub const DEAD_OBJECT: StatusCode = -32;
pub const BAD_TYPE: StatusCode = i32::MIN + 1;
pub const FAILED_TRANSACTION: StatusCode = i32::MIN + 2;

/// `Stability::Level::SYSTEM`: what the platform's own binders carry.
const STABILITY_SYSTEM: i32 = 0b001100;
/// `Parcel::kHeader` of the platform (`B_PACK_CHARS('S', 'Y', 'S', 'T')`).
const HEADER: i32 = i32::from_be_bytes(*b"SYST");
/// `STRICT_MODE_PENALTY_GATHER`.
const STRICT_MODE_PENALTY_GATHER: i32 = 1 << 31;
/// `IPCThreadState::kUnsetWorkSource`.
const UNSET_WORK_SOURCE: i32 = -1;
/// Scheduling bits libbinder gives a local binder: SCHED_NORMAL, nice 19.
const DEFAULT_SCHED_BITS: u32 = 19 & FLAT_BINDER_FLAG_PRIORITY_MASK;

/// `binder::Status` / `android.os.Parcel` exception codes.
pub const EX_SECURITY: i32 = -1;
pub const EX_ILLEGAL_ARGUMENT: i32 = -3;
pub const EX_NULL_POINTER: i32 = -4;
pub const EX_ILLEGAL_STATE: i32 = -5;
pub const EX_UNSUPPORTED_OPERATION: i32 = -7;
pub const EX_SERVICE_SPECIFIC: i32 = -8;
/// Reply headers that precede the exception code.
const EX_HAS_NOTED_APPOPS_REPLY_HEADER: i32 = -127;
const EX_HAS_STRICTMODE_REPLY_HEADER: i32 = -128;

/// A binder in a parcel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binder {
    /// A reference to another process's node.
    Handle(u32),
    /// A node of this process (its `binder` pointer value).
    Local(u64),
}

/// A Java exception (`Parcel.writeException`), or `binder::Status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exception {
    pub code: i32,
    pub message: String,
    /// `EX_SERVICE_SPECIFIC`'s error code.
    pub service_specific: i32,
}

impl Exception {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            service_specific: 0,
        }
    }

    pub fn security(message: impl Into<String>) -> Self {
        Self::new(EX_SECURITY, message)
    }

    pub fn illegal_argument(message: impl Into<String>) -> Self {
        Self::new(EX_ILLEGAL_ARGUMENT, message)
    }
}

/// A parcel being written.
#[derive(Clone, Debug, Default)]
pub struct Parcel {
    data: Vec<u8>,
    objects: Vec<u64>,
}

impl Parcel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The offsets of the binder objects in [`Parcel::data`].
    pub fn objects(&self) -> &[u64] {
        &self.objects
    }

    fn pad(&mut self) {
        let padded = self.data.len().next_multiple_of(4);
        self.data.resize(padded, 0);
    }

    pub fn write_i32(&mut self, value: i32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i64(&mut self, value: i64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_f32(&mut self, value: f32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// `writeBoolean`: an int.
    pub fn write_bool(&mut self, value: bool) {
        self.write_i32(value.into());
    }

    /// `writeString16` (Java's `writeString`).
    pub fn write_string16(&mut self, value: Option<&str>) {
        let Some(value) = value else {
            return self.write_i32(-1);
        };
        let units: Vec<u16> = value.encode_utf16().collect();
        self.write_i32(units.len() as i32);
        for unit in units.iter().chain([&0]) {
            self.data.extend_from_slice(&unit.to_le_bytes());
        }
        self.pad();
    }

    /// `writeString8` (UTF-8).
    pub fn write_string8(&mut self, value: Option<&str>) {
        let Some(value) = value else {
            return self.write_i32(-1);
        };
        self.write_i32(value.len() as i32);
        self.data.extend_from_slice(value.as_bytes());
        self.data.push(0);
        self.pad();
    }

    /// A binder and its stability, as `Parcel::flattenBinder` writes it.
    pub fn write_binder(&mut self, binder: Option<Binder>) {
        let object = match binder {
            Some(Binder::Handle(handle)) => FlatBinderObject {
                kind: BINDER_TYPE_HANDLE,
                flags: DEFAULT_SCHED_BITS,
                binder: u64::from(handle),
                cookie: 0,
            },
            Some(Binder::Local(ptr)) => FlatBinderObject {
                kind: BINDER_TYPE_BINDER,
                flags: DEFAULT_SCHED_BITS | FLAT_BINDER_FLAG_ACCEPTS_FDS,
                binder: ptr,
                cookie: ptr,
            },
            None => FlatBinderObject {
                kind: BINDER_TYPE_BINDER,
                flags: 0,
                binder: 0,
                cookie: 0,
            },
        };
        if binder.is_some() {
            self.objects.push(self.data.len() as u64);
        }
        self.data.extend_from_slice(&object.encode());
        self.write_i32(if binder.is_some() {
            STABILITY_SYSTEM
        } else {
            0
        });
    }

    /// The interface token of a call (`Parcel::writeInterfaceToken`).
    pub fn write_interface_token(&mut self, descriptor: &str) {
        self.write_i32(STRICT_MODE_PENALTY_GATHER);
        self.write_i32(UNSET_WORK_SOURCE);
        self.write_i32(HEADER);
        self.write_string16(Some(descriptor));
    }

    /// `writeNoException`.
    pub fn write_no_exception(&mut self) {
        self.write_i32(0);
    }

    /// `writeException`: the code, the message and an empty remote stack
    /// trace.
    pub fn write_exception(&mut self, exception: &Exception) {
        self.write_i32(exception.code);
        self.write_string16(Some(&exception.message));
        self.write_i32(0);
        if exception.code == EX_SERVICE_SPECIFIC {
            self.write_i32(exception.service_specific);
        }
    }

    /// Bytes read from another parcel, with the binder objects among them
    /// (offsets relative to `bytes`).
    pub fn write_raw(&mut self, bytes: &[u8], objects: &[u64]) {
        let base = self.data.len() as u64;
        self.objects.extend(objects.iter().map(|o| base + o));
        self.data.extend_from_slice(bytes);
        self.pad();
    }

    /// Where the next write goes.
    pub fn position(&self) -> usize {
        self.data.len()
    }

    /// Overwrites an int written earlier (a length filled in later).
    pub fn set_i32_at(&mut self, at: usize, value: i32) {
        self.data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

/// A received parcel being read.
pub struct Reader<'a> {
    data: &'a [u8],
    objects: &'a [u64],
    pos: usize,
}

pub type Result<T> = std::result::Result<T, StatusCode>;

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], objects: &'a [u64]) -> Self {
        Self {
            data,
            objects,
            pos: 0,
        }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// The bytes from `start` to the current position, and the binder
    /// objects among them (relative offsets), for [`Parcel::write_raw`].
    pub fn since(&self, start: usize) -> (&'a [u8], Vec<u64>) {
        let objects = self
            .objects
            .iter()
            .filter(|&&o| o as usize >= start && (o as usize) < self.pos)
            .map(|o| o - start as u64)
            .collect();
        (&self.data[start..self.pos], objects)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let padded = len.next_multiple_of(4);
        if self.remaining() < padded.max(len) {
            return Err(NOT_ENOUGH_DATA);
        }
        let bytes = &self.data[self.pos..self.pos + len];
        self.pos += padded;
        Ok(bytes)
    }

    pub fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        Ok(self.read_i32()? as u32)
    }

    pub fn read_i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read_i32()? != 0)
    }

    /// Skips `len` bytes (padded).
    pub fn skip(&mut self, len: usize) -> Result<()> {
        self.take(len).map(drop)
    }

    pub fn read_string16(&mut self) -> Result<Option<String>> {
        let len = self.read_i32()?;
        if len < 0 {
            return Ok(None);
        }
        let len = len as usize;
        let bytes = self.take(
            len.checked_mul(2)
                .and_then(|n| n.checked_add(2))
                .ok_or(BAD_VALUE)?,
        )?;
        let units: Vec<u16> = bytes[..2 * len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).map(Some).map_err(|_| BAD_VALUE)
    }

    pub fn read_string8(&mut self) -> Result<Option<String>> {
        let len = self.read_i32()?;
        if len < 0 {
            return Ok(None);
        }
        let bytes = self.take((len as usize).checked_add(1).ok_or(BAD_VALUE)?)?;
        Ok(Some(
            String::from_utf8_lossy(&bytes[..len as usize]).into_owned(),
        ))
    }

    /// A binder written by `flattenBinder`: the object must be one the
    /// driver recorded.
    pub fn read_binder(&mut self) -> Result<Option<Binder>> {
        let at = self.pos as u64;
        let raw = self.take(FLAT_BINDER_OBJECT_SIZE)?;
        let object = FlatBinderObject::decode(raw);
        self.read_i32()?; // stability
        let recorded = self.objects.contains(&at);
        match object.kind {
            BINDER_TYPE_BINDER if object.binder == 0 => Ok(None),
            BINDER_TYPE_BINDER if recorded => Ok(Some(Binder::Local(object.binder))),
            BINDER_TYPE_HANDLE if recorded => Ok(Some(Binder::Handle(object.handle()))),
            _ => Err(BAD_TYPE),
        }
    }

    /// A file descriptor (`writeFileDescriptor`): the fd the driver
    /// installed in this process ([`crate::local::LocalProcess::file`]).
    pub fn read_fd(&mut self) -> Result<u32> {
        let at = self.pos as u64;
        let raw = self.take(FLAT_BINDER_OBJECT_SIZE)?;
        let object = FlatBinderObject::decode(raw);
        if object.kind != BINDER_TYPE_FD || !self.objects.contains(&at) {
            return Err(BAD_TYPE);
        }
        Ok(object.binder as u32)
    }

    /// `enforceInterface`: the token of a call to `descriptor`.
    pub fn enforce_interface(&mut self, descriptor: &str) -> Result<()> {
        self.read_i32()?; // strict mode policy
        self.read_i32()?; // work source
        if self.read_i32()? != HEADER {
            return Err(BAD_TYPE);
        }
        match self.read_string16()? {
            Some(d) if d == descriptor => Ok(()),
            _ => Err(BAD_TYPE),
        }
    }

    /// A reply's status (`readException`): `Ok(Err(exception))` when the
    /// callee threw.
    pub fn read_exception(&mut self) -> Result<std::result::Result<(), Exception>> {
        let mut code = self.read_i32()?;
        if code == EX_HAS_NOTED_APPOPS_REPLY_HEADER {
            // AppOpsManager.readAndLogNotedAppops
            for _ in 0..self.read_i32()? {
                self.read_string16()?;
                self.read_i64()?;
                self.read_i64()?;
            }
            code = self.read_i32()?;
        }
        if code == EX_HAS_STRICTMODE_REPLY_HEADER {
            // Only sent with no exception; the size counts from itself.
            let start = self.pos;
            self.pos = start + self.read_i32()?.max(4) as usize;
            return Ok(Ok(()));
        }
        if code == 0 {
            return Ok(Ok(()));
        }
        let message = self.read_string16()?.unwrap_or_default();
        let trace = self.read_i32()?;
        if trace > 0 {
            self.read_string16()?;
        }
        let service_specific = if code == EX_SERVICE_SPECIFIC {
            self.read_i32()?
        } else {
            0
        };
        Ok(Err(Exception {
            code,
            message,
            service_specific,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_round_trip_with_padding() {
        let mut p = Parcel::new();
        p.write_string16(Some("clip"));
        p.write_string8(Some("héllo"));
        p.write_string16(None);
        p.write_i64(-2);
        // 4 + (4 + 1) * 2 = 14 -> 16; 4 + 6 + 1 = 11 -> 12
        assert_eq!(p.data().len(), 16 + 12 + 4 + 8);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("clip"));
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("héllo"));
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.read_i64().unwrap(), -2);
        assert_eq!(r.read_i32(), Err(NOT_ENOUGH_DATA));
    }

    #[test]
    fn interface_token_and_binders() {
        let mut p = Parcel::new();
        p.write_interface_token("android.content.IClipboard");
        p.write_binder(Some(Binder::Local(0x1000)));
        p.write_binder(None);
        assert_eq!(p.objects().len(), 1);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(r.enforce_interface("android.content.IClipboard"), Ok(()));
        assert_eq!(r.read_binder(), Ok(Some(Binder::Local(0x1000))));
        assert_eq!(r.read_binder(), Ok(None));
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(r.enforce_interface("android.os.IOther"), Err(BAD_TYPE));
    }

    #[test]
    fn exceptions() {
        let mut p = Parcel::new();
        p.write_exception(&Exception::security("no"));
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(r.read_exception(), Ok(Err(Exception::security("no"))));
        let mut p = Parcel::new();
        p.write_no_exception();
        assert_eq!(Reader::new(p.data(), &[]).read_exception(), Ok(Ok(())));
    }
}
