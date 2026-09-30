//! What the bridge reads of a `StatusBarNotification` and writes back, in
//! the Java wire format of the pinned tag (`StatusBarNotification`,
//! `Notification.writeToParcelImpl`, `Icon`, `Bitmap_writeToParcel`,
//! `BaseBundle`, `Parcel.writeValue`, `Notification.Action`,
//! `RemoteInput`, `Intent`, `ClipData`, `RemoteViews`, `ApplicationInfo`).
//!
//! A notification is read in order. `RemoteViews` (custom content, ticker
//! or heads-up views) are read past, not kept: the Mac cannot show them.
//! What cannot be read ends the reading, and what came before is kept
//! ([`Notification::complete`] is false).

use aim_binder_host::parcel::{BAD_TYPE, BAD_VALUE, Binder, Parcel, Reader, Result};
use aim_host_display::notify::Image;

use crate::clip::{bundle as skip_bundle, char_sequence, uri, write_char_sequence};

/// `BaseBundle.BUNDLE_MAGIC` and `BUNDLE_MAGIC_NATIVE`.
const BUNDLE_MAGIC: i32 = 0x4C44_4E42;
const BUNDLE_MAGIC_NATIVE: i32 = 0x4C44_4E44;

/// `Parcel.VAL_*`.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
const VAL_INTEGER: i32 = 1;
const VAL_BUNDLE: i32 = 3;
const VAL_PARCELABLE: i32 = 4;
const VAL_SHORT: i32 = 5;
const VAL_LONG: i32 = 6;
const VAL_FLOAT: i32 = 7;
const VAL_DOUBLE: i32 = 8;
const VAL_BOOLEAN: i32 = 9;
const VAL_CHARSEQUENCE: i32 = 10;
const VAL_BYTEARRAY: i32 = 13;
const VAL_STRINGARRAY: i32 = 14;
const VAL_IBINDER: i32 = 15;
const VAL_PARCELABLEARRAY: i32 = 16;
const VAL_INTARRAY: i32 = 18;
const VAL_LONGARRAY: i32 = 19;
const VAL_BYTE: i32 = 20;
const VAL_BOOLEANARRAY: i32 = 23;
const VAL_CHARSEQUENCEARRAY: i32 = 24;
const VAL_PERSISTABLEBUNDLE: i32 = 25;
const VAL_SIZE: i32 = 26;
const VAL_SIZEF: i32 = 27;
const VAL_DOUBLEARRAY: i32 = 28;
const VAL_CHAR: i32 = 29;
const VAL_SHORTARRAY: i32 = 30;
const VAL_CHARARRAY: i32 = 31;
const VAL_FLOATARRAY: i32 = 32;

/// `Parcel.isLengthPrefixed`: map, parcelable, list, sparse array,
/// parcelable and object arrays, serializable.
fn length_prefixed(kind: i32) -> bool {
    matches!(kind, 2 | 4 | 11 | 12 | 16 | 17 | 21)
}

/// `Notification.FLAG_*`.
pub const FLAG_ONGOING_EVENT: i32 = 0x2;
pub const FLAG_ONLY_ALERT_ONCE: i32 = 0x8;
pub const FLAG_NO_CLEAR: i32 = 0x20;
pub const FLAG_FOREGROUND_SERVICE: i32 = 0x40;
pub const FLAG_GROUP_SUMMARY: i32 = 0x200;

/// Reads a blob's file: the fd of a call or reply being read, and how
/// many bytes of it.
pub trait Files {
    fn read(&self, fd: u32, len: usize) -> Option<Vec<u8>>;
}

/// A notification's icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    Image(Image),
    /// Drawable `id` of `package`'s resources.
    Resource {
        package: String,
        id: i32,
    },
    /// A `content:` or `file:` URI.
    Uri(String),
    /// A bitmap in a format the Mac does not take.
    Unreadable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteInput {
    pub result_key: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Action {
    pub title: Option<String>,
    /// Its `PendingIntent`'s `IIntentSender`.
    pub intent: Option<Binder>,
    pub inputs: Vec<RemoteInput>,
}

/// A message of a messaging-style notification.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub sender: Option<String>,
    pub text: Option<String>,
}

/// The extras a Mac notification shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    pub title: Option<String>,
    pub text: Option<String>,
    pub sub_text: Option<String>,
    pub big_text: Option<String>,
    pub conversation_title: Option<String>,
    pub lines: Vec<String>,
    pub messages: Vec<Message>,
    pub picture: Option<Icon>,
    pub large_icon: Option<Icon>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    /// The token its `PendingIntent`s are sent with (`mAllowlistToken`).
    pub allowlist_token: Option<Binder>,
    pub content_intent: Option<Binder>,
    pub full_screen_intent: Option<Binder>,
    pub ticker: Option<String>,
    pub large_icon: Option<Icon>,
    pub flags: i32,
    pub group: Option<String>,
    /// `Notification.GROUP_ALERT_*`.
    pub group_alert_behavior: i32,
    /// Its `BubbleMetadata`'s flags.
    pub bubble_flags: i32,
    pub extras: Extras,
    pub actions: Vec<Action>,
    pub channel_id: Option<String>,
    pub shortcut_id: Option<String>,
    /// Read to its end.
    pub complete: bool,
}

impl Notification {
    /// Every binder it holds, which must outlive the parcel it came in.
    pub fn binders(&self) -> impl Iterator<Item = Binder> + '_ {
        [
            self.allowlist_token,
            self.content_intent,
            self.full_screen_intent,
        ]
        .into_iter()
        .chain(self.actions.iter().map(|a| a.intent))
        .flatten()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusBarNotification {
    pub package: String,
    pub id: i32,
    pub tag: Option<String>,
    pub uid: i32,
    pub notification: Notification,
    /// Its `UserHandle`; after the notification, so known only when that
    /// was read completely.
    pub user: Option<i32>,
    pub override_group_key: Option<String>,
}

impl StatusBarNotification {
    /// `StatusBarNotification.key()`.
    pub fn key(&self) -> String {
        // UserHandle.getUserId: the uid's user when the handle is unknown.
        let user = self.user.unwrap_or(self.uid / 100_000);
        let mut key = format!(
            "{user}|{}|{}|{}|{}",
            self.package,
            self.id,
            self.tag.as_deref().unwrap_or("null"),
            self.uid
        );
        if let Some(g) = &self.override_group_key
            && self.notification.flags & FLAG_GROUP_SUMMARY != 0
        {
            key = format!("{key}|{g}");
        }
        key
    }

    /// `StatusBarNotification(Parcel)`: the notification as far as it can
    /// be read (the rest of the parcel is then unread).
    pub fn read(r: &mut Reader<'_>, files: &dyn Files) -> Result<Self> {
        let package = r.read_string16()?.unwrap_or_default();
        r.read_string16()?; // opPkg
        let id = r.read_i32()?;
        let tag = if r.read_i32()? != 0 {
            r.read_string16()?
        } else {
            None
        };
        let uid = r.read_i32()?;
        r.read_i32()?; // initialPid
        let mut sbn = StatusBarNotification {
            package,
            id,
            tag,
            uid,
            ..Default::default()
        };
        let mut n = Notification::default();
        let done = read_notification(r, files, &mut n);
        sbn.notification = n;
        if done.is_err() || !sbn.notification.complete {
            return Ok(sbn);
        }
        sbn.user = Some(r.read_i32()?);
        r.read_i64()?; // postTime
        if r.read_i32()? != 0 {
            sbn.override_group_key = r.read_string16()?;
        }
        if r.read_i32()? != 0 {
            r.read_i32()?; // InstanceId
        }
        Ok(sbn)
    }
}

/// A `readTypedObject` flag, then the object.
fn typed<T>(r: &mut Reader<'_>, f: impl FnOnce(&mut Reader<'_>) -> Result<T>) -> Result<Option<T>> {
    if r.read_i32()? != 0 {
        f(r).map(Some)
    } else {
        Ok(None)
    }
}

