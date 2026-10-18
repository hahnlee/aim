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
    /// Where its component is.
    component: (usize, usize),
}

impl RawIntent {
    /// Sets its flags and extended flags.
    pub fn set_flags(&mut self, flags: i32, extended_flags: i32) {
        self.intent.flags = flags;
        self.intent.extended_flags = extended_flags;
        self.raw.set_i32(self.flags_at, flags);
        self.raw.set_i32(self.flags_at + 4, extended_flags);
    }

    /// `new Intent(action)` with `flags` and the extras `extras`, as
    /// `Intent.writeToParcel` writes it.
    pub fn new(action: &str, flags: i32, extras: &[(&str, crate::bundle::Value)]) -> RawIntent {
        Self::of(Some(action), flags, extras)
    }

    /// `new Intent()` (no action) or `new Intent(action)`, with `flags`
    /// and the extras `extras`.
    pub fn of(action: Option<&str>, flags: i32, extras: &[(&str, crate::bundle::Value)]) -> RawIntent {
        Self::with_data(action, None, None, flags, extras)
    }

    /// `new Intent(action, data)`, with its package (`setPackage`),
    /// `flags` and the extras `extras`.
    pub fn with_data(
        action: Option<&str>,
        data: Option<&Uri>,
        package: Option<&str>,
        flags: i32,
        extras: &[(&str, crate::bundle::Value)],
    ) -> RawIntent {
        let mut p = Parcel::new();
        p.write_string8(action);
        match data {
            Some(uri) => uri.write(&mut p),
            None => p.write_i32(0), // `Uri.writeToParcel` of null
        }
        p.write_string8(None); // type
        p.write_string8(None); // identifier
        p.write_i32(flags);
        p.write_i32(0); // extended flags
        p.write_string8(package);
        p.write_string16(None); // no component
        p.write_i32(0); // no source bounds
        p.write_i32(0); // no categories
        p.write_i32(0); // no selector
        p.write_i32(0); // no clip data
        p.write_i32(-2); // content user hint: `UserHandle.USER_CURRENT`
        if extras.is_empty() {
            p.write_i32(-1); // no extras
        } else {
            crate::bundle::write(&mut p, extras);
        }
        p.write_i32(0); // no original intent
        p.write_i32(0); // no creator token
        RawIntent::read_from(&mut Reader::new(p.data(), &[])).expect("a written intent reads")
    }

    /// The intent with its component set (`Intent.setComponent`).
    pub fn with_component(&self, package: &str, class: &str) -> RawIntent {
        let mut p = Parcel::new();
        p.write_string16(Some(package));
        p.write_string16(Some(class));
        let (start, end) = self.component;
        RawIntent {
            intent: Intent {
                component: Some(crate::package::intent::ComponentName { package: package.into(), class: class.into() }),
                ..self.intent.clone()
            },
            raw: self.raw.splice(start, end, p.data()),
            flags_at: self.flags_at,
            component: (start, start + p.data().len()),
        }
    }

    /// `Intent.hasFileDescriptors`.
    pub fn has_fds(&self) -> bool {
        self.raw.has_fds()
    }

    pub fn handles(&self) -> Vec<u32> {
        self.raw.handles().collect()
    }
}

/// `Intent.FILL_IN_*`.
pub const FILL_IN_ACTION: i32 = 1 << 0;
pub const FILL_IN_DATA: i32 = 1 << 1;
pub const FILL_IN_CATEGORIES: i32 = 1 << 2;
pub const FILL_IN_COMPONENT: i32 = 1 << 3;
pub const FILL_IN_PACKAGE: i32 = 1 << 4;
pub const FILL_IN_SOURCE_BOUNDS: i32 = 1 << 5;
pub const FILL_IN_SELECTOR: i32 = 1 << 6;
pub const FILL_IN_CLIP_DATA: i32 = 1 << 7;
pub const FILL_IN_IDENTIFIER: i32 = 1 << 8;
/// `UserHandle.USER_CURRENT`: an intent's content user hint unless set.
const USER_CURRENT: i32 = -2;

/// Where each of an intent's fields is in its parcel, start and end.
struct Spans {
    action: (usize, usize),
    data: (usize, usize),
    ty: (usize, usize),
    identifier: (usize, usize),
    package: (usize, usize),
    component: (usize, usize),
    bounds: (usize, usize),
    categories: (usize, usize),
    selector: (usize, usize),
    clip: (usize, usize),
    content_user: usize,
    extras: (usize, usize),
    rest: (usize, usize),
}

