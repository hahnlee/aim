//! The parcels of the status bar's calls, read as their `CREATOR`s read
//! them and written as their `writeToParcel`s write them (pinned
//! `android-16.0.0_r1`).

use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use crate::clip::char_sequence;
use crate::notifications::parcels::{Files, Icon, bitmap, icon, skip_value};

/// A call's blobs, not read: its bitmaps are skipped.
struct NoFiles;

impl Files for NoFiles {
    fn read(&self, _: u32, _: usize) -> Option<Vec<u8>> {
        None
    }
}

/// A `CharSequence`, as its text.
pub struct Text(pub Option<String>);

impl ReadParcelable for Text {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        char_sequence(r).map(Text)
    }
}

/// A `ComponentName`, skipped.
pub struct Component;

impl ReadParcelable for Component {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        r.read_string16()?;
        r.read_string16()?;
        Ok(Component)
    }
}

/// An `Icon`, skipped.
pub struct Skipped;

impl ReadParcelable for Skipped {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        icon(r, &NoFiles).map(|_| Skipped)
    }
}

/// `ActivityTaskManager.RootTaskInfo`, up to its task id.
pub struct RootTask {
    pub id: i32,
}

impl ReadParcelable for RootTask {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let rect = |r: &mut Reader<'_>| -> Result<()> {
            if r.read_i32()? != 0 {
                (0..4).try_for_each(|_| r.read_i32().map(drop))?;
            }
            Ok(())
        };
        rect(r)?; // bounds
        aim_service_aidl::read_int_array(r)?; // child task ids
        aim_service_aidl::read_string_list(r)?; // child task names
        for _ in 0..r.read_i32()?.max(0) {
            rect(r)?; // child task bounds
        }
        aim_service_aidl::read_int_array(r)?; // child task user ids
        r.read_i32()?; // visible
        r.read_i32()?; // position
        r.read_i32()?; // TaskInfo.userId
        Ok(RootTask { id: r.read_i32()? })
    }
}

/// `writeParcelable`'s class name, then the object; `None` for null.
fn parcelable<T>(
    r: &mut Reader<'_>,
    f: impl FnOnce(&mut Reader<'_>) -> Result<T>,
) -> Result<Option<T>> {
    match r.read_string16()? {
        Some(_) => f(r).map(Some),
        None => Ok(None),
    }
}

/// `StatusBarIcon(Parcel)`: what the Mac shows of it.
pub struct StatusBarIcon {
    pub icon: Option<Icon>,
    /// The package whose resources the icon is of.
    pub package: String,
    pub user: i32,
    pub level: i32,
    pub visible: bool,
    pub number: i32,
    pub description: Option<String>,
}

impl StatusBarIcon {
    pub fn read(r: &mut Reader<'_>, files: &dyn Files) -> Result<Self> {
        let icon = parcelable(r, |r| icon(r, files))?;
        let package = r.read_string16()?.unwrap_or_default();
        let user = parcelable(r, |r| r.read_i32())?.unwrap_or(0);
        let level = r.read_i32()?;
        let visible = r.read_i32()? != 0;
        let number = r.read_i32()?;
        let description = char_sequence(r)?;
        r.read_string16()?; // type
        r.read_string16()?; // shape
        Ok(Self {
            icon,
            package,
            user,
            level,
            visible,
            number,
            description,
        })
    }
}

/// `RegisterStatusBarResult`: the icons set before the bar registered,
/// by slot.
pub struct Registered(pub Vec<(String, StatusBarIcon)>);

impl ReadParcelable for Registered {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        // createTypedArrayMap: the size (-1: null), then keys and typed
        // values; the rest of the result is the shade's.
        let n = r.read_i32()?;
        let mut icons = Vec::new();
        for _ in 0..n.max(0) {
            let slot = r.read_string16()?.unwrap_or_default();
            if r.read_i32()? != 0 {
                icons.push((slot, StatusBarIcon::read(r, &NoFiles)?));
            }
        }
        Ok(Registered(icons))
    }
}

/// `PromptInfo(Parcel)`: the texts of the request.
#[derive(Default)]
pub struct PromptInfo {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub credential_title: Option<String>,
    pub credential_subtitle: Option<String>,
    pub credential_description: Option<String>,
}

/// `PromptContentView`'s implementations, skipped.
fn content_view(r: &mut Reader<'_>, class: &str) -> Result<()> {
    match class {
        "android.hardware.biometrics.PromptVerticalListContentView" => {
            // writeList of items, then the description.
            for _ in 0..r.read_i32()?.max(0) {
                skip_value(r)?;
            }
            r.read_string16()?;
        }
        "android.hardware.biometrics.PromptContentViewWithMoreOptionsButton" => {
            r.read_string16()?;
        }
        _ => return Err(BAD_VALUE),
    }
    Ok(())
}