/// `PendingIntent.CREATOR`: its target.
fn pending_intent(r: &mut Reader<'_>) -> Result<Option<Binder>> {
    r.read_binder()
}

/// `Notification(Parcel)`, into `n`; stops (with `complete` false) at
/// what it does not read.
fn read_notification(r: &mut Reader<'_>, files: &dyn Files, n: &mut Notification) -> Result<()> {
    if r.read_i32()? != 1 {
        return Err(BAD_VALUE);
    }
    n.allowlist_token = r.read_binder()?;
    r.read_i64()?; // when
    r.read_i64()?; // creationTime
    typed(r, |r| icon(r, files))?; // small icon
    r.read_i32()?; // number
    n.content_intent = typed(r, pending_intent)?.flatten();
    typed(r, pending_intent)?; // delete intent
    n.ticker = typed(r, char_sequence)?.flatten();
    typed(r, |r| remote_views(r, files, 0))?; // ticker view
    typed(r, |r| remote_views(r, files, 0))?; // content view
    n.large_icon = typed(r, |r| icon(r, files))?;
    r.read_i32()?; // defaults
    n.flags = r.read_i32()?;
    typed(r, uri)?; // sound
    r.read_i32()?; // audioStreamType
    typed(r, audio_attributes)?;
    skip_array(r, 8)?; // vibrate
    for _ in 0..4 {
        r.read_i32()?; // ledARGB, ledOnMS, ledOffMS, iconLevel
    }
    n.full_screen_intent = typed(r, pending_intent)?.flatten();
    r.read_i32()?; // priority
    r.read_string8()?; // category
    n.group = r.read_string8()?;
    r.read_string8()?; // sortKey
    n.extras = extras(r, files)?;
    let count = r.read_i32()?;
    for _ in 0..count.max(0) {
        if let Some(a) = typed(r, |r| action(r, files))? {
            n.actions.push(a);
        }
    }
    typed(r, |r| remote_views(r, files, 0))?; // big content view
    typed(r, |r| remote_views(r, files, 0))?; // heads-up content view
    r.read_i32()?; // visibility
    if r.read_i32()? != 0 {
        let mut public = Notification::default();
        read_notification(r, files, &mut public)?;
        if !public.complete {
            return Ok(());
        }
    }
    r.read_i32()?; // color
    n.channel_id = typed(r, |r| r.read_string8())?.flatten();
    r.read_i64()?; // timeout
    n.shortcut_id = typed(r, |r| r.read_string8())?.flatten();
    typed(r, |r| r.read_string16())?; // locus id
    r.read_i32()?; // badge icon
    typed(r, char_sequence)?; // settings text
    n.group_alert_behavior = r.read_i32()?;
    n.bubble_flags = typed(r, |r| bubble_metadata(r, files))?.unwrap_or(0);
    r.read_bool()?; // allow system generated contextual actions
    r.read_i32()?; // FGS defer behavior
    // Notification.writeToParcel ends with the PendingIntents it wrote:
    // an ArraySet of values.
    for _ in 0..r.read_i32()?.max(0) {
        skip_value(r)?;
    }
    n.complete = true;
    Ok(())
}

/// `Notification.BubbleMetadata(Parcel)`: its flags.
fn bubble_metadata(r: &mut Reader<'_>, files: &dyn Files) -> Result<i32> {
    typed(r, pending_intent)?;
    typed(r, |r| icon(r, files))?;
    r.read_i32()?; // desired height
    let flags = r.read_i32()?;
    typed(r, pending_intent)?; // delete intent
    r.read_i32()?; // desired height resource
    typed(r, |r| r.read_string8())?; // shortcut id
    Ok(flags)
}

/// How deep `RemoteViews` nest (`RemoteViews.MAX_NESTED_VIEWS`).
const MAX_NESTED_VIEWS: u32 = 10;

/// `RemoteViews(Parcel)`, read past. The root of a hierarchy (`depth` 0)
/// carries the bitmap and collection caches its children share.
fn remote_views(r: &mut Reader<'_>, files: &dyn Files, depth: u32) -> Result<()> {
    if depth > MAX_NESTED_VIEWS {
        return Err(BAD_VALUE);
    }
    let mode = r.read_i32()?;
    if depth == 0 {
        // BitmapCache: a typed list of bitmaps.
        for _ in 0..r.read_i32()?.max(0) {
            typed(r, |r| bitmap(r, files))?;
        }
        // RemoteCollectionCache: intent id, URI, items.
        for _ in 0..r.read_i32()?.max(0) {
            r.read_i32()?;
            r.read_string8()?;
            collection_items(r, files, depth)?;
        }
    }
    match mode {
        // MODE_NORMAL
        0 => {
            typed(r, application_info)?;
            if r.read_i32()? != 0 {
                r.skip(8)?; // ideal size, a SizeF
            }
            for _ in 0..3 {
                r.read_i32()?; // layout, view id, light background layout
            }
            for _ in 0..r.read_i32()?.max(0) {
                view_action(r, files, depth)?;
            }
        }
        // MODE_HAS_LANDSCAPE_AND_PORTRAIT
        1 => {
            remote_views(r, files, depth + 1)?;
            remote_views(r, files, depth + 1)?;
        }
        // MODE_HAS_SIZED_REMOTEVIEWS
        2 => {
            for _ in 0..r.read_i32()?.max(0) {
                remote_views(r, files, depth + 1)?;
            }
        }
        _ => return Err(BAD_VALUE),
    }
    r.read_i32()?; // apply flags
    r.read_i64()?; // provider instance id
    r.read_bool()?; // has draw instructions
    Ok(())
}

/// `RemoteViews.RemoteCollectionItems(Parcel)`: its views, the first a
/// root of its own unless the items are attached to a hierarchy.
fn collection_items(r: &mut Reader<'_>, files: &dyn Files, depth: u32) -> Result<()> {
    r.read_bool()?; // has stable ids
    r.read_i32()?; // view type count
    let length = r.read_i32()?;
    skip_array(r, 8)?; // ids
    let attached = r.read_bool()?;
    for i in 0..length.max(0) {
        let child = if i == 0 && !attached { 0 } else { depth + 1 };
        remote_views(r, files, child)?;
    }
    Ok(())
}

/// `RemoteViews.RemoteResponse.readFromParcel`.
fn remote_response(r: &mut Reader<'_>) -> Result<()> {
    pending_intent(r)?;
    if r.read_bool()? {
        typed(r, crate::clip::intent)?; // fill-in intent
    }
    r.read_i32()?; // interaction type
    skip_array(r, 4)?; // view ids
    aim_service_aidl::read_string_list(r)?; // element names
    Ok(())
}

/// `RemoteViews.BaseReflectionAction(Parcel)`: its value's type.
fn reflection(r: &mut Reader<'_>) -> Result<i32> {
    r.read_i32()?; // view id
    r.read_string8()?; // method
    r.read_i32()
}

