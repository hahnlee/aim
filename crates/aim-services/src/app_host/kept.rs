//! Parcelables the app host passes on as they arrived: an `Intent`
//! (parsed for matching) and a `Bundle`, and what keeps one past the call
//! that brought it: the binders among it held, the files of its fds kept.

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use crate::clip::{self, Raw};
use crate::package::intent::Intent;
use crate::package::intent_filter::Plain;
use crate::package::uri::Uri;

/// An `Intent` as `Intent.writeToParcel` wrote it.
#[derive(Clone)]
pub struct RawIntent {
    pub intent: Intent,
    raw: Raw,
    /// Where its flags are; its extended flags follow.
    flags_at: usize,
}

impl RawIntent {
    /// Sets its flags and extended flags.
    pub fn set_flags(&mut self, flags: i32, extended_flags: i32) {
        self.intent.flags = flags;
        self.intent.extended_flags = extended_flags;
        self.raw.set_i32(self.flags_at, flags);
        self.raw.set_i32(self.flags_at + 4, extended_flags);
    }

    /// `Intent.hasFileDescriptors`.
    pub fn has_fds(&self) -> bool {
        self.raw.has_fds()
    }

    pub fn handles(&self) -> Vec<u32> {
        self.raw.handles().collect()
    }
}

impl ReadParcelable for RawIntent {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.position();
        // Action, data, type and identifier come before the flags.
        r.read_string8()?;
        Uri::read(r, &mut Plain)?;
        r.read_string8()?;
        r.read_string8()?;
        let flags_at = r.position() - start;
        r.set_position(start);
        let intent = Intent::read(r, &mut Plain)?;
        Ok(RawIntent { intent, raw: Raw::capture(r, start), flags_at })
    }
}

impl WriteParcelable for RawIntent {
    fn write_to(&self, p: &mut Parcel) {
        self.raw.write(p);
    }
}

/// A `Bundle` as `BaseBundle.writeToParcelInner` wrote it.
#[derive(Clone)]
pub struct RawBundle(Raw);

impl RawBundle {
    /// `Bundle.hasFileDescriptors`.
    pub fn has_fds(&self) -> bool {
        self.0.has_fds()
    }
}

impl ReadParcelable for RawBundle {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.position();
        clip::bundle(r)?;
        Ok(RawBundle(Raw::capture(r, start)))
    }
}

impl WriteParcelable for RawBundle {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write(p);
    }
}

/// A bundle kept past its call: the binders among it held and the files
/// of its fds kept.
#[derive(Clone)]
pub struct KeptBundle {
    pub bundle: RawBundle,
    _binders: std::sync::Arc<Vec<Strong>>,
}

impl KeptBundle {
    pub fn new(mut bundle: RawBundle, process: &LocalProcess) -> Result<KeptBundle> {
        bundle.0.keep_files(&|fd| process.file(fd))?;
        let binders = bundle.0.handles().map(|h| process.strong(h)).collect();
        Ok(KeptBundle { bundle, _binders: std::sync::Arc::new(binders) })
    }
}

/// A parcelable written by reference.
pub struct By<'a, T>(pub &'a T);

impl<T: WriteParcelable> WriteParcelable for By<'_, T> {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write_to(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An intent with an action, a data URI and an extra, as
    /// `Intent.writeToParcel` writes it.
    fn intent() -> Parcel {
        let mut p = Parcel::new();
        p.write_string8(Some("a.ACTION"));
        p.write_i32(1); // `Uri.StringUri`
        p.write_string8(Some("content://x/y"));
        p.write_string8(None); // type
        p.write_string8(None); // identifier
        p.write_i32(0x10); // flags
        p.write_i32(0); // extended flags
        p.write_string8(Some("p")); // package
        p.write_string16(None); // component
        p.write_i32(0); // source bounds
        p.write_i32(0); // categories
        p.write_i32(0); // selector
        p.write_i32(0); // clip data
        p.write_i32(-2); // content user hint
        crate::bundle::write(&mut p, &[("k", crate::bundle::Value::Int(7))]);
        p.write_i32(0); // original intent
        p.write_i32(0); // creator token
        p
    }

    #[test]
    fn an_intent_is_passed_on_as_it_came_with_new_flags() {
        let sent = intent();
        let mut r = Reader::new(sent.data(), &[]);
        let mut kept = RawIntent::read_from(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(kept.intent.action.as_deref(), Some("a.ACTION"));
        assert_eq!(kept.intent.package.as_deref(), Some("p"));
        let mut again = Parcel::new();
        kept.write_to(&mut again);
        assert_eq!(again.data(), sent.data());
        kept.set_flags(0x30, 1);
        let mut flagged = Parcel::new();
        kept.write_to(&mut flagged);
        let read = RawIntent::read_from(&mut Reader::new(flagged.data(), &[])).unwrap();
        assert_eq!((read.intent.flags, read.intent.extended_flags), (0x30, 1));
    }
}
