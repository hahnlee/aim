//! `ClipData` and `ClipDescription` in their Java wire format
//! (`ClipData.writeToParcel` and what it writes, at the pinned tag).
//!
//! The service keeps what it needs parsed (the description, each item's
//! text and URIs) and every item as the bytes it arrived as, which it
//! sends back unchanged: its binders (an `IntentSender`, an intent's
//! creator token) are recorded so that the service can hold them.
//!
//! Not parsed, so refused with `BAD_VALUE`: a clip icon (`Bitmap`), an
//! item's `ActivityInfo` or `TextLinks` (both set only inside
//! system_server), and the text spans whose parcel form is not a fixed
//! list of ints and strings (typeface, text appearance, suggestion, easy
//! edit, locale, TTS, accessibility replacement, line break config and
//! the writing tools span) (#433).

use aim_binder_driver::uapi::{BINDER_TYPE_HANDLE, FlatBinderObject};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

/// `ClipDescription.MIMETYPE_TEXT_PLAIN`.
pub const MIMETYPE_TEXT_PLAIN: &str = "text/plain";
/// `ClipDescription.CLASSIFICATION_NOT_PERFORMED`.
const CLASSIFICATION_NOT_PERFORMED: i32 = 2;
/// `BaseBundle.BUNDLE_MAGIC` and `BUNDLE_MAGIC_NATIVE`.
const BUNDLE_MAGIC: i32 = 0x4C44_4E42;
const BUNDLE_MAGIC_NATIVE: i32 = 0x4C44_4E44;
/// `Parcel.VAL_BOOLEAN`.
const VAL_BOOLEAN: i32 = 9;
/// What EmulatorClipboardMonitor puts in a host clip's extras, so that
/// SystemUI shows no clipboard overlay for it.
const SUPPRESS_CLIPBOARD_OVERLAY: &str = "com.android.systemui.SUPPRESS_CLIPBOARD_OVERLAY";

/// Bytes of a received parcel, with the offsets of their binder objects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    bytes: Vec<u8>,
    objects: Vec<u64>,
}

impl Raw {
    fn capture(r: &Reader<'_>, start: usize) -> Raw {
        let (bytes, objects) = r.since(start);
        Raw {
            bytes: bytes.to_vec(),
            objects,
        }
    }

    fn write(&self, p: &mut Parcel) {
        p.write_raw(&self.bytes, &self.objects);
    }