/// One of a `RemoteViews`' actions: its tag, then the action
/// (`RemoteViews.getActionFromParcel`).
fn view_action(r: &mut Reader<'_>, files: &dyn Files, depth: u32) -> Result<()> {
    let ints = |r: &mut Reader<'_>, n: usize| r.skip(4 * n);
    match r.read_i32()? {
        // SetOnClickResponse, SetOnCheckedChangeResponse
        1 | 29 => {
            r.read_i32()?;
            remote_response(r)?;
        }
        // ReflectionAction
        2 => match reflection(r)? {
            // boolean, byte, short, int, float, char, blend mode
            1..=4 | 6 | 8 | 17 => ints(r, 1)?,
            // long, double
            5 | 7 => r.skip(8)?,
            9 => {
                r.read_string8()?;
            }
            10 => {
                char_sequence(r)?;
            }
            11 => {
                typed(r, uri)?;
            }
            12 => {
                typed(r, |r| bitmap(r, files))?;
            }
            13 => skip_bundle_or_null(r)?,
            14 => {
                typed(r, crate::clip::intent)?;
            }
            15 => {
                typed(r, color_state_list)?;
            }
            16 => {
                typed(r, |r| icon(r, files))?;
            }
            _ => {}
        },
        // SetDrawableTint: view, target background, color, mode
        3 => ints(r, 4)?,
        // ViewGroupActionAdd: view, index, stable id, the views
        4 => {
            ints(r, 3)?;
            remote_views(r, files, depth + 1)?;
        }
        // ViewContentNavigation, SetEmptyView, ViewGroupActionRemove,
        // SetCompoundButtonChecked, SetRadioGroupChecked
        5 | 6 | 7 | 26 | 27 => ints(r, 2)?,
        // SetPendingIntentTemplate, SetOnStylusHandwritingResponse
        8 | 34 => {
            r.read_i32()?;
            pending_intent(r)?;
        }
        // SetRemoteViewsAdapterIntent
        10 => {
            r.read_i32()?;
            typed(r, crate::clip::intent)?;
        }
        // TextViewDrawableAction: four icons, or four resources
        11 => {
            ints(r, 2)?;
            if r.read_i32()? != 0 {
                for _ in 0..4 {
                    typed(r, |r| icon(r, files))?;
                }
            } else {
                ints(r, 4)?;
            }
        }
        // BitmapReflectionAction: view, method, bitmap id
        12 => {
            r.read_i32()?;
            r.read_string8()?;
            r.read_i32()?;
        }
        // TextViewSizeAction: view, units, size
        13 => ints(r, 3)?,
        // ViewPaddingAction
        14 => ints(r, 5)?,
        // SetRemoteInputsAction
        18 => {
            r.read_i32()?;
            for _ in 0..r.read_i32()?.max(0) {
                typed(r, remote_input)?;
            }
        }
        // LayoutParamAction
        19 => ints(r, 4)?,
        // SetRippleDrawableColor: a ColorStateList by writeParcelable
        21 => {
            r.read_i32()?;
            if r.read_string16()?.is_some() {
                color_state_list(r)?;
            }
        }
        // SetIntTagAction, SetViewOutlinePreferredRadiusAction
        22 | 28 => ints(r, 3)?,
        // RemoveFromParentAction
        23 => ints(r, 1)?,
        // ResourceReflectionAction, AttributeReflectionAction: type, id
        24 | 32 => {
            reflection(r)?;
            ints(r, 2)?;
        }
        // ComplexUnitDimensionReflectionAction: value, unit
        25 => {
            reflection(r)?;
            ints(r, 2)?;
        }
        // NightModeReflectionAction: light and dark values
        30 => match reflection(r)? {
            16 => {
                typed(r, |r| icon(r, files))?;
                typed(r, |r| icon(r, files))?;
            }
            15 => {
                typed(r, color_state_list)?;
                typed(r, color_state_list)?;
            }
            4 => ints(r, 2)?,
            _ => return Err(BAD_VALUE),
        },
        // SetRemoteCollectionItemListAdapterAction: view, intent id,
        // replaced, the service intent, else the items
        31 => {
            ints(r, 3)?;
            if typed(r, crate::clip::intent)?.is_none() {
                collection_items(r, files, depth)?;
            }
        }
        // Draw instructions (behind a flag) and unknown actions.
        _ => return Err(BAD_TYPE),
    }
    Ok(())
}

/// What an `ApplicationInfo` says of where an app's resources are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplicationInfo {
    pub source_dir: Option<String>,
    pub theme: i32,
}

impl aim_service_aidl::ReadParcelable for ApplicationInfo {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        application_info(r)?.ok_or(BAD_VALUE)
    }
}

/// `writeString8Array`: a count (-1 for null), then the strings.
fn string8_array(r: &mut Reader<'_>) -> Result<()> {
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string8()?;
    }
    Ok(())
}

/// `ApplicationInfo.CREATOR` (`readSquashed`): none when it refers back
/// to a copy written earlier in the parcel.
fn application_info(r: &mut Reader<'_>) -> Result<Option<ApplicationInfo>> {
    if r.read_i32()? != 0 {
        return Ok(None);
    }
    // PackageItemInfo: name, package, label, non-localized label, icon,
    // logo, meta-data, banner, user icon, archived.
    r.read_string8()?;
    r.read_string8()?;
    r.read_i32()?;
    char_sequence(r)?;
    r.skip(8)?;
    skip_bundle_or_null(r)?;
    r.skip(12)?;
    // Task affinity, permission, process, class name.
    for _ in 0..4 {
        r.read_string8()?;
    }
    let theme = r.read_i32()?;
    // Flags, private flags and more, the screen width limits.
    r.skip(24)?;
    if r.read_i32()? != 0 {
        r.skip(16)?; // storage UUID
    }
    r.read_string8()?; // scan source dir
    r.read_string8()?; // scan public source dir
    let source_dir = r.read_string8()?;
    r.read_string8()?; // public source dir
    for _ in 0..3 {
        string8_array(r)?; // split names, source dirs, public source dirs
    }
    // Split dependencies: a SparseArray of int arrays.
    for _ in 0..r.read_i32()?.max(0) {
        r.read_i32()?;
        skip_value(r)?;
    }
    for _ in 0..3 {
        r.read_string8()?; // native library dirs
    }
    r.read_i32()?;
    r.read_string8()?; // CPU ABIs
    r.read_string8()?;
    string8_array(r)?; // resource dirs
    string8_array(r)?; // overlay paths
    r.read_string8()?; // SELinux info
    r.read_string8()?;
    string8_array(r)?; // shared library files
    for _ in 0..2 {
        // Shared libraries, optional shared libraries.
        for _ in 0..r.read_i32()?.max(0) {
            typed(r, shared_library_info)?;
        }
    }
    for _ in 0..3 {
        r.read_string8()?; // data dirs
    }
    r.skip(12)?; // uid, min and target SDK
    r.read_i64()?; // version code
    r.skip(12)?; // enabled, enabled setting, install location
    r.read_string8()?; // manage space activity
    r.read_string8()?; // backup agent
    // Description, UI options, backup content, extraction rules, cross
    // profile, network security config, category, sandbox version.
    r.skip(32)?;
    r.read_string8()?; // class loader
    string8_array(r)?; // split class loaders
    r.read_i32()?; // compile SDK
    r.read_string8()?; // its codename
    r.read_string8()?; // app component factory
    r.skip(16)?; // icon, round icon, hidden API policy, hidden until installed
    r.read_string8()?; // zygote preload
    // GWP-ASan, MTE, heap zero init, raw external storage access.
    r.skip(16)?;
    r.read_i64()?; // creation time
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string16()?; // app class names by process
        r.read_string16()?;
    }
    r.skip(12)?; // locale config, cross-uid activity switch, page size compat
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string16()?; // known activity embedding certificates
    }
    Ok(Some(ApplicationInfo { source_dir, theme }))
}