impl Spans {
    /// `Intent(Parcel)` read for where its fields are.
    fn of(raw: &Raw) -> Result<Spans> {
        let p = raw.parcel();
        let mut r = p.reader();
        let span = |r: &mut Reader<'_>, read: &dyn Fn(&mut Reader<'_>) -> Result<()>| -> Result<(usize, usize)> {
            let start = r.position();
            read(r)?;
            Ok((start, r.position()))
        };
        let action = span(&mut r, &|r| r.read_string8().map(drop))?;
        let data = span(&mut r, &|r| Uri::read(r, &mut Plain).map(drop))?;
        let ty = span(&mut r, &|r| r.read_string8().map(drop))?;
        let identifier = span(&mut r, &|r| r.read_string8().map(drop))?;
        r.read_i32()?; // flags
        r.read_i32()?; // extended flags
        let package = span(&mut r, &|r| r.read_string8().map(drop))?;
        let component = span(&mut r, &|r| {
            if r.read_string16()?.is_some() {
                r.read_string16()?;
            }
            Ok(())
        })?;
        let bounds = span(&mut r, &|r| {
            if r.read_i32()? != 0 {
                r.skip(16)?;
            }
            Ok(())
        })?;
        let categories = span(&mut r, &|r| {
            for _ in 0..r.read_i32()?.max(0) {
                r.read_string8()?;
            }
            Ok(())
        })?;
        let selector = span(&mut r, &|r| {
            if r.read_i32()? != 0 {
                Intent::read(r, &mut Plain)?;
            }
            Ok(())
        })?;
        let clip = span(&mut r, &|r| {
            if r.read_i32()? != 0 {
                crate::clip::ClipData::read_from(r)?;
            }
            Ok(())
        })?;
        let content_user = r.position();
        r.read_i32()?;
        let extras = span(&mut r, &|r| clip::bundle(r).map(drop))?;
        let rest = (r.position(), p.data().len());
        Ok(Spans { action, data, ty, identifier, package, component, bounds, categories, selector, clip, content_user, extras, rest })
    }
}

/// Whether the intent whose fields after its extras start at `at` has a
/// creator token (after its original intent).
fn has_creator_token(raw: &Raw, at: usize) -> Result<bool> {
    let p = raw.parcel();
    let mut r = p.reader();
    r.set_position(at);
    if r.read_i32()? != 0 {
        Intent::read(&mut r, &mut Plain)?;
    }
    Ok(r.read_i32()? != 0)
}

/// Whether a span holds more than a null (a length or flag of -1 or 0).
fn present(raw: &Raw, (start, end): (usize, usize)) -> bool {
    let p = raw.parcel();
    let v = i32::from_le_bytes(p.data()[start..start + 4].try_into().unwrap());
    end - start > 4 || !(v == -1 || v == 0)
}

/// A bundle's magic, its entries (each key and its bytes) and whether it
/// holds an intent.
type Entries = (i32, Vec<(String, Raw)>, bool);

/// A bundle's entries; none when it is null.
fn entries(raw: &Raw, (start, _): (usize, usize)) -> Result<Option<Entries>> {
    let p = raw.parcel();
    let mut r = p.reader();
    r.set_position(start);
    let length = r.read_i32()?;
    if length < 0 {
        return Ok(None);
    }
    if length == 0 {
        return Ok(Some((crate::bundle::MAGIC, Vec::new(), false)));
    }
    let magic = r.read_i32()?;
    let end = r.position() + length as usize;
    let mut out = Vec::new();
    for _ in 0..r.read_i32()?.max(0) {
        let at = r.position();
        let key = r.read_string16()?.unwrap_or_default();
        crate::notifications::parcels::skip_value(&mut r)?;
        out.push((key, raw.slice(at, r.position())));
    }
    r.set_position(end);
    let has_intent = r.read_bool()?;
    Ok(Some((magic, out, has_intent)))
}

