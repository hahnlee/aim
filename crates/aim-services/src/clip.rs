//! `ClipData` and `ClipDescription` in their Java wire format
//! (`ClipData.writeToParcel` and what it writes, at the pinned tag).
//!
//! The service keeps what it needs parsed (the description, each item's
//! text, URI and intent data) and every item as the bytes it arrived as,
//! which it sends back unchanged: its binders (an `IntentSender`, an
//! intent's creator token, a `PendingIntent` in a span) are recorded so
//! that the service can hold them, and so are the files of its fd objects
//! (in bundles).
//!
//! Not parsed, so refused with `BAD_VALUE`: a clip icon (`Bitmap`) and an
//! item's `ActivityInfo`. No API gives an app's clip either (only
//! unparcelling sets the icon; only `copyForTransferWithActivityInfo`,
//! hidden and for drag and drop, parcels the activity info, and the
//! original drops it again when it hands the clip out), and skipping them
//! means reading a whole bitmap or `ApplicationInfo`.

use aim_binder_driver::File;
use aim_binder_driver::uapi::{BINDER_TYPE_FD, BINDER_TYPE_HANDLE, FlatBinderObject};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use crate::bundle::{self as bundles, Value};

/// `ClipDescription.MIMETYPE_TEXT_PLAIN`.
pub const MIMETYPE_TEXT_PLAIN: &str = "text/plain";
/// `ClipDescription.CLASSIFICATION_NOT_PERFORMED`.
const CLASSIFICATION_NOT_PERFORMED: i32 = 2;
/// What EmulatorClipboardMonitor puts in a host clip's extras, so that
/// SystemUI shows no clipboard overlay for it.
const SUPPRESS_CLIPBOARD_OVERLAY: &str = "com.android.systemui.SUPPRESS_CLIPBOARD_OVERLAY";

/// Bytes of a received parcel, with the offsets of their binder objects
/// and the files of its fd objects.
#[derive(Clone, Default)]
pub struct Raw {
    bytes: Vec<u8>,
    objects: Vec<u64>,
    files: Vec<(u64, File)>,
}

impl std::fmt::Debug for Raw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Raw")
            .field("bytes", &self.bytes.len())
            .field("objects", &self.objects)
            .finish()
    }
}

impl PartialEq for Raw {
    fn eq(&self, other: &Raw) -> bool {
        self.bytes == other.bytes && self.objects == other.objects
    }
}

impl Raw {
    fn capture(r: &Reader<'_>, start: usize) -> Raw {
        let (bytes, objects) = r.since(start);
        Raw {
            bytes: bytes.to_vec(),
            objects,
            files: Vec::new(),
        }
    }

    fn of(p: &Parcel) -> Raw {
        Raw {
            bytes: p.data().to_vec(),
            ..Raw::default()
        }
    }

    fn write(&self, p: &mut Parcel) {
        p.write_raw_files(&self.bytes, &self.objects, &self.files);
    }

    fn object(&self, at: u64) -> FlatBinderObject {
        let at = at as usize;
        FlatBinderObject::decode(&self.bytes[at..at + 24])
    }