/// `SharedLibraryInfo(Parcel)`, read past.
fn shared_library_info(r: &mut Reader<'_>) -> Result<()> {
    r.read_string8()?; // path
    r.read_string8()?; // package
    if r.read_i32()? != 0 {
        string8_array(r)?; // code paths
    }
    r.read_string8()?; // name
    r.read_i64()?; // version
    r.read_i32()?; // type
    versioned_package(r)?; // declaring package
    // Dependent packages: a list of values.
    for _ in 0..r.read_i32()?.max(0) {
        skip_value(r)?;
    }
    for _ in 0..r.read_i32()?.max(0) {
        typed(r, shared_library_info)?; // dependencies
    }
    r.read_bool()?; // native
    for _ in 0..r.read_i32()?.max(0) {
        versioned_package(r)?; // optional dependent packages
    }
    aim_service_aidl::read_string_list(r)?; // certificate digests
    Ok(())
}

/// A `VersionedPackage` by `writeParcelable`: its class, then the package
/// and version.
fn versioned_package(r: &mut Reader<'_>) -> Result<()> {
    if r.read_string16()?.is_some() {
        r.read_string8()?;
        r.read_i64()?;
    }
    Ok(())
}

/// `AudioAttributes(Parcel)` with the flags Notification writes it with.
fn audio_attributes(r: &mut Reader<'_>) -> Result<()> {
    for _ in 0..4 {
        r.read_i32()?; // usage, content type, source, flags
    }
    // FLATTEN_TAGS: one string, else an array of them.
    if r.read_i32()? & 1 != 0 {
        r.read_string16()?;
    } else {
        aim_service_aidl::read_string_list(r)?;
    }
    // ATTR_PARCEL_IS_VALID_BUNDLE
    if r.read_i32()? == 1980 {
        skip_bundle_or_null(r)?;
    }
    Ok(())
}

/// `writeBundle`: -1 for null, else the bundle.
fn skip_bundle_or_null(r: &mut Reader<'_>) -> Result<()> {
    skip_bundle(r).map(drop)
}

/// An array of `n` elements of `size` bytes each (-1 for null).
fn skip_array(r: &mut Reader<'_>, size: usize) -> Result<()> {
    let n = r.read_i32()?;
    if n > 0 {
        r.skip(n as usize * size)?;
    }
    Ok(())
}

/// `ColorStateList(Parcel)`.
fn color_state_list(r: &mut Reader<'_>) -> Result<()> {
    for _ in 0..r.read_i32()?.max(0) {
        skip_array(r, 4)?;
    }
    skip_array(r, 4)
}

/// `Icon(Parcel)`.
fn icon(r: &mut Reader<'_>, files: &dyn Files) -> Result<Icon> {
    let icon = match r.read_i32()? {
        // TYPE_BITMAP, TYPE_ADAPTIVE_BITMAP
        1 | 5 => bitmap(r, files)?.map_or(Icon::Unreadable, Icon::Image),
        // TYPE_RESOURCE: package, id, monochrome, inset scale
        2 => {
            let package = r.read_string16()?.unwrap_or_default();
            let id = r.read_i32()?;
            r.read_bool()?;
            r.read_f32()?;
            Icon::Resource { package, id }
        }
        // TYPE_DATA: its length, then `writeBlob` (the length again, then
        // the blob)
        3 => {
            r.read_i32()?;
            let len = r.read_i32()?;
            blob(r, files, len)?
                .map(|d| Icon::Image(Image::Encoded(d)))
                .unwrap_or(Icon::Unreadable)
        }
        // TYPE_URI, TYPE_URI_ADAPTIVE_BITMAP
        4 | 6 => Icon::Uri(r.read_string16()?.unwrap_or_default()),
        _ => return Err(BAD_VALUE),
    };
    typed(r, color_state_list)?; // tint
    r.read_i32()?; // blend mode
    Ok(icon)
}

/// `Parcel.writeBlob`'s content after its length: in place, or an ashmem
/// file.
fn blob(r: &mut Reader<'_>, files: &dyn Files, len: i32) -> Result<Option<Vec<u8>>> {
    if len < 0 {
        return Ok(None);
    }
    match r.read_i32()? {
        // BLOB_INPLACE
        0 => {
            let at = r.position();
            r.skip(len as usize)?;
            let (bytes, _) = r.since(at);
            Ok(Some(bytes[..len as usize].to_vec()))
        }
        // BLOB_ASHMEM_IMMUTABLE, BLOB_ASHMEM_MUTABLE
        1 | 2 => {
            let fd = r.read_fd()?;
            Ok(files.read(fd, len as usize))
        }
        _ => Err(BAD_VALUE),
    }
}

/// `writeByteArray`: its length (-1 for null), then the bytes.
fn byte_array(r: &mut Reader<'_>) -> Result<Option<Vec<u8>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    let at = r.position();
    r.skip(n as usize)?;
    Ok(Some(r.since(at).0[..n as usize].to_vec()))
}

/// `SkColorType` values a parcelled bitmap has on Android.
const ALPHA_8: i32 = 1;
const RGB_565: i32 = 2;
const RGBA_8888: i32 = 4;
const BGRA_8888: i32 = 6;
const GRAY_8: i32 = 14;
/// `SkAlphaType`: kOpaque, kPremul.
const OPAQUE: i32 = 1;
const PREMUL: i32 = 2;

/// `Bitmap_writeToParcel`: its pixels as RGBA, if the format is one the
/// Mac takes.
fn bitmap(r: &mut Reader<'_>, files: &dyn Files) -> Result<Option<Image>> {
    r.read_i32()?; // mutable
    let color_type = r.read_i32()?;
    let alpha_type = r.read_i32()?;
    byte_array(r)?; // color space
    let width = r.read_i32()?;
    let height = r.read_i32()?;
    let row_bytes = r.read_i32()?;
    r.read_i32()?; // density
    r.read_i64()?; // id
    let pixels = match r.read_i32()? {
        // BlobType::IN_PLACE
        0 => byte_array(r)?,
        // BlobType::ASHMEM: its size, then a ParcelFileDescriptor
        1 => {
            let size = r.read_i32()?;
            if r.read_i32()? == 0 {
                None
            } else {
                r.read_i32()?; // no comm channel
                let fd = r.read_fd()?;
                files.read(fd, size.max(0) as usize)
            }
        }
        _ => return Err(BAD_VALUE),
    };
    let (Some(pixels), true) = (pixels, width > 0 && height > 0 && row_bytes > 0) else {
        return Ok(None);
    };
    let (w, h, stride) = (width as usize, height as usize, row_bytes as usize);
    let bpp = match color_type {
        RGBA_8888 | BGRA_8888 => 4,
        RGB_565 => 2,
        ALPHA_8 | GRAY_8 => 1,
        _ => return Ok(None),
    };
    if stride < w * bpp || pixels.len() < stride * (h - 1) + w * bpp {
        return Ok(None);
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &pixels[y * stride..][..w * bpp];
        for p in row.chunks_exact(bpp) {
            rgba.extend_from_slice(&match color_type {
                RGBA_8888 => [p[0], p[1], p[2], p[3]],
                BGRA_8888 => [p[2], p[1], p[0], p[3]],
                RGB_565 => {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    let c = |bits: u16, max: u16| (u32::from(bits) * 255 / u32::from(max)) as u8;
                    [c(v >> 11, 31), c((v >> 5) & 63, 63), c(v & 31, 31), 255]
                }
                ALPHA_8 => [0, 0, 0, p[0]],
                _ => [p[0], p[0], p[0], 255],
            });
        }
    }
    Ok(Some(Image::Rgba {
        width: width as u32,
        height: height as u32,
        premultiplied: alpha_type == PREMUL || alpha_type == OPAQUE,
        pixels: rgba,
    }))
}

