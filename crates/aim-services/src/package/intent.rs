//! `android.content.Intent` as resolution reads it, from its parcel
//! (`Intent.readFromParcel` at `android-16.0.0_r1`, where
//! `prevent_intent_redirect` is on): what it names and matches by, not
//! its clip data or extras.

use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};
use aim_service_aidl::ReadParcelable;

use super::intent_filter::{ACTION_VIEW, Strings};
use super::uri::Uri;
use crate::clip::{ClipData, bundle};

pub const FLAG_DEBUG_LOG_RESOLUTION: i32 = 0x0000_0008;
pub const FLAG_EXCLUDE_STOPPED_PACKAGES: i32 = 0x0000_0010;
pub const FLAG_INCLUDE_STOPPED_PACKAGES: i32 = 0x0000_0020;
pub const FLAG_ACTIVITY_REQUIRE_NON_BROWSER: i32 = 0x0000_0400;
pub const FLAG_ACTIVITY_MATCH_EXTERNAL: i32 = 0x0000_0800;
pub const FLAG_IGNORE_EPHEMERAL: i32 = 0x8000_0000_u32 as i32;

/// `MediaStore`'s capture actions (`Intent.isImageCaptureIntent`).
const IMAGE_CAPTURE_ACTIONS: [&str; 5] = [
    "android.media.action.IMAGE_CAPTURE",
    "android.media.action.IMAGE_CAPTURE_SECURE",
    "android.provider.action.MOTION_PHOTO_CAPTURE",
    "android.provider.action.MOTION_PHOTO_CAPTURE_SECURE",
    "android.media.action.VIDEO_CAPTURE",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentName {
    pub package: String,
    pub class: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Intent {
    pub action: Option<String>,
    pub data: Option<Uri>,
    /// The type set on the intent (`getType`), not the resolved one.
    pub ty: Option<String>,
    pub identifier: Option<String>,
    pub flags: i32,
    pub extended_flags: i32,
    pub package: Option<String>,
    pub component: Option<ComponentName>,
    /// An `ArraySet`; `None` when empty.
    pub categories: Option<Vec<String>>,
    pub selector: Option<Box<Intent>>,
}

impl Intent {
    /// `Intent(Parcel)`; the strings through `strings` (an intent of a
    /// package's `<queries>` is in the parser cache's string pool).
    pub fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<Intent> {
        let mut i = Intent {
            action: strings.string8(r)?,
            data: Uri::read(r, strings)?,
            ty: strings.string8(r)?,
            identifier: strings.string8(r)?,
            flags: r.read_i32()?,
            extended_flags: r.read_i32()?,
            package: strings.string8(r)?,
            ..Intent::default()
        };
        if let Some(package) = strings.string16(r)? {
            let class = strings.string16(r)?.ok_or(BAD_VALUE)?;
            i.component = Some(ComponentName { package, class });
        }
        if r.read_i32()? != 0 {
            for _ in 0..4 {
                r.read_i32()?; // source bounds
            }
        }
        let n = r.read_i32()?;
        if n > 0 {
            let mut categories: Vec<String> = Vec::new();
            for _ in 0..n {
                let c = strings.string8(r)?.ok_or(BAD_VALUE)?;
                if !categories.contains(&c) {
                    categories.push(c);
                }
            }
            i.categories = Some(categories);
        }
        if r.read_i32()? != 0 {
            i.selector = Some(Box::new(Intent::read(r, strings)?));
        }
        if r.read_i32()? != 0 {
            ClipData::read_from(r)?;
        }
        r.read_i32()?; // content user hint
        bundle(r)?; // extras
        if r.read_i32()? != 0 {
            Intent::read(r, strings)?; // the original intent
        }
        if r.read_i32()? != 0 {
            // The creator token and its nested intents' keys.
            r.read_binder()?;
            for _ in 0..r.read_i32()?.max(0) {
                r.read_i32()?;
                strings.string8(r)?;
                r.read_i32()?;
            }
        }
        Ok(i)
    }

    pub fn scheme(&self) -> Option<&str> {
        self.data.as_ref().and_then(Uri::scheme)
    }

    pub fn has_flag(&self, flag: i32) -> bool {
        self.flags & flag != 0
    }

    /// `isExcludingStopped`.
    pub fn is_excluding_stopped(&self) -> bool {
        self.flags & (FLAG_EXCLUDE_STOPPED_PACKAGES | FLAG_INCLUDE_STOPPED_PACKAGES)
            == FLAG_EXCLUDE_STOPPED_PACKAGES
    }

    /// `hasWebURI`: http or https data.
    pub fn has_web_uri(&self) -> bool {
        self.data.is_some() && matches!(self.scheme(), Some("http" | "https"))
    }

    /// `isWebIntent`.
    pub fn is_web_intent(&self) -> bool {
        self.action.as_deref() == Some(ACTION_VIEW) && self.has_web_uri()
    }

    /// `isImplicitImageCaptureIntent`.
    pub fn is_implicit_image_capture_intent(&self) -> bool {
        self.package.is_none()
            && self.component.is_none()
            && self
                .action
                .as_deref()
                .is_some_and(|a| IMAGE_CAPTURE_ACTIONS.contains(&a))
    }
}

#[cfg(test)]
mod tests {
    use aim_binder_host::parcel::Parcel;

    use super::super::intent_filter::Plain;
    use super::*;

    /// An intent as `Intent.writeToParcel` writes `new Intent(VIEW,
    /// Uri.parse("https://example.com/a")).addCategory(BROWSABLE)
    /// .setPackage("p").setSelector(new Intent("s"))`.
    fn view() -> Parcel {
        let mut p = Parcel::new();
        let plain = |p: &mut Parcel, action: &str, data: Option<&str>| {
            p.write_string8(Some(action));
            match data {
                Some(d) => {
                    p.write_i32(1);
                    p.write_string8(Some(d));
                }
                None => p.write_i32(0),
            }
            p.write_string8(None); // type
            p.write_string8(None); // identifier
            p.write_i32(0x10); // flags
            p.write_i32(0);
        };
        plain(&mut p, ACTION_VIEW, Some("https://example.com/a"));
        p.write_string8(Some("p"));
        p.write_string16(None); // component
        p.write_i32(0); // source bounds
        p.write_i32(2);
        p.write_string8(Some("android.intent.category.BROWSABLE"));
        p.write_string8(Some("android.intent.category.BROWSABLE"));
        p.write_i32(1); // selector
        plain(&mut p, "s", None);
        p.write_string8(None);
        p.write_string16(Some("q"));
        p.write_string16(Some("q.C"));
        for _ in 0..3 {
            p.write_i32(0); // source bounds, categories, selector
        }
        p.write_i32(0); // clip data
        p.write_i32(-2); // content user hint
        p.write_i32(-1); // extras
        p.write_i32(0); // original intent
        p.write_i32(0); // creator token
        for v in [0, -2, -1, 0, 0] {
            p.write_i32(v); // the outer intent's clip data .. creator token
        }
        p
    }

    #[test]
    fn reads_a_parcel() {
        let p = view();
        let mut r = Reader::new(p.data(), p.objects());
        let i = Intent::read(&mut r, &mut Plain).unwrap();
        assert_eq!(r.remaining(), 0);
        assert!(i.is_web_intent() && i.is_excluding_stopped());
        assert_eq!(i.categories.as_ref().unwrap().len(), 1);
        assert_eq!(i.package.as_deref(), Some("p"));
        let s = i.selector.unwrap();
        assert_eq!(s.component.as_ref().unwrap().class, "q.C");
        assert!(!s.has_web_uri());
    }

    #[test]
    fn image_capture() {
        let mut i = Intent {
            action: Some("android.media.action.IMAGE_CAPTURE".into()),
            ..Intent::default()
        };
        assert!(i.is_implicit_image_capture_intent());
        i.package = Some("p".into());
        assert!(!i.is_implicit_image_capture_intent());
    }
}