    /// The handles among its binders.
    fn handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.objects.iter().filter_map(|&at| {
            let object = self.object(at);
            (object.kind == BINDER_TYPE_HANDLE).then(|| object.handle())
        })
    }

    /// Takes the file of each of its fd objects, which the call that
    /// brought it is about to close.
    fn keep_files(&mut self, file: &dyn Fn(u32) -> Option<File>) -> Result<()> {
        for &at in &self.objects {
            let object = self.object(at);
            if object.kind == BINDER_TYPE_FD {
                self.files
                    .push((at, file(object.binder as u32).ok_or(BAD_VALUE)?));
            }
        }
        Ok(())
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
    /// Its URI (`getUri()`).
    pub uri: Option<String>,
    /// Its intent's data URI (`getIntent().getData()`).
    pub intent_data: Option<String>,
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
        bundles::write(
            &mut extras,
            &[(SUPPRESS_CLIPBOARD_OVERLAY, Value::Bool(true))],
        );
        let mut empty = Parcel::new();
        bundles::write(&mut empty, &[]);
        let mut item = Parcel::new();
        write_char_sequence(&mut item, Some(text));
        item.write_string8(None); // html
        for _ in 0..5 {
            item.write_i32(0); // intent, sender, uri, activity info, links
        }
        ClipData {
            description: ClipDescription {
                label: Raw::of(&label),
                mime_types: Some(vec![Some(MIMETYPE_TEXT_PLAIN.into())]),
                extras: Raw::of(&extras),
                timestamp,
                is_styled_text: false,
                classification_status: CLASSIFICATION_NOT_PERFORMED,
                confidences: Raw::of(&empty),
            },
            items: vec![Item {
                text: Some(text.into()),
                uri: None,
                intent_data: None,
                raw: Raw::of(&item),
            }],
        }
    }

    fn raws(&mut self) -> impl Iterator<Item = &mut Raw> {
        let d = &mut self.description;
        [&mut d.label, &mut d.extras, &mut d.confidences]
            .into_iter()
            .chain(self.items.iter_mut().map(|i| &mut i.raw))
    }

    /// The handles of the binders it carries, which its holder must keep.
    pub fn handles(&mut self) -> Vec<u32> {
        self.raws()
            .flat_map(|r| r.handles().collect::<Vec<_>>())
            .collect()
    }

    /// Keeps the files of its fd objects past the call that brought it
    /// (`file` is the calling process's file behind an fd).
    pub fn keep_files(&mut self, file: &dyn Fn(u32) -> Option<File>) -> Result<()> {
        self.raws().try_for_each(|r| r.keep_files(file))
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
            let mut intent_data = None;
            if r.read_i32()? != 0 {
                intent_data = intent(r)?;
            }
            if r.read_i32()? != 0 {
                r.read_binder()?; // IntentSender
            }
            let mut item_uri = None;
            if r.read_i32()? != 0 {
                item_uri = uri(r)?;
            }
            if r.read_i32()? != 0 {
                return Err(BAD_VALUE); // ActivityInfo
            }
            if r.read_i32()? != 0 {
                text_links(r)?;
            }
            items.push(Item {
                text,
                uri: item_uri,
                intent_data,
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
pub(crate) fn write_char_sequence(p: &mut Parcel, text: Option<&str>) {
    p.write_i32(1);
    p.write_string8(text);
}

/// `TextUtils.CHAR_SEQUENCE_CREATOR`: the text, past its spans.
pub(crate) fn char_sequence(r: &mut Reader<'_>) -> Result<Option<String>> {
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
        skip_span(r, span)?;
        for _ in 0..3 {
            r.read_i32()?; // start, end, flags
        }
    }
}

/// A span's `writeToParcelInternal`, skipped (`TextUtils` span ids).
fn skip_span(r: &mut Reader<'_>, span: i32) -> Result<()> {
    // The fixed layouts: i = an int or a float, s = a String.
    let layout = match span {
        5 | 6 | 14 | 15 | 31 => "",
        2 | 3 | 4 | 12 | 20 | 21 | 25 | 27 | 28 => "i",
        7 | 10 | 16 => "ii",
        9 => "iii",
        8 => "iiii",
        1 | 11 => "s",
        18 => "ss",
        26 => "si",
        // The family, then the typeface (LeakyTypefaceStorage: pid, index).
        13 => "sii",
        // SuggestionSpan, past its suggestions: flags, locale, language
        // tag, hash code, four underline colors and thicknesses.
        19 => {
            aim_service_aidl::read_string_list(r)?;
            "issiiiiiiiii"
        }
        17 => return skip_text_appearance(r),
        // EasyEditSpan: its PendingIntent (the binder), then whether
        // deleting is enabled.
        22 => {
            if skip_parcelable_name(r)? {
                r.read_binder()?;
            }
            "i"
        }
        // LocaleSpan: its LocaleList's tags.
        23 => {
            r.read_string8()?;
            ""
        }
        // TtsSpan: its type and arguments (a PersistableBundle).
        24 => {
            r.read_string16()?;
            bundle(r)?;
            ""
        }
        // AccessibilityReplacementSpan: its content description.
        29 => {
            char_sequence(r)?;
            ""
        }
        // LineBreakConfigSpan: its LineBreakConfig (style, word style,
        // hyphenation).
        30 => {
            if skip_parcelable_name(r)? {
                "iii"
            } else {
                ""
            }
        }
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
    Ok(())
}

/// `TextAppearanceSpan.writeToParcelInternal`.
fn skip_text_appearance(r: &mut Reader<'_>) -> Result<()> {
    r.read_string16()?; // family
    r.read_i32()?; // style
    r.read_i32()?; // size
    for _ in 0..2 {
        // The text and link colors: a ColorStateList's state specs and
        // colors.
        if r.read_i32()? != 0 {
            for _ in 0..r.read_i32()?.max(0) {
                aim_service_aidl::read_int_array(r)?;
            }
            aim_service_aidl::read_int_array(r)?;
        }
    }
    r.read_i32()?; // typeface: pid
    r.read_i32()?; // and index
    r.read_i32()?; // font weight
    if skip_parcelable_name(r)? {
        r.read_string8()?; // LocaleList
    }
    for _ in 0..8 {
        // Shadow radius, dx, dy and color; elegant text height (has, is);
        // letter spacing (has, value).
        r.read_i32()?;
    }
    r.read_string16()?; // font feature settings
    r.read_string16()?; // font variation settings
    Ok(())
}

/// `writeParcelable`'s class name: whether a parcelable follows.
fn skip_parcelable_name(r: &mut Reader<'_>) -> Result<bool> {
    Ok(r.read_string16()?.is_some())
}

/// `TextLinks.writeToParcel`: the text, its links and extras.
fn text_links(r: &mut Reader<'_>) -> Result<()> {
    r.read_string16()?;
    for _ in 0..r.read_i32()?.max(0) {
        if r.read_i32()? != 0 {
            // TextLink: its entity scores, start, end and extras.
            for _ in 0..r.read_i32()?.max(0) {
                r.read_string16()?;
                r.read_f32()?;
            }
            r.read_i32()?;
            r.read_i32()?;
            bundle(r)?;
        }
    }
    bundle(r).map(drop)
}

/// `Uri.CREATOR`: the URI string, or none for null.
pub(crate) fn uri(r: &mut Reader<'_>) -> Result<Option<String>> {
    match r.read_i32()? {
        0 => Ok(None),
        1..=3 => r.read_string8(),
        _ => Err(BAD_VALUE),
    }
}

/// `BaseBundle.readFromParcelInner`, skipped: the length, and unless
/// empty, the magic, that many bytes and whether it holds an intent.
/// Whether there was one: a negative length is null.
pub(crate) fn bundle(r: &mut Reader<'_>) -> Result<bool> {
    let length = r.read_i32()?;
    if length <= 0 {
        return Ok(length == 0);
    }
    match r.read_i32()? {
        bundles::MAGIC | bundles::MAGIC_NATIVE => {
            r.skip(length as usize)?;
            r.read_bool().map(|_| true)
        }
        _ => Err(BAD_VALUE),
    }
}

/// `Intent(Parcel)`, skipped: its data URI.
pub(crate) fn intent(r: &mut Reader<'_>) -> Result<Option<String>> {
    r.read_string8()?; // action
    let data = uri(r)?;
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
        intent(r)?; // selector
    }
    if r.read_i32()? != 0 {
        ClipData::read_from(r)?;
    }
    r.read_i32()?; // content user hint
    bundle(r)?; // extras
    if r.read_i32()? != 0 {
        intent(r)?; // original intent
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
    Ok(data)
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
        p.write_i32(bundles::MAGIC);
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
        assert_eq!(clip.items[0].uri.as_deref(), Some("content://media/1"));
        assert_eq!(clip.items[1].text.as_deref(), Some("héllo"));
        assert_eq!(
            clip.items[1].intent_data.as_deref(),
            Some("https://example.com")
        );
        let mut out = Parcel::new();
        clip.write_to(&mut out);
        assert_eq!(out.data(), p.data());
    }

    #[test]
    fn refuses_what_it_cannot_parse() {
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_string8(Some("x"));
        p.write_i32(32); // past TextUtils.LAST_SPAN
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(char_sequence(&mut r), Err(BAD_VALUE));
    }

    /// Text with each span whose parcel form is not a fixed list, as
    /// their `writeToParcelInternal` write them.
    #[test]
    fn reads_every_span() {
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_string8(Some("styled"));
        let place = |p: &mut Parcel| (0..3).for_each(|i| p.write_i32(i));
        // TextAppearanceSpan with a text color and a locale list.
        p.write_i32(17);
        p.write_string16(Some("serif"));
        p.write_i32(1);
        p.write_i32(12);
        p.write_i32(1);
        p.write_i32(1); // one state spec
        aim_service_aidl::write_int_array(&mut p, Some(&[0x10100a7]));
        aim_service_aidl::write_int_array(&mut p, Some(&[-1]));
        p.write_i32(0); // no link color
        p.write_i32(42);
        p.write_i32(0);
        p.write_i32(400);
        p.write_string16(Some("android.os.LocaleList"));
        p.write_string8(Some("en-US"));
        (0..8).for_each(|_| p.write_i32(0));
        p.write_string16(None);
        p.write_string16(Some("wght 400"));
        place(&mut p);
        // SuggestionSpan.
        p.write_i32(19);
        aim_service_aidl::write_string_list(&mut p, Some(&[Some("styles".into())]));
        p.write_i32(1);
        p.write_string16(None);
        p.write_string16(Some("en"));
        (0..9).for_each(|_| p.write_i32(0));
        place(&mut p);
        // EasyEditSpan without an intent, LocaleSpan, TtsSpan.
        p.write_i32(22);
        p.write_string16(None);
        p.write_i32(1);
        place(&mut p);
        p.write_i32(23);
        p.write_string8(Some("ko-KR"));
        place(&mut p);
        p.write_i32(24);
        p.write_string16(Some("android.type.text"));
        bundles::write(
            &mut p,
            &[("android.arg.text", Value::String(Some("s".into())))],
        );
        place(&mut p);
        // AccessibilityReplacementSpan, LineBreakConfigSpan.
        p.write_i32(29);
        write_char_sequence(&mut p, Some("description"));
        place(&mut p);
        p.write_i32(30);
        p.write_string16(Some("android.graphics.text.LineBreakConfig"));
        (0..3).for_each(|i| p.write_i32(i));
        place(&mut p);
        p.write_i32(0);
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(char_sequence(&mut r).unwrap().as_deref(), Some("styled"));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn reads_text_links() {
        let mut p = Parcel::new();
        p.write_string16(Some("call 555"));
        p.write_i32(1);
        p.write_i32(1);
        p.write_i32(1);
        p.write_string16(Some("phone"));
        p.write_f32(0.9);
        p.write_i32(5);
        p.write_i32(8);
        bundles::write(&mut p, &[]);
        bundles::write(&mut p, &[]);
        let mut r = Reader::new(p.data(), &[]);
        text_links(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn keeps_the_files_of_its_fds() {
        let mut p = java_clip();
        let mut clip = {
            let mut r = Reader::new(p.data(), p.objects());
            ClipData::read_from(&mut r).unwrap()
        };
        // An fd object in the description's extras, as the driver leaves
        // it: kept by the file the call's fd stands for.
        let mut extras = Parcel::new();
        extras.write_file(std::sync::Arc::new(7u32));
        clip.description.extras = {
            let mut r = Reader::new(extras.data(), extras.objects());
            r.skip(extras.data().len()).unwrap();
            Raw::capture(&r, 0)
        };
        assert_eq!(clip.keep_files(&|_| None), Err(BAD_VALUE));
        let file: File = std::sync::Arc::new(());
        clip.keep_files(&|fd| (fd == 0).then(|| file.clone()))
            .unwrap();
        p = Parcel::new();
        clip.write_to(&mut p);
        assert_eq!(p.files().len(), 1);
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