/// `Notification.Action(Parcel)`.
fn action(r: &mut Reader<'_>, files: &dyn Files) -> Result<Action> {
    typed(r, |r| icon(r, files))?;
    let title = char_sequence(r)?;
    let intent = typed(r, pending_intent)?.flatten();
    skip_bundle_or_null(r)?; // extras
    let mut inputs = Vec::new();
    let count = r.read_i32()?;
    for _ in 0..count.max(0) {
        if let Some(i) = typed(r, remote_input)? {
            inputs.push(i);
        }
    }
    for _ in 0..4 {
        r.read_i32()?; // generated replies, semantic, contextual, authentication
    }
    Ok(Action {
        title,
        intent,
        inputs,
    })
}

/// `RemoteInput(Parcel)`.
fn remote_input(r: &mut Reader<'_>) -> Result<RemoteInput> {
    let result_key = r.read_string16()?.unwrap_or_default();
    let label = char_sequence(r)?;
    let choices = r.read_i32()?;
    for _ in 0..choices.max(0) {
        char_sequence(r)?;
    }
    r.read_i32()?; // flags
    r.read_i32()?; // edit choices before sending
    skip_bundle_or_null(r)?;
    let types = r.read_i32()?; // allowed data types: an ArraySet of values
    for _ in 0..types.max(0) {
        skip_value(r)?;
    }
    Ok(RemoteInput { result_key, label })
}

/// One `writeValue`, skipped.
fn skip_value(r: &mut Reader<'_>) -> Result<()> {
    let kind = r.read_i32()?;
    if length_prefixed(kind) {
        let len = r.read_i32()?;
        return r.skip(len.max(0) as usize);
    }
    match kind {
        VAL_NULL => {}
        VAL_STRING => {
            r.read_string16()?;
        }
        VAL_INTEGER | VAL_SHORT | VAL_BOOLEAN | VAL_BYTE | VAL_CHAR => {
            r.read_i32()?;
        }
        VAL_LONG | VAL_DOUBLE => {
            r.read_i64()?;
        }
        VAL_FLOAT => {
            r.read_f32()?;
        }
        VAL_BUNDLE | VAL_PERSISTABLEBUNDLE => skip_bundle_or_null(r)?,
        VAL_CHARSEQUENCE => {
            char_sequence(r)?;
        }
        VAL_BYTEARRAY => {
            byte_array(r)?;
        }
        VAL_STRINGARRAY => {
            aim_service_aidl::read_string_list(r)?;
        }
        VAL_IBINDER => {
            r.read_binder()?;
        }
        VAL_INTARRAY | VAL_BOOLEANARRAY | VAL_SHORTARRAY | VAL_CHARARRAY | VAL_FLOATARRAY => {
            skip_array(r, 4)?
        }
        VAL_LONGARRAY | VAL_DOUBLEARRAY => skip_array(r, 8)?,
        VAL_CHARSEQUENCEARRAY => {
            for _ in 0..r.read_i32()?.max(0) {
                char_sequence(r)?;
            }
        }
        VAL_SIZE | VAL_SIZEF => r.skip(8)?,
        _ => return Err(BAD_TYPE),
    }
    Ok(())
}

/// A value read as text (a `String` or `CharSequence`); other values are
/// skipped.
fn text_value(r: &mut Reader<'_>, kind: i32) -> Result<Option<String>> {
    match kind {
        VAL_STRING => r.read_string16(),
        VAL_CHARSEQUENCE => char_sequence(r),
        _ => Ok(None),
    }
}

/// A bundle's entries (`BaseBundle.writeToParcelInner`): `visit` reads a
/// value it wants and returns true; the others are skipped. -1 is null.
fn walk_bundle(
    r: &mut Reader<'_>,
    visit: &mut dyn FnMut(&str, i32, &mut Reader<'_>) -> Result<bool>,
) -> Result<()> {
    let length = r.read_i32()?;
    if length <= 0 {
        return Ok(());
    }
    if !matches!(r.read_i32()?, BUNDLE_MAGIC | BUNDLE_MAGIC_NATIVE) {
        return Err(BAD_VALUE);
    }
    let end = r.position() + length as usize;
    for _ in 0..r.read_i32()?.max(0) {
        let key = r.read_string16()?.unwrap_or_default();
        let at = r.position();
        let kind = r.read_i32()?;
        if length_prefixed(kind) {
            let len = r.read_i32()?;
            let value_end = r.position() + len.max(0) as usize;
            // A value it cannot read is skipped by its length.
            let _ = visit(&key, kind, r);
            r.set_position(value_end);
        } else if !visit(&key, kind, r)? {
            r.set_position(at);
            skip_value(r)?;
        }
    }
    r.set_position(end);
    r.read_bool()?; // has an intent
    Ok(())
}

/// `writeParcelable`'s class name, then the parcelable, if it is a
/// `Bitmap` or an `Icon`.
fn parcelable_image(r: &mut Reader<'_>, files: &dyn Files) -> Result<Option<Icon>> {
    Ok(match r.read_string16()?.as_deref() {
        Some("android.graphics.Bitmap") => {
            Some(bitmap(r, files)?.map_or(Icon::Unreadable, Icon::Image))
        }
        Some("android.graphics.drawable.Icon") => Some(icon(r, files)?),
        _ => None,
    })
}

/// `Notification.extras`.
fn extras(r: &mut Reader<'_>, files: &dyn Files) -> Result<Extras> {
    let mut e = Extras::default();
    walk_bundle(r, &mut |key, kind, r| {
        let text = |r: &mut Reader<'_>, slot: &mut Option<String>| -> Result<bool> {
            *slot = text_value(r, kind)?;
            Ok(matches!(kind, VAL_STRING | VAL_CHARSEQUENCE))
        };
        match (key, kind) {
            ("android.title", _) => text(r, &mut e.title),
            ("android.text", _) => text(r, &mut e.text),
            ("android.subText", _) => text(r, &mut e.sub_text),
            ("android.bigText", _) => text(r, &mut e.big_text),
            ("android.conversationTitle", _) => text(r, &mut e.conversation_title),
            ("android.textLines", VAL_CHARSEQUENCEARRAY) => {
                for _ in 0..r.read_i32()?.max(0) {
                    e.lines.extend(char_sequence(r)?);
                }
                Ok(true)
            }
            ("android.picture" | "android.pictureIcon", VAL_PARCELABLE) => {
                e.picture = parcelable_image(r, files)?;
                Ok(true)
            }
            ("android.largeIcon", VAL_PARCELABLE) => {
                e.large_icon = parcelable_image(r, files)?;
                Ok(true)
            }
            ("android.messages", VAL_PARCELABLEARRAY) => {
                for _ in 0..r.read_i32()?.max(0) {
                    if r.read_string16()?.as_deref() != Some("android.os.Bundle") {
                        return Err(BAD_VALUE);
                    }
                    e.messages.push(message(r)?);
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    })?;
    Ok(e)
}

/// `Notification.MessagingStyle.Message.toBundle()`.
fn message(r: &mut Reader<'_>) -> Result<Message> {
    let mut m = Message::default();
    walk_bundle(r, &mut |key, kind, r| match (key, kind) {
        ("text", _) => {
            m.text = text_value(r, kind)?;
            Ok(matches!(kind, VAL_STRING | VAL_CHARSEQUENCE))
        }
        ("sender", _) if m.sender.is_none() => {
            m.sender = text_value(r, kind)?;
            Ok(matches!(kind, VAL_STRING | VAL_CHARSEQUENCE))
        }
        // A Person: its name comes first.
        ("sender_person", VAL_PARCELABLE) => {
            if r.read_string16()?.as_deref() == Some("android.app.Person") {
                m.sender = char_sequence(r)?;
            }
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(m)
}

/// `NotificationStats(Parcel)`: nothing the bridge uses.
pub struct NotificationStats;

impl aim_service_aidl::ReadParcelable for NotificationStats {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        // seen, expanded, direct replied, smart replied
        // (lifetime_extension_refactor), snoozed, viewed settings,
        // interacted; dismissal surface and sentiment.
        for _ in 0..9 {
            r.read_i32()?;
        }
        Ok(NotificationStats)
    }
}

/// `NotificationRankingUpdate(Parcel)` with `ranking_update_ashmem`: the
/// ranking map's shared memory (closed after the call) and the smart
/// actions. The bridge takes importance from the channel instead.
pub struct RankingUpdate;

impl aim_service_aidl::ReadParcelable for RankingUpdate {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        if r.read_string16()?.is_some() {
            r.read_fd()?; // SharedMemory
        }
        skip_bundle_or_null(r)?;
        Ok(RankingUpdate)
    }
}

/// `NotificationChannel(Parcel)`, up to its importance.
pub struct Channel {
    pub importance: i32,
}

impl aim_service_aidl::ReadParcelable for Channel {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        // id, name, description: each a byte flag, then the string.
        for _ in 0..3 {
            if r.read_i32()? != 0 {
                r.read_string16()?;
            }
        }
        Ok(Channel {
            importance: r.read_i32()?,
        })
    }
}

/// `ComponentName`.
pub struct ComponentName<'a>(pub &'a str, pub &'a str);

impl aim_service_aidl::WriteParcelable for ComponentName<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(self.0));
        p.write_string16(Some(self.1));
    }
}