impl ReadParcelable for PromptInfo {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        r.read_i32()?; // logo resource
        if r.read_i32()? != 0 {
            bitmap(r, &NoFiles)?; // logo
        }
        r.read_string16()?; // logo description
        let title = char_sequence(r)?;
        r.read_bool()?; // use default title
        let subtitle = char_sequence(r)?;
        r.read_bool()?; // use default subtitle
        let description = char_sequence(r)?;
        if let Some(class) = r.read_string16()? {
            content_view(r, &class)?;
        }
        let info = Self {
            title,
            subtitle,
            description,
            credential_title: char_sequence(r)?,
            credential_subtitle: char_sequence(r)?,
            credential_description: char_sequence(r)?,
        };
        // Read to its end: the call's arguments follow.
        char_sequence(r)?; // negative button
        for _ in 0..2 {
            r.read_bool()?; // confirmation requested, credential allowed
        }
        r.read_i32()?; // authenticators
        for _ in 0..2 {
            r.read_bool()?; // biometrics disallowed by policy, system events
        }
        for _ in 0..r.read_i32()?.max(0) {
            skip_value(r)?; // allowed sensor ids
        }
        for _ in 0..5 {
            r.read_bool()?; // background, enrollment, legacy, emergency, parent profile
        }
        if r.read_string16()?.is_some() {
            Component::read_from(r)?; // the real caller of ConfirmDeviceCredentialActivity
        }
        r.read_string16()?; // its class name
        Ok(info)
    }
}

/// `LockscreenCredential`: its type (a `CREDENTIAL_TYPE_*`) and bytes.
pub struct LockscreenCredential {
    pub kind: i32,
    pub bytes: Vec<u8>,
    /// Characters outside printable ASCII, which `LockscreenCredential`
    /// keeps apart.
    pub invalid_chars: bool,
}

impl LockscreenCredential {
    /// The credential of `kind` the user entered as `text`
    /// (`charsToBytesTruncating` of its UTF-16 units).
    pub fn new(kind: i32, text: &str) -> Self {
        let units: Vec<u16> = text.encode_utf16().collect();
        Self {
            kind,
            bytes: units.iter().map(|&u| u as u8).collect(),
            invalid_chars: units.iter().any(|&u| !(32..=127).contains(&u)),
        }
    }
}

impl WriteParcelable for LockscreenCredential {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.kind);
        aim_service_aidl::write_byte_array(p, Some(&self.bytes));
        p.write_bool(self.invalid_chars);
    }
}

/// `VerifyCredentialResponse`.
pub struct VerifyCredentialResponse {
    pub code: i32,
    pub timeout_ms: i32,
    pub hat: Option<Vec<u8>>,
    pub password_handle: i64,
}

impl ReadParcelable for VerifyCredentialResponse {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            code: r.read_i32()?,
            timeout_ms: r.read_i32()?,
            hat: aim_service_aidl::read_byte_array(r)?,
            password_handle: r.read_i64()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::write_char_sequence;

    #[test]
    fn reads_a_prompt_with_a_list() {
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_i32(0); // no logo
        p.write_string16(None);
        write_char_sequence(&mut p, Some("Title"));
        p.write_bool(false);
        write_char_sequence(&mut p, None);
        p.write_bool(false);
        write_char_sequence(&mut p, Some("Why"));
        p.write_string16(Some(
            "android.hardware.biometrics.PromptVerticalListContentView",
        ));
        p.write_i32(1);
        p.write_i32(0); // VAL_STRING
        p.write_string16(Some("item"));
        p.write_string16(Some("list"));
        write_char_sequence(&mut p, Some("PIN"));
        write_char_sequence(&mut p, None);
        write_char_sequence(&mut p, None);
        write_char_sequence(&mut p, Some("Cancel"));
        p.write_bool(false);
        p.write_bool(true);
        p.write_i32(1 << 15); // DEVICE_CREDENTIAL
        p.write_bool(false);
        p.write_bool(false);
        p.write_i32(1);
        p.write_i32(1); // VAL_INTEGER
        p.write_i32(7);
        for _ in 0..5 {
            p.write_bool(false);
        }
        p.write_string16(Some("android.content.ComponentName"));
        p.write_string16(Some("com.android.settings"));
        p.write_string16(Some("com.android.settings.Caller"));
        p.write_string16(None);
        p.write_i32(0x5eed); // what follows it
        let mut r = Reader::new(p.data(), p.objects());
        let info = PromptInfo::read_from(&mut r).unwrap();
        assert_eq!(r.read_i32().unwrap(), 0x5eed);
        assert_eq!(info.title.as_deref(), Some("Title"));
        assert_eq!(info.subtitle, None);
        assert_eq!(info.description.as_deref(), Some("Why"));
        assert_eq!(info.credential_title.as_deref(), Some("PIN"));
    }

    #[test]
    fn keeps_credentials_as_android_does() {
        let c = LockscreenCredential::new(3, "1234");
        assert_eq!((c.bytes.as_slice(), c.invalid_chars), (&b"1234"[..], false));
        assert!(LockscreenCredential::new(4, "pässword").invalid_chars);
    }
}