impl RawIntent {
    /// `Intent.fillIn(other, flags)`: this intent with what `other` fills
    /// in, and the `FILL_IN_*` changes it made.
    pub fn fill_in(&self, other: &RawIntent, flags: i32) -> Result<(RawIntent, i32)> {
        let (a, b) = (&self.raw, &other.raw);
        let (sa, sb) = (Spans::of(a)?, Spans::of(b)?);
        let (mine, theirs) = (&self.intent, &other.intent);
        let mut changes = 0;
        let pick = |take: bool, sa: (usize, usize), sb: (usize, usize)| if take { b.slice(sb.0, sb.1) } else { a.slice(sa.0, sa.1) };
        let action = theirs.action.is_some() && (mine.action.is_none() || flags & FILL_IN_ACTION != 0);
        changes |= if action { FILL_IN_ACTION } else { 0 };
        let data = (theirs.data.is_some() || theirs.ty.is_some()) && ((mine.data.is_none() && mine.ty.is_none()) || flags & FILL_IN_DATA != 0);
        changes |= if data { FILL_IN_DATA } else { 0 };
        let identifier = theirs.identifier.is_some() && (mine.identifier.is_none() || flags & FILL_IN_IDENTIFIER != 0);
        changes |= if identifier { FILL_IN_IDENTIFIER } else { 0 };
        let categories = theirs.categories.is_some() && (mine.categories.is_none() || flags & FILL_IN_CATEGORIES != 0);
        changes |= if categories { FILL_IN_CATEGORIES } else { 0 };
        let package = theirs.package.is_some() && (mine.package.is_none() || flags & FILL_IN_PACKAGE != 0) && mine.selector.is_none();
        changes |= if package { FILL_IN_PACKAGE } else { 0 };
        let selector = theirs.selector.is_some() && flags & FILL_IN_SELECTOR != 0 && mine.package.is_none() && !package;
        if selector {
            changes |= FILL_IN_SELECTOR;
        }
        let clip = present(b, sb.clip) && (!present(a, sa.clip) || flags & FILL_IN_CLIP_DATA != 0);
        changes |= if clip { FILL_IN_CLIP_DATA } else { 0 };
        let component = theirs.component.is_some() && flags & FILL_IN_COMPONENT != 0;
        changes |= if component { FILL_IN_COMPONENT } else { 0 };
        let bounds = present(b, sb.bounds) && (!present(a, sa.bounds) || flags & FILL_IN_SOURCE_BOUNDS != 0);
        changes |= if bounds { FILL_IN_SOURCE_BOUNDS } else { 0 };
        // The other's creator token's nested intent keys
        // (`fillInCreatorTokenInfo`) are not merged.
        if has_creator_token(b, sb.rest.0)? {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let int = |v: i32| Raw::of(&{
            let mut p = Parcel::new();
            p.write_i32(v);
            p
        });
        // Extras: the other's, then this one's over them (`putAll`).
        let extras = match (entries(a, sa.extras)?, entries(b, sb.extras)?) {
            (None, None) => int(-1),
            (Some(_), None) => a.slice(sa.extras.0, sa.extras.1),
            (None, Some(_)) => b.slice(sb.extras.0, sb.extras.1),
            (Some((ma, ea, ia)), Some((mb, eb, ib))) => {
                if ma != mb {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                let mut parts: Vec<Raw> = eb.into_iter().filter(|(k, _)| !ea.iter().any(|(m, _)| m == k)).map(|(_, r)| r).collect();
                let count = parts.len() + ea.len();
                parts.extend(ea.into_iter().map(|(_, r)| r));
                let body = Raw::concat(&parts);
                let mut head = Parcel::new();
                head.write_i32((4 + body.parcel().data().len()) as i32);
                head.write_i32(ma);
                head.write_i32(count as i32);
                let mut tail = Parcel::new();
                tail.write_bool(ia || ib);
                Raw::concat(&[Raw::of(&head), body, Raw::of(&tail)])
            }
        };
        let mut flags_raw = Parcel::new();
        flags_raw.write_i32(mine.flags | theirs.flags);
        flags_raw.write_i32(mine.extended_flags | theirs.extended_flags);
        let user_a = i32::from_le_bytes(a.parcel().data()[sa.content_user..sa.content_user + 4].try_into().unwrap());
        let user_b = i32::from_le_bytes(b.parcel().data()[sb.content_user..sb.content_user + 4].try_into().unwrap());
        let copied_uris = data || clip || present(b, sb.extras);
        let user = if copied_uris && user_a == USER_CURRENT && user_b != USER_CURRENT { user_b } else { user_a };
        let selector_part = if selector {
            b.slice(sb.selector.0, sb.selector.1)
        } else {
            a.slice(sa.selector.0, sa.selector.1)
        };
        let raw = Raw::concat(&[
            pick(action, sa.action, sb.action),
            pick(data, sa.data, sb.data),
            pick(data, sa.ty, sb.ty),
            pick(identifier, sa.identifier, sb.identifier),
            Raw::of(&flags_raw),
            pick(package, sa.package, sb.package),
            pick(component, sa.component, sb.component),
            pick(bounds, sa.bounds, sb.bounds),
            pick(categories, sa.categories, sb.categories),
            selector_part,
            pick(clip, sa.clip, sb.clip),
            int(user),
            extras,
            a.slice(sa.rest.0, sa.rest.1),
        ]);
        // Read again for its fields; the parcel kept is the one made, with
        // the files of its fds.
        let filled = RawIntent::read_from(&mut raw.parcel().reader())?;
        Ok((RawIntent { raw, ..filled }, changes))
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
        r.read_i32()?;
        r.read_i32()?;
        r.read_string8()?; // package
        let component_at = r.position() - start;
        if r.read_string16()?.is_some() {
            r.read_string16()?;
        }
        let component = (component_at, r.position() - start);
        r.set_position(start);
        let intent = Intent::read(r, &mut Plain)?;
        Ok(RawIntent { intent, raw: Raw::capture(r, start), flags_at, component })
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

/// An intent kept past its call: the binders among it held.
#[derive(Clone)]
pub struct KeptIntent {
    pub intent: RawIntent,
    _binders: std::sync::Arc<Vec<Strong>>,
}

impl KeptIntent {
    pub fn new(intent: RawIntent, process: &LocalProcess) -> KeptIntent {
        let binders = intent.handles().into_iter().map(|h| process.strong(h)).collect();
        KeptIntent { intent, _binders: std::sync::Arc::new(binders) }
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
    fn a_new_intent_has_its_action_and_extras() {
        let i = RawIntent::new("a.B", 0x10, &[("k", crate::bundle::Value::Int(3))]);
        assert_eq!((i.intent.action.as_deref(), i.intent.flags), (Some("a.B"), 0x10));
        let mut p = Parcel::new();
        i.write_to(&mut p);
        assert_eq!(RawIntent::read_from(&mut Reader::new(p.data(), &[])).unwrap().intent, i.intent);
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
        // A component set where there was none.
        let explicit = kept.with_component("p", "p.Main");
        let mut out = Parcel::new();
        explicit.write_to(&mut out);
        let read = RawIntent::read_from(&mut Reader::new(out.data(), &[])).unwrap();
        assert_eq!(read.intent.component.map(|c| c.class).as_deref(), Some("p.Main"));
        assert_eq!(read.intent.action.as_deref(), Some("a.ACTION"));
        kept.set_flags(0x30, 1);
        let mut flagged = Parcel::new();
        kept.write_to(&mut flagged);
        let read = RawIntent::read_from(&mut Reader::new(flagged.data(), &[])).unwrap();
        assert_eq!((read.intent.flags, read.intent.extended_flags), (0x30, 1));
    }

    /// The extras of an intent's parcel.
    fn extras(i: &RawIntent) -> std::collections::HashMap<String, crate::bundle::Value> {
        let p = i.raw.parcel();
        let mut r = p.reader();
        r.set_position(Spans::of(&i.raw).unwrap().extras.0);
        crate::bundle::read(&mut r).unwrap().unwrap_or_default()
    }

    /// `Intent.fillIn`: what this intent lacks is filled in, its extras
    /// win over the other's, flags are merged, and a component only with
    /// `FILL_IN_COMPONENT`.
    #[test]
    fn fill_in_follows_the_original() {
        use crate::bundle::Value;
        let sent = intent();
        let mine = RawIntent::read_from(&mut Reader::new(sent.data(), &[])).unwrap();
        let other = RawIntent::new("other.ACTION", 0x4, &[("k", Value::Int(1)), ("x", Value::Int(2))]).with_component("q", "q.C");
        let (filled, changes) = mine.fill_in(&other, 0).unwrap();
        assert_eq!(changes, 0);
        assert_eq!(filled.intent.action.as_deref(), Some("a.ACTION"));
        assert_eq!(filled.intent.flags, 0x14);
        assert!(filled.intent.component.is_none());
        let e = extras(&filled);
        assert_eq!((e.get("k"), e.get("x")), (Some(&Value::Int(7)), Some(&Value::Int(2))));
        let (filled, changes) = mine.fill_in(&other, FILL_IN_ACTION | FILL_IN_COMPONENT).unwrap();
        assert_eq!(changes, FILL_IN_ACTION | FILL_IN_COMPONENT);
        assert_eq!(filled.intent.action.as_deref(), Some("other.ACTION"));
        assert_eq!(filled.intent.component.map(|c| c.class).as_deref(), Some("q.C"));
        // An intent without extras or action takes the other's.
        let bare = RawIntent::of(None, 0, &[]);
        let (filled, _) = bare.fill_in(&other, 0).unwrap();
        assert_eq!(extras(&filled).len(), 2);
    }
}