/// `NotificationVisibility`: shown, in Notification Center.
pub struct Visibility<'a>(pub &'a str);

impl aim_service_aidl::WriteParcelable for Visibility<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(self.0));
        p.write_i32(0); // rank
        p.write_i32(1); // count
        p.write_i32(1); // visible
        p.write_string16(Some("LOCATION_UNKNOWN"));
    }
}

/// A bundle of `entries` written by `write`, as `writeToParcelInner`.
fn write_bundle(p: &mut Parcel, entries: usize, write: impl FnOnce(&mut Parcel)) {
    let length_at = p.position();
    p.write_i32(0);
    p.write_i32(BUNDLE_MAGIC);
    let start = p.position();
    p.write_i32(entries as i32);
    write(p);
    let length = (p.position() - start) as i32;
    p.set_i32_at(length_at, length);
    p.write_bool(false); // has an intent
}

/// `ActivityOptions` for sending a notification's `PendingIntent` as
/// SystemUI does: the sender's background activity start privileges
/// apply (`MODE_BACKGROUND_ACTIVITY_START_ALLOWED`).
pub struct SendOptions;

impl aim_service_aidl::WriteParcelable for SendOptions {
    fn write_to(&self, p: &mut Parcel) {
        write_bundle(p, 1, |p| {
            p.write_string16(Some("android.pendingIntent.backgroundActivityAllowed"));
            p.write_i32(VAL_INTEGER);
            p.write_i32(1);
        });
    }
}

/// The fill-in intent of a reply, as `RemoteInput.addResultsToIntent` and
/// `setResultsSource(SOURCE_FREE_FORM_INPUT)` make it: a clip whose
/// intent carries the results.
pub struct ReplyIntent<'a> {
    pub result_key: &'a str,
    pub text: &'a str,
}

/// An `Intent` with only `clip` and `extras` set.
fn write_intent(p: &mut Parcel, clip: Option<&dyn Fn(&mut Parcel)>, extras: &dyn Fn(&mut Parcel)) {
    p.write_string8(None); // action
    p.write_i32(0); // data
    p.write_string8(None); // type
    p.write_string8(None); // identifier
    p.write_i32(0); // flags
    p.write_i32(0); // extended flags
    p.write_string8(None); // package
    p.write_string16(None); // component
    p.write_i32(0); // source bounds
    p.write_i32(0); // categories
    p.write_i32(0); // selector
    match clip {
        Some(c) => {
            p.write_i32(1);
            c(p);
        }
        None => p.write_i32(0),
    }
    p.write_i32(-2); // content user hint: UserHandle.USER_CURRENT
    extras(p);
    p.write_i32(0); // original intent
    p.write_i32(0); // creator token
}