    /// The handles among its binders.
    fn handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.objects.iter().filter_map(|&at| {
            let at = at as usize;
            let object = FlatBinderObject::decode(&self.bytes[at..at + 24]);
            (object.kind == BINDER_TYPE_HANDLE).then(|| object.handle())
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipDescription {
    label: Raw,
    pub mime_types: Option<Vec<Option<String>>>,
    extras: Raw,
    pub timestamp: i64,
    is_styled_text: bool,
    classification_status: i32,
    confidences: Raw,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The item's text (`getText().toString()`).
    pub text: Option<String>,
    /// Its URI and its intent's data URI.
    pub uris: Vec<String>,
    raw: Raw,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipData {
    pub description: ClipDescription,
    pub items: Vec<Item>,
}

impl ClipData {
    /// A clip of the Mac's text, as EmulatorClipboardMonitor makes the
    /// host's: labelled "host clipboard", with no clipboard overlay.
    pub fn host_text(text: &str, timestamp: i64) -> ClipData {
        let mut label = Parcel::new();
        write_char_sequence(&mut label, Some("host clipboard"));
        let mut extras = Parcel::new();
        let length_at = extras.position();
        extras.write_i32(0);
        extras.write_i32(BUNDLE_MAGIC);
        let start = extras.position();
        extras.write_i32(1);
        extras.write_string16(Some(SUPPRESS_CLIPBOARD_OVERLAY));
        extras.write_i32(VAL_BOOLEAN);
        extras.write_bool(true);
        let length = (extras.position() - start) as i32;
        extras.set_i32_at(length_at, length);
        extras.write_bool(false); // has an intent
        let mut empty = Parcel::new();
        empty.write_i32(0);
        let mut item = Parcel::new();
        write_char_sequence(&mut item, Some(text));
        item.write_string8(None); // html
        for _ in 0..5 {
            item.write_i32(0); // intent, sender, uri, activity info, links
        }
        let raw = |p: &Parcel| Raw {
            bytes: p.data().to_vec(),
            objects: Vec::new(),
        };
        ClipData {
            description: ClipDescription {
                label: raw(&label),
                mime_types: Some(vec![Some(MIMETYPE_TEXT_PLAIN.into())]),
                extras: raw(&extras),
                timestamp,
                is_styled_text: false,
                classification_status: CLASSIFICATION_NOT_PERFORMED,
                confidences: raw(&empty),
            },
            items: vec![Item {
                text: Some(text.into()),
                uris: Vec::new(),
                raw: raw(&item),
            }],
        }
    }

    /// The handles of the binders it carries, which its holder must keep.
    pub fn handles(&self) -> Vec<u32> {
        self.items.iter().flat_map(|i| i.raw.handles()).collect()
    }
}

impl ClipDescription {
    /// Records that the text was not classified.
    pub fn set_not_classified(&mut self) {
        self.classification_status = CLASSIFICATION_NOT_PERFORMED;
    }
}

impl ReadParcelable for ClipDescription {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.position();
        char_sequence(r)?;
        let label = Raw::capture(r, start);
        let mime_types = aim_service_aidl::read_string_list(r)?;
        let start = r.position();
        bundle(r)?;
        let extras = Raw::capture(r, start);
        let timestamp = r.read_i64()?;
        let is_styled_text = r.read_bool()?;
        let classification_status = r.read_i32()?;
        let start = r.position();
        bundle(r)?;
        Ok(ClipDescription {
            label,
            mime_types,
            extras,
            timestamp,
            is_styled_text,
            classification_status,
            confidences: Raw::capture(r, start),
        })
    }
}

impl WriteParcelable for ClipDescription {
    fn write_to(&self, p: &mut Parcel) {
        self.label.write(p);
        aim_service_aidl::write_string_list(p, self.mime_types.as_deref());
        self.extras.write(p);
        p.write_i64(self.timestamp);
        p.write_bool(self.is_styled_text);
        p.write_i32(self.classification_status);
        self.confidences.write(p);
    }
}

impl ReadParcelable for ClipData {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let description = ClipDescription::read_from(r)?;
        if r.read_i32()? != 0 {
            return Err(BAD_VALUE); // an icon
        }
        let count = r.read_i32()?;
        let mut items = Vec::new();
        for _ in 0..count.max(0) {
            let start = r.position();
            let text = char_sequence(r)?;
            r.read_string8()?; // html
            let mut uris = Vec::new();
            if r.read_i32()? != 0 {
                uris.extend(intent(r)?);
            }
            if r.read_i32()? != 0 {
                r.read_binder()?; // IntentSender
            }
            if r.read_i32()? != 0 {
                uris.extend(uri(r)?);
            }
            if r.read_i32()? != 0 || r.read_i32()? != 0 {
                return Err(BAD_VALUE); // ActivityInfo, TextLinks
            }
            items.push(Item {
                text,
                uris,
                raw: Raw::capture(r, start),
            });
        }
        Ok(ClipData { description, items })
    }
}

impl WriteParcelable for ClipData {
    fn write_to(&self, p: &mut Parcel) {
        self.description.write_to(p);
        p.write_i32(0); // icon
        p.write_i32(self.items.len() as i32);
        for item in &self.items {
            item.raw.write(p);
        }
    }
}

/// `TextUtils.writeToParcel` of a plain string.
fn write_char_sequence(p: &mut Parcel, text: Option<&str>) {
    p.write_i32(1);
    p.write_string8(text);
}

/// `TextUtils.CHAR_SEQUENCE_CREATOR`: the text, past its spans.
fn char_sequence(r: &mut Reader<'_>) -> Result<Option<String>> {
    let kind = r.read_i32()?;
    let text = r.read_string8()?;
    if text.is_none() || kind == 1 {
        return Ok(text);
    }
    loop {
        let span = r.read_i32()?;
        if span == 0 {
            return Ok(text);
        }
        // Each span's writeToParcelInternal: i = int (or float), s = String.
        let layout = match span {
            5 | 6 | 14 | 15 => "",
            2 | 3 | 4 | 12 | 20 | 21 | 25 | 27 | 28 => "i",
            7 | 10 | 16 => "ii",
            9 => "iii",
            8 => "iiii",
            1 | 11 => "s",
            18 => "ss",
            26 => "si",
            _ => return Err(BAD_VALUE),
        };
        for field in layout.chars() {
            match field {
                'i' => {
                    r.read_i32()?;
                }
                _ => {
                    r.read_string16()?;
                }
            }
        }
        for _ in 0..3 {
            r.read_i32()?; // start, end, flags
        }
    }
}

/// `Uri.CREATOR`: the URI string, or none for null.
fn uri(r: &mut Reader<'_>) -> Result<Option<String>> {
    match r.read_i32()? {
        0 => Ok(None),
        1..=3 => r.read_string8(),
        _ => Err(BAD_VALUE),
    }
}

/// `BaseBundle.readFromParcelInner`, skipped: the length, and unless
/// empty, the magic, that many bytes and whether it holds an intent.
fn bundle(r: &mut Reader<'_>) -> Result<()> {
    let length = r.read_i32()?;
    if length <= 0 {
        return Ok(());
    }
    match r.read_i32()? {
        BUNDLE_MAGIC | BUNDLE_MAGIC_NATIVE => {
            r.skip(length as usize)?;
            r.read_bool().map(drop)
        }
        _ => Err(BAD_VALUE),
    }
}

/// `Intent(Parcel)`: its data URIs (its own, its selector's, its clip's
/// and its original intent's).
fn intent(r: &mut Reader<'_>) -> Result<Vec<String>> {
    let mut uris = Vec::new();
    r.read_string8()?; // action
    uris.extend(uri(r)?);
    r.read_string8()?; // type
    r.read_string8()?; // identifier
    r.read_i32()?; // flags
    r.read_i32()?; // extended flags
    r.read_string8()?; // package
    if r.read_string16()?.is_some() {
        r.read_string16()?; // component class
    }
    if r.read_i32()? != 0 {
        for _ in 0..4 {
            r.read_i32()?; // source bounds
        }
    }
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string8()?; // category
    }
    if r.read_i32()? != 0 {
        uris.extend(intent(r)?); // selector
    }
    if r.read_i32()? != 0 {
        let clip = ClipData::read_from(r)?;
        uris.extend(clip.items.into_iter().flat_map(|i| i.uris));
    }
    r.read_i32()?; // content user hint
    bundle(r)?; // extras
    if r.read_i32()? != 0 {
        uris.extend(intent(r)?); // original intent
    }
    // The creator token (android.security.prevent_intent_redirect, which
    // is enabled and read-only in the pinned image).
    if r.read_i32()? != 0 {
        r.read_binder()?;
        for _ in 0..r.read_i32()?.max(0) {
            r.read_i32()?; // nested intent key: type, key, index
            r.read_string8()?;
            r.read_i32()?;
        }
    }
    Ok(uris)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clip as `ClipData.writeToParcel` writes
    /// `ClipData.newRawUri(label, uri)` with a text item added.
    fn java_clip() -> Parcel {
        let mut p = Parcel::new();
        // description: label (spanned, one StyleSpan), mime types, extras,
        // timestamp, styled, classification status, confidences
        p.write_i32(0);
        p.write_string8(Some("label"));
        p.write_i32(7);
        p.write_i32(1);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(5);
        p.write_i32(0x11);
        p.write_i32(0);
        aim_service_aidl::write_string_list(
            &mut p,
            Some(&[Some("text/uri-list".into()), Some("text/plain".into())]),
        );
        p.write_i32(8); // extras: one boolean-free entry of 8 bytes
        p.write_i32(BUNDLE_MAGIC);
        p.write_i32(0);
        p.write_i32(0);
        p.write_bool(false);
        p.write_i64(7);
        p.write_bool(true);
        p.write_i32(1);
        p.write_i32(0);
        p.write_i32(0); // icon
        p.write_i32(2);
        // item: uri only
        p.write_i32(1);
        p.write_string8(None);
        p.write_string8(None);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(1);
        p.write_i32(3);
        p.write_string8(Some("content://media/1"));
        p.write_i32(0);
        p.write_i32(0);
        // item: text with an intent
        p.write_i32(1);
        p.write_string8(Some("héllo"));
        p.write_string8(Some("<b>h</b>"));
        p.write_i32(1);
        p.write_string8(Some("android.intent.action.VIEW"));
        p.write_i32(1);
        p.write_string8(Some("https://example.com"));
        p.write_string8(None);
        p.write_string8(None);
        p.write_i32(0);
        p.write_i32(0);
        p.write_string8(None);
        p.write_string16(Some("pkg"));
        p.write_string16(Some("pkg.Cls"));
        p.write_i32(0);
        p.write_i32(1);
        p.write_string8(Some("android.intent.category.DEFAULT"));
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(-2);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(0); // no creator token
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(0);
        p
    }

    #[test]
    fn reads_and_writes_back_a_java_clip() {
        let p = java_clip();
        let mut r = Reader::new(p.data(), p.objects());
        let clip = ClipData::read_from(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(clip.description.timestamp, 7);
        assert_eq!(clip.items.len(), 2);
        assert_eq!(clip.items[0].uris, ["content://media/1"]);
        assert_eq!(clip.items[1].text.as_deref(), Some("héllo"));
        assert_eq!(clip.items[1].uris, ["https://example.com"]);
        let mut out = Parcel::new();
        clip.write_to(&mut out);
        assert_eq!(out.data(), p.data());
    }

    #[test]
    fn refuses_what_it_cannot_parse() {
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_string8(Some("x"));
        p.write_i32(17); // TextAppearanceSpan
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(char_sequence(&mut r), Err(BAD_VALUE));
    }

    #[test]
    fn host_clip_round_trips() {
        let clip = ClipData::host_text("from the Mac", 42);
        let mut p = Parcel::new();
        clip.write_to(&mut p);
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(ClipData::read_from(&mut r).unwrap(), clip);
    }
}