impl aim_service_aidl::WriteParcelable for ReplyIntent<'_> {
    fn write_to(&self, p: &mut Parcel) {
        let results = |p: &mut Parcel| {
            write_bundle(p, 2, |p| {
                p.write_string16(Some("android.remoteinput.resultsData"));
                p.write_i32(VAL_BUNDLE);
                write_bundle(p, 1, |p| {
                    p.write_string16(Some(self.result_key));
                    p.write_i32(VAL_CHARSEQUENCE);
                    write_char_sequence(p, Some(self.text));
                });
                p.write_string16(Some("android.remoteinput.resultsSource"));
                p.write_i32(VAL_INTEGER);
                p.write_i32(0); // SOURCE_FREE_FORM_INPUT
            })
        };
        let clip = |p: &mut Parcel| {
            // ClipDescription: label, MIME types, extras, timestamp,
            // styled, classification status, confidences.
            write_char_sequence(p, Some("android.remoteinput.results"));
            aim_service_aidl::write_string_list(p, Some(&[Some("text/vnd.android.intent".into())]));
            p.write_i32(-1);
            p.write_i64(0);
            p.write_bool(false);
            p.write_i32(2); // CLASSIFICATION_NOT_PERFORMED
            p.write_i32(0);
            p.write_i32(0); // icon
            p.write_i32(1);
            // The item: no text or HTML, the intent, no sender, URI,
            // activity info or links.
            write_char_sequence(p, None);
            p.write_string8(None);
            p.write_i32(1);
            write_intent(p, None, &results);
            for _ in 0..4 {
                p.write_i32(0);
            }
        };
        write_intent(p, Some(&clip), &|p| p.write_i32(-1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoFiles;

    impl Files for NoFiles {
        fn read(&self, _: u32, _: usize) -> Option<Vec<u8>> {
            None
        }
    }

    fn cs(p: &mut Parcel, s: &str) {
        write_char_sequence(p, Some(s));
    }

    /// A 1x1 RGBA bitmap in place, as `Bitmap_writeToParcel` writes it.
    fn java_bitmap(p: &mut Parcel) {
        p.write_i32(0);
        p.write_i32(RGBA_8888);
        p.write_i32(PREMUL);
        p.write_i32(-1);
        p.write_i32(1);
        p.write_i32(1);
        p.write_i32(4);
        p.write_i32(160);
        p.write_i64(7);
        p.write_i32(0);
        p.write_i32(4);
        p.write_i32(i32::from_le_bytes([1, 2, 3, 4]));
    }

    /// `ApplicationInfo.writeToParcel` of an app with one shared library,
    /// not squashed.
    fn java_application_info(p: &mut Parcel) {
        p.write_i32(0); // squashing: the first copy
        p.write_string8(Some("com.example.App"));
        p.write_string8(Some("com.example"));
        p.write_i32(0x7f0a_0000);
        cs(p, "Example");
        p.write_i32(0x7f08_0000);
        p.write_i32(0);
        p.write_i32(-1); // meta-data
        for _ in 0..3 {
            p.write_i32(0);
        }
        for s in [Some("com.example"), None, Some("com.example"), None] {
            p.write_string8(s);
        }
        p.write_i32(0x7f0f_0001); // theme
        for _ in 0..6 {
            p.write_i32(0);
        }
        p.write_i32(1);
        p.write_i64(1);
        p.write_i64(2);
        for _ in 0..2 {
            p.write_string8(None);
        }
        p.write_string8(Some("/data/app/~~a/com.example-b/base.apk"));
        p.write_string8(Some("/data/app/~~a/com.example-b/base.apk"));
        p.write_i32(1);
        p.write_string8(Some("config.en"));
        p.write_i32(-1);
        p.write_i32(-1);
        p.write_i32(1); // split dependencies: {1: [0]}
        p.write_i32(1);
        p.write_i32(VAL_INTARRAY);
        p.write_i32(1);
        p.write_i32(0);
        for _ in 0..3 {
            p.write_string8(Some("/lib"));
        }
        p.write_i32(0);
        p.write_string8(Some("arm64-v8a"));
        p.write_string8(None);
        p.write_i32(-1);
        p.write_i32(0);
        p.write_string8(Some("default:targetSdkVersion=36"));
        p.write_string8(None);
        p.write_i32(-1);
        p.write_i32(1); // a shared library
        p.write_i32(1);
        p.write_string8(Some("/system/framework/lib.jar"));
        p.write_string8(None);
        p.write_i32(1);
        p.write_i32(1);
        p.write_string8(Some("/system/framework/lib.jar"));
        p.write_string8(Some("lib"));
        p.write_i64(-1);
        p.write_i32(0);
        p.write_string16(Some("android.content.pm.VersionedPackage"));
        p.write_string8(Some("android"));
        p.write_i64(36);
        p.write_i32(-1); // dependent packages
        p.write_i32(-1); // dependencies
        p.write_bool(false);
        p.write_i32(0);
        p.write_i32(-1);
        p.write_i32(-1); // optional shared libraries
        for _ in 0..3 {
            p.write_string8(Some("/data/user/0/com.example"));
        }
        for v in [10_123, 24, 36] {
            p.write_i32(v);
        }
        p.write_i64(3);
        for _ in 0..3 {
            p.write_i32(1);
        }
        p.write_string8(None);
        p.write_string8(None);
        for _ in 0..8 {
            p.write_i32(0);
        }
        p.write_string8(None);
        p.write_i32(-1);
        p.write_i32(36);
        p.write_string8(Some("REL"));
        p.write_string8(None);
        for _ in 0..4 {
            p.write_i32(0);
        }
        p.write_string8(None);
        for _ in 0..3 {
            p.write_i32(0);
        }
        p.write_i32(1); // raw external storage access: unset
        p.write_i64(5);
        p.write_i32(1);
        p.write_string16(Some("com.example"));
        p.write_string16(Some("com.example.App"));
        for _ in 0..3 {
            p.write_i32(0);
        }
        p.write_i32(-1);
    }

    /// A custom view as `RemoteViews.writeToParcel` writes one with
    /// `setTextViewText` and `setImageViewBitmap`: the bitmap in the root's
    /// cache, the application info in full (or, `squashed`, as an offset
    /// back to an earlier copy).
    fn java_remote_views(p: &mut Parcel, squashed: bool) {
        p.write_i32(0); // MODE_NORMAL
        p.write_i32(1); // bitmap cache
        p.write_i32(1);
        java_bitmap(p);
        p.write_i32(0); // collection cache
        p.write_i32(1);
        if squashed {
            p.write_i32(400);
        } else {
            java_application_info(p);
        }
        p.write_i32(0); // ideal size
        p.write_i32(0x7f0c_0001);
        p.write_i32(-1);
        p.write_i32(0);
        p.write_i32(2); // actions
        p.write_i32(2); // ReflectionAction: setText, a CharSequence
        p.write_i32(0x7f09_0001);
        p.write_string8(Some("setText"));
        p.write_i32(10);
        cs(p, "Custom");
        p.write_i32(12); // BitmapReflectionAction: setImageBitmap, bitmap 0
        p.write_i32(0x7f09_0002);
        p.write_string8(Some("setImageBitmap"));
        p.write_i32(0);
        p.write_i32(0); // apply flags
        p.write_i64(-1);
        p.write_bool(false);
    }

    /// A notification as `Notification.writeToParcelImpl` writes a
    /// builder's: a resource small icon, a content intent, a bitmap large
    /// icon in place, big text and messages in its extras, one action
    /// with a remote input, a channel; with `custom`, custom content and
    /// big content views (`DecoratedCustomViewStyle`).
    fn java_notification(p: &mut Parcel, intent: Binder, custom: bool) {
        p.write_i32(1);
        p.write_binder(None); // allowlist token
        p.write_i64(1);
        p.write_i64(2);
        p.write_i32(1); // small icon: a resource
        p.write_i32(2);
        p.write_string16(Some("com.example"));
        p.write_i32(0x7f01_0001);
        p.write_bool(false);
        p.write_f32(0.0);
        p.write_i32(0); // no tint
        p.write_i32(9); // blend mode
        p.write_i32(0); // number
        p.write_i32(1);
        p.write_binder(Some(intent));
        p.write_i32(0); // delete intent
        p.write_i32(0); // ticker
        p.write_i32(0); // ticker view
        p.write_i32(custom as i32); // content view
        if custom {
            java_remote_views(p, false);
        }
        p.write_i32(1); // large icon: a 1x1 RGBA bitmap in place
        p.write_i32(1);
        java_bitmap(p);
        p.write_i32(0);
        p.write_i32(3);
        p.write_i32(0); // defaults
        p.write_i32(FLAG_ONLY_ALERT_ONCE);
        p.write_i32(0); // sound
        p.write_i32(-1);
        p.write_i32(1); // audio attributes
        for v in [5, 0, 0, 0, 0] {
            p.write_i32(v);
        }
        aim_service_aidl::write_string_list(p, Some(&[]));
        p.write_i32(-1977);
        p.write_i32(-1); // vibrate
        for _ in 0..4 {
            p.write_i32(0);
        }
        p.write_i32(0); // full-screen intent
        p.write_i32(0); // priority
        p.write_string8(Some("msg"));
        p.write_string8(Some("group"));
        p.write_string8(None);
        write_bundle(p, 4, |p| {
            p.write_string16(Some("android.title"));
            p.write_i32(VAL_CHARSEQUENCE);
            p.write_i32(0); // spanned, one StyleSpan
            p.write_string8(Some("Title"));
            p.write_i32(7);
            p.write_i32(1);
            p.write_i32(0);
            for v in [0, 5, 0x11] {
                p.write_i32(v);
            }
            p.write_i32(0);
            p.write_string16(Some("android.bigText"));
            p.write_i32(VAL_STRING);
            p.write_string16(Some("Big text"));
            p.write_string16(Some("android.extensions"));
            p.write_i32(VAL_PARCELABLE);
            p.write_i32(8);
            p.write_string16(Some("x"));
            p.write_string16(Some("android.messages"));
            p.write_i32(VAL_PARCELABLEARRAY);
            let length_at = p.position();
            p.write_i32(0);
            let start = p.position();
            p.write_i32(1);
            p.write_string16(Some("android.os.Bundle"));
            write_bundle(p, 2, |p| {
                p.write_string16(Some("text"));
                p.write_i32(VAL_CHARSEQUENCE);
                cs(p, "hi");
                p.write_string16(Some("time"));
                p.write_i32(VAL_LONG);
                p.write_i64(3);
            });
            let len = (p.position() - start) as i32;
            p.set_i32_at(length_at, len);
        });
        p.write_i32(1); // actions
        p.write_i32(1);
        p.write_i32(0); // no icon
        cs(p, "Reply");
        p.write_i32(1);
        p.write_binder(Some(intent));
        p.write_i32(0); // extras: empty
        p.write_i32(1);
        p.write_i32(1);
        p.write_string16(Some("reply_key"));
        cs(p, "Message");
        p.write_i32(-1);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(-1);
        p.write_i32(-1);
        for _ in 0..4 {
            p.write_i32(0);
        }
        p.write_i32(custom as i32); // big content view
        if custom {
            java_remote_views(p, true);
        }
        p.write_i32(0); // heads-up view
        p.write_i32(0); // visibility
        p.write_i32(0); // public version
        p.write_i32(0); // color
        p.write_i32(1);
        p.write_string8(Some("chat"));
        p.write_i64(0);
        p.write_i32(0); // shortcut id
        p.write_i32(0); // locus id
        p.write_i32(0); // badge icon
        p.write_i32(0); // settings text
        p.write_i32(0); // group alert behavior
        p.write_i32(0); // bubble metadata
        p.write_bool(true);
        p.write_i32(0); // FGS defer behavior
        // allPendingIntents: the content intent, twice (an ArraySet of
        // parcelables).
        p.write_i32(2);
        for _ in 0..2 {
            p.write_i32(VAL_PARCELABLE);
            let length_at = p.position();
            p.write_i32(0);
            let start = p.position();
            p.write_string16(Some("android.app.PendingIntent"));
            p.write_binder(Some(intent));
            let len = (p.position() - start) as i32;
            p.set_i32_at(length_at, len);
        }
    }

    /// A `StatusBarNotification` of `java_notification`.
    fn java_sbn(intent: Binder, custom: bool) -> Parcel {
        let mut p = Parcel::new();
        p.write_string16(Some("com.example"));
        p.write_string16(Some("com.example"));
        p.write_i32(5);
        p.write_i32(0); // no tag
        p.write_i32(10_123);
        p.write_i32(99);
        java_notification(&mut p, intent, custom);
        p.write_i32(10); // user
        p.write_i64(4); // post time
        p.write_i32(0); // override group key
        p.write_i32(0); // instance id
        p
    }

    #[test]
    fn reads_a_builder_notification() {
        let intent = Binder::Local(0x10);
        let p = java_sbn(intent, false);
        let mut r = Reader::new(p.data(), p.objects());
        let sbn = StatusBarNotification::read(&mut r, &NoFiles).unwrap();
        let n = &sbn.notification;
        assert!(n.complete);
        assert_eq!(sbn.key(), "10|com.example|5|null|10123");
        assert_eq!(r.remaining(), 0);
        assert_eq!(n.content_intent, Some(intent));
        assert_eq!(n.flags, FLAG_ONLY_ALERT_ONCE);
        assert_eq!(n.group.as_deref(), Some("group"));
        assert_eq!(n.channel_id.as_deref(), Some("chat"));
        assert_eq!(n.extras.title.as_deref(), Some("Title"));
        assert_eq!(n.extras.big_text.as_deref(), Some("Big text"));
        assert_eq!(n.extras.messages[0].text.as_deref(), Some("hi"));
        assert_eq!(
            n.large_icon,
            Some(Icon::Image(Image::Rgba {
                width: 1,
                height: 1,
                premultiplied: true,
                pixels: vec![1, 2, 3, 4],
            }))
        );
        assert_eq!(n.actions.len(), 1);
        assert_eq!(n.actions[0].title.as_deref(), Some("Reply"));
        assert_eq!(n.actions[0].inputs[0].result_key, "reply_key");
        assert_eq!(n.binders().count(), 2);
    }

    #[test]
    fn reads_a_data_icon_in_place() {
        let png = [0x89, b'P', b'N', b'G', 1];
        let mut p = Parcel::new();
        p.write_i32(3); // TYPE_DATA
        p.write_i32(png.len() as i32);
        p.write_i32(png.len() as i32); // writeBlob: its length,
        p.write_i32(0); // BLOB_INPLACE,
        p.write_raw(&png, &[]); // then the bytes, padded
        p.write_i32(0); // no tint
        p.write_i32(9);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(
            icon(&mut r, &NoFiles).unwrap(),
            Icon::Image(Image::Encoded(png.to_vec()))
        );
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn reads_past_custom_views() {
        let p = java_sbn(Binder::Local(0x10), true);
        let mut r = Reader::new(p.data(), p.objects());
        let sbn = StatusBarNotification::read(&mut r, &NoFiles).unwrap();
        let n = &sbn.notification;
        assert!(n.complete);
        assert_eq!(r.remaining(), 0);
        assert_eq!(n.extras.title.as_deref(), Some("Title"));
        assert_eq!(n.channel_id.as_deref(), Some("chat"));
        assert_eq!(n.actions.len(), 1);
    }

    #[test]
    fn reads_an_application_info() {
        let mut p = Parcel::new();
        java_application_info(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(
            application_info(&mut r).unwrap(),
            Some(ApplicationInfo {
                source_dir: Some("/data/app/~~a/com.example-b/base.apk".into()),
                theme: 0x7f0f_0001,
            })
        );
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn keeps_what_precedes_an_unknown_view_action() {
        let mut p = Parcel::new();
        p.write_i32(1);
        p.write_binder(None);
        p.write_i64(1);
        p.write_i64(2);
        p.write_i32(0); // small icon
        p.write_i32(0);
        p.write_i32(0); // content intent
        p.write_i32(0);
        p.write_i32(1);
        cs(&mut p, "ticker");
        p.write_i32(0);
        p.write_i32(1); // a content view with an action of tag 99
        for v in [0, 0, 0, 0, 0, -1, 0, 0, 1, 99] {
            p.write_i32(v);
        }
        let mut r = Reader::new(p.data(), p.objects());
        let mut n = Notification::default();
        assert!(read_notification(&mut r, &NoFiles, &mut n).is_err());
        assert!(!n.complete);
        assert_eq!(n.ticker.as_deref(), Some("ticker"));
    }

    #[test]
    fn writes_a_reply_intent_that_reads_back() {
        let mut p = Parcel::new();
        aim_service_aidl::WriteParcelable::write_to(
            &ReplyIntent {
                result_key: "k",
                text: "hello",
            },
            &mut p,
        );
        let mut r = Reader::new(p.data(), p.objects());
        crate::clip::intent(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
    }
}
