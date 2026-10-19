//! A media session for the app host's `media_session`: MediaSessionRecord
//! at `android-16.0.0_r1`, its `ISession` (what the owning app's
//! `MediaSession` sets: flags, activeness, the media button receiver, the
//! launch intent, metadata (its art URIs the app may not grant dropped),
//! playback state, queue (`getBinderForSetQueue`'s `ParcelableListBinder`),
//! queue title, extras, rating type and the playback type) and its
//! `ISessionController` (what controllers read, their callbacks told of
//! each change as its handler pushes it, and the transport controls sent to
//! the session's `ISessionCallback` with the controller's package, pid and
//! uid). The parcelables are kept as they came, with their binders and
//! files. Media buttons sent by a controller, volume adjustments and the
//! volume of a local session whose attributes are not the default media
//! ones are not implemented (#1793).
//!
//! The temporary allowlisting of the session's app for a controller's
//! command (`tempAllowlistTargetPkgIfPossible`) checks the controller's
//! package and lifts nothing: every app host process is top (#1738).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_BAD_PARCELABLE, EX_ILLEGAL_STATE, EX_SECURITY, Exception, Parcel, Reader,
    Result as ParcelResult,
};
use aim_service_aidl::android_media_session_isession as session;
use aim_service_aidl::android_media_session_isessioncallback as session_cb;
use aim_service_aidl::android_media_session_isessioncontroller as controller;
use aim_service_aidl::android_media_session_isessioncontrollercallback as controller_cb;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::content_service::ParcelUri;
use super::kept::{KeptBundle, RawBundle};
use super::media_session::{
    MediaSession, ReceiverHolder, THROW_FOR_ACTIVITY_MEDIA_BUTTON_RECEIVER,
    THROW_FOR_INVALID_BROADCAST_RECEIVER,
};
use super::pending_intents::INTENT_SENDER_ACTIVITY;
use super::playback::Attributes;
use super::{AppHost, reply};
use crate::clip::{self, Raw};
use crate::package::intent::ComponentName;
use crate::package::list_slice::Slice;

/// `MediaSession.FLAG_EXCLUSIVE_GLOBAL_PRIORITY`.
pub(super) const FLAG_EXCLUSIVE_GLOBAL_PRIORITY: i64 = 1 << 16;
/// `PlaybackState.STATE_*`.
const STATE_NONE: i32 = 0;
const STATE_PLAYING: i32 = 3;
const STATE_FAST_FORWARDING: i32 = 4;
const STATE_REWINDING: i32 = 5;
const STATE_BUFFERING: i32 = 6;
const STATE_CONNECTING: i32 = 8;
const STATE_SKIPPING_TO_PREVIOUS: i32 = 9;
const STATE_SKIPPING_TO_NEXT: i32 = 10;
const STATE_SKIPPING_TO_QUEUE_ITEM: i32 = 11;
/// `ALWAYS_PRIORITY_STATES`, `TRANSITION_PRIORITY_STATES`.
const ALWAYS_PRIORITY_STATES: [i32; 4] = [
    STATE_FAST_FORWARDING,
    STATE_REWINDING,
    STATE_SKIPPING_TO_PREVIOUS,
    STATE_SKIPPING_TO_NEXT,
];
const TRANSITION_PRIORITY_STATES: [i32; 3] = [STATE_BUFFERING, STATE_CONNECTING, STATE_PLAYING];
/// `PlaybackInfo.PLAYBACK_TYPE_*`, `VolumeProvider.VOLUME_CONTROL_ABSOLUTE`.
const PLAYBACK_TYPE_LOCAL: i32 = 1;
const PLAYBACK_TYPE_REMOTE: i32 = 2;
const VOLUME_CONTROL_ABSOLUTE: i32 = 2;
/// `AudioAttributes.USAGE_MEDIA`.
const USAGE_MEDIA: i32 = 1;
/// `MediaMetadata`'s art URIs (`ART_URIS`).
const ART_URIS: [&str; 3] = [
    "android.media.metadata.ALBUM_ART_URI",
    "android.media.metadata.ART_URI",
    "android.media.metadata.DISPLAY_ICON_URI",
];
/// `Intent.FLAG_GRANT_READ_URI_PERMISSION`.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 1;
/// `ParcelableListBinder`'s codes.
const FIRST_CALL_TRANSACTION: u32 = 1;
const END_OF_PARCEL: i32 = 0;
const QUEUE_ITEM: &str = "android.media.session.MediaSession$QueueItem";

/// A parcelable kept past its call: its bytes, with the binders among
/// them held and the files of its fds kept.
#[derive(Clone)]
pub(super) struct Held {
    raw: Raw,
    _binders: Arc<Vec<Strong>>,
}

impl Held {
    fn new(mut raw: Raw, process: &LocalProcess) -> ParcelResult<Held> {
        raw.keep_files(&|fd| process.file(fd))?;
        let binders = raw.handles().map(|h| process.strong(h)).collect();
        Ok(Held {
            raw,
            _binders: Arc::new(binders),
        })
    }
}

impl WriteParcelable for Held {
    fn write_to(&self, p: &mut Parcel) {
        self.raw.write(p);
    }
}

/// The bytes `read` reads, captured.
fn captured(
    r: &mut Reader<'_>,
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<()>,
) -> ParcelResult<Raw> {
    let start = r.position();
    read(r)?;
    Ok(Raw::capture(r, start))
}

/// A parcelable passed on as it came.
pub(super) struct In(Raw);

/// A `CharSequence` (`TextUtils.writeToParcel`).
pub(super) struct Text(Raw);

impl ReadParcelable for Text {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        captured(r, |r| clip::char_sequence(r).map(|_| ())).map(Text)
    }
}

/// A `PendingIntent` or a `ResultReceiver`: its binder.
pub(super) struct BinderIn(pub(super) Option<Binder>);

impl ReadParcelable for BinderIn {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        r.read_binder().map(BinderIn)
    }
}

impl WriteParcelable for BinderIn {
    fn write_to(&self, p: &mut Parcel) {
        p.write_binder(self.0);
    }
}

/// A `Rating`: its style and value.
impl ReadParcelable for In {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        captured(r, |r| {
            r.read_i32()?;
            r.read_f32()?;
            Ok(())
        })
        .map(In)
    }
}

impl WriteParcelable for In {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write(p);
    }
}

impl WriteParcelable for Text {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write(p);
    }
}

/// A `MediaMetadata`: its bundle and bitmap dimension limit.
pub(super) struct Metadata {
    bundle: RawBundle,
    limit: i32,
}

impl ReadParcelable for Metadata {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Metadata {
            bundle: RawBundle::read_from(r)?,
            limit: r.read_i32()?,
        })
    }
}

impl WriteParcelable for Metadata {
    fn write_to(&self, p: &mut Parcel) {
        self.bundle.write_to(p);
        p.write_i32(self.limit);
    }
}

/// A `PlaybackState`, and of it what the record reads: its state,
/// position, speed and update time (the fields that open it).
pub(super) struct State {
    raw: Raw,
    state: i32,
    position: i64,
    speed: f32,
    update_time: i64,
}

impl ReadParcelable for State {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let state = r.read_i32()?;
        let position = r.read_i64()?;
        let speed = r.read_f32()?;
        let update_time = r.read_i64()?;
        r.read_i64()?; // buffered position
        r.read_i64()?; // actions
        // `CustomAction`s.
        for _ in 0..r.read_i32()?.max(0) {
            if r.read_i32()? != 0 {
                r.read_string16()?;
                clip::char_sequence(r)?;
                r.read_i32()?;
                clip::bundle(r)?;
            }
        }
        r.read_i64()?; // active item id
        clip::char_sequence(r)?;
        clip::bundle(r)?;
        Ok(State {
            raw: Raw::capture(r, start),
            state,
            position,
            speed,
            update_time,
        })
    }
}

/// A kept `PlaybackState`.
#[derive(Clone)]
struct KeptState {
    held: Held,
    state: i32,
    position: i64,
    speed: f32,
    update_time: i64,
}

impl KeptState {
    /// `PlaybackState.isActive`.
    fn is_active(&self) -> bool {
        matches!(
            self.state,
            STATE_FAST_FORWARDING
                | STATE_REWINDING
                | STATE_SKIPPING_TO_PREVIOUS
                | STATE_SKIPPING_TO_NEXT
                | STATE_SKIPPING_TO_QUEUE_ITEM
                | STATE_BUFFERING
                | STATE_CONNECTING
                | STATE_PLAYING
        )
    }

    /// `new Builder(state).setState(state, position, speed, updateTime)`.
    fn at(&self, position: i64, update_time: i64) -> KeptState {
        let mut held = self.held.clone();
        for (at, value) in [(4, position), (16, update_time)] {
            held.raw.set_i32(at, value as i32);
            held.raw.set_i32(at + 4, (value >> 32) as i32);
        }
        KeptState {
            held,
            position,
            update_time,
            ..*self
        }
    }
}

/// A `MediaSession.QueueItem` as `writeParcelable` writes it: its class,
/// its `MediaDescription` and id; a description's icon bitmap is not
/// read (#1793).
pub(super) struct QueueItemIn(Option<Raw>);

/// `writeParcelable` of a `Uri` or null.
fn parcelable_uri(r: &mut Reader<'_>) -> ParcelResult<()> {
    if r.read_string16()?.is_some() {
        clip::uri(r)?;
    }
    Ok(())
}

impl QueueItemIn {
    /// `readParcelable`: none for an item with an icon, which is not read.
    fn read(r: &mut Reader<'_>) -> ParcelResult<QueueItemIn> {
        if r.read_string16()?.as_deref() != Some(QUEUE_ITEM) {
            return Err(BAD_VALUE);
        }
        let start = r.position();
        r.read_string16()?; // media id
        for _ in 0..3 {
            clip::char_sequence(r)?;
        }
        if r.read_string16()?.is_some() {
            return Ok(QueueItemIn(None));
        }
        parcelable_uri(r)?; // icon URI
        clip::bundle(r)?;
        parcelable_uri(r)?; // media URI
        r.read_i64()?;
        Ok(QueueItemIn(Some(Raw::capture(r, start))))
    }
}

/// `MediaSession.Token`: the owner's uid and the controller.
#[derive(Clone, Copy)]
pub(super) struct Token {
    pub(super) uid: i32,
    pub(super) controller: Binder,
}

impl WriteParcelable for Token {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.uid);
        p.write_binder(Some(self.controller));
    }
}

/// `MediaController.PlaybackInfo`.
struct PlaybackInfo {
    kind: i32,
    control: i32,
    max: i32,
    current: i32,
    attributes: Attributes,
    control_id: Option<String>,
}

impl WriteParcelable for PlaybackInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.kind);
        p.write_i32(self.control);
        p.write_i32(self.max);
        p.write_i32(self.current);
        p.write_string16(Some("android.media.AudioAttributes"));
        self.attributes.write_to(p);
        p.write_string16(self.control_id.as_deref());
    }
}

/// A controller's callback (`ISessionControllerCallbackHolder`).
struct Holder {
    callback: Strong,
    death: u64,
}

/// What the record keeps, under its lock.
struct Fields {
    flags: i64,
    receiver: Option<ReceiverHolder>,
    launch_intent: Option<BinderIn>,
    launch_intent_held: Option<Strong>,
    extras: Option<KeptBundle>,
    metadata: Option<Held>,
    playback_state: Option<KeptState>,
    queue: Option<Vec<Held>>,
    queue_title: Option<Held>,
    rating_type: i32,
    audio_attrs: Attributes,
    volume_type: i32,
    volume_control_type: i32,
    max_volume: i32,
    current_volume: i32,
    optimistic_volume: i32,
    volume_control_id: Option<String>,
    active: bool,
    destroyed: bool,
    duration: i64,
    metadata_description: Option<String>,
    holders: Vec<Holder>,
    callback_death: Option<u64>,
}

/// `MediaSessionRecord`.
pub(super) struct Record {
    pub(super) owner_uid: i32,
    pub(super) user: i32,
    pub(super) package: String,
    tag: Option<String>,
    session_info: Option<KeptBundle>,
    callback: Strong,
    service: Weak<MediaSession>,
    host: Arc<AppHost>,
    controller: OnceLock<Binder>,
    session: OnceLock<Binder>,
    fields: Mutex<Fields>,
}

impl Record {
    /// A record of `callback`'s session, its binders served, linked to the
    /// callback's death.
    pub(super) fn new(
        service: &Arc<MediaSession>,
        host: &Arc<AppHost>,
        owner: (i32, i32),
        package: String,
        callback: u32,
        tag: Option<String>,
        session_info: Option<KeptBundle>,
    ) -> Arc<Record> {
        let (owner_uid, user) = owner;
        let record = Arc::new(Record {
            owner_uid,
            user,
            package,
            tag,
            session_info,
            callback: host.process.strong(callback),
            service: Arc::downgrade(service),
            host: host.clone(),
            controller: OnceLock::new(),
            session: OnceLock::new(),
            fields: Mutex::new(Fields {
                flags: 0,
                receiver: None,
                launch_intent: None,
                launch_intent_held: None,
                extras: None,
                metadata: None,
                playback_state: None,
                queue: None,
                queue_title: None,
                rating_type: 0,
                audio_attrs: Attributes::built(USAGE_MEDIA, 0, 0),
                volume_type: PLAYBACK_TYPE_LOCAL,
                volume_control_type: VOLUME_CONTROL_ABSOLUTE,
                max_volume: 0,
                current_volume: 0,
                optimistic_volume: -1,
                volume_control_id: None,
                active: false,
                destroyed: false,
                duration: -1,
                metadata_description: None,
                holders: Vec::new(),
                callback_death: None,
            }),
        });
        let _ = record.controller.set(
            host.process
                .add_service(Arc::new(ControllerStub(record.clone()))),
        );
        let _ = record.session.set(
            host.process
                .add_service(Arc::new(SessionStub(record.clone()))),
        );
        let weak = Arc::downgrade(&record);
        let death = host.process.link_to_death(
            &record.callback,
            Box::new(move || {
                if let Some(record) = weak.upgrade()
                    && let Some(service) = record.service.upgrade()
                {
                    service.on_session_died(&record);
                }
            }),
        );
        record.fields.lock().unwrap().callback_death = Some(death);
        record
    }

    pub(super) fn session_binder(&self) -> Binder {
        *self.session.get().unwrap()
    }

    /// `getSessionToken`.
    pub(super) fn token(&self) -> Token {
        Token {
            uid: self.owner_uid,
            controller: *self.controller.get().unwrap(),
        }
    }

    /// `isActive`.
    pub(super) fn is_active(&self) -> bool {
        let f = self.fields.lock().unwrap();
        f.active && !f.destroyed
    }

    /// `checkPlaybackActiveState`.
    pub(super) fn check_playback_active_state(&self, expected: bool) -> bool {
        self.fields
            .lock()
            .unwrap()
            .playback_state
            .as_ref()
            .is_some_and(|s| s.is_active() == expected)
    }

    /// `MediaSessionService.isUserEngaged` of the record and its playback
    /// state: an active state of an active session.
    pub(super) fn is_user_engaged(&self) -> bool {
        let f = self.fields.lock().unwrap();
        f.playback_state.as_ref().is_some_and(|s| s.is_active()) && f.active && !f.destroyed
    }

    /// `isSystemPriority`.
    pub(super) fn is_system_priority(&self) -> bool {
        self.fields.lock().unwrap().flags & FLAG_EXCLUSIVE_GLOBAL_PRIORITY != 0
    }

    /// `isClosed`.
    pub(super) fn is_closed(&self) -> bool {
        self.fields.lock().unwrap().destroyed
    }

    /// `getMediaButtonReceiver`.
    pub(super) fn media_button_receiver(&self) -> Option<ReceiverHolder> {
        self.fields.lock().unwrap().receiver.clone()
    }

    /// `close`: unlinked from the callback, destroyed, its playback state
    /// gone, and its controllers told.
    pub(super) fn close(self: &Arc<Self>) {
        {
            let mut f = self.fields.lock().unwrap();
            if f.destroyed {
                return;
            }
            if let Some(death) = f.callback_death.take() {
                self.host.process.clear_death(&self.callback, death);
            }
            f.destroyed = true;
            f.playback_state = None;
        }
        let me = self.clone();
        self.post(move || me.push_session_destroyed());
    }

    fn post(&self, job: impl FnOnce() + Send + 'static) {
        if let Some(service) = self.service.upgrade() {
            service.post(Box::new(job));
        }
    }

    /// `performOnCallbackHolders`: `code` with `args` to each controller
    /// callback, the dead ones dropped.
    fn perform(&self, code: u32, args: impl Fn(&mut Parcel)) {
        let handles: Vec<u32> = self
            .fields
            .lock()
            .unwrap()
            .holders
            .iter()
            .map(|h| h.callback.handle)
            .collect();
        let mut dead = Vec::new();
        for handle in handles {
            let mut p = Parcel::new();
            p.write_interface_token(controller_cb::DESCRIPTOR);
            args(&mut p);
            if self.host.process.transact(handle, code, &p, true).is_err() {
                dead.push(handle);
            }
        }
        let mut f = self.fields.lock().unwrap();
        for handle in dead {
            if let Some(i) = f.holders.iter().position(|h| h.callback.handle == handle) {
                let h = f.holders.remove(i);
                self.host.process.clear_death(&h.callback, h.death);
            }
        }
    }

    /// The push of what `pick` reads, unless destroyed.
    fn push<T: Clone>(
        &self,
        code: u32,
        pick: impl FnOnce(&Fields) -> T,
        write: impl Fn(&mut Parcel, &T),
    ) {
        let value = {
            let f = self.fields.lock().unwrap();
            if f.destroyed {
                return;
            }
            pick(&f)
        };
        self.perform(code, |p| write(p, &value));
    }

    fn push_playback_state(&self) {
        self.push(
            controller_cb::ON_PLAYBACK_STATE_CHANGED,
            |f| f.playback_state.as_ref().map(|s| s.held.clone()),
            |p, s| aim_service_aidl::write_typed(p, s.as_ref()),
        );
    }

    fn push_metadata(&self) {
        self.push(
            controller_cb::ON_METADATA_CHANGED,
            |f| f.metadata.clone(),
            |p, m| aim_service_aidl::write_typed(p, m.as_ref()),
        );
    }

    fn push_queue(&self) {
        let process = self.host.process.clone();
        self.push(
            controller_cb::ON_QUEUE_CHANGED,
            |f| f.queue.clone(),
            move |p, queue| {
                // Each callback gets a slice of its own, one item inline.
                let slice = queue
                    .clone()
                    .map(|q| Slice::new(process.clone(), QUEUE_ITEM, q).with_inline_limit(1));
                aim_service_aidl::write_typed(p, slice.as_ref());
            },
        );
    }

    fn push_queue_title(&self) {
        self.push(
            controller_cb::ON_QUEUE_TITLE_CHANGED,
            |f| f.queue_title.clone(),
            |p, t| aim_service_aidl::write_typed(p, t.as_ref()),
        );
    }

    fn push_extras(&self) {
        self.push(
            controller_cb::ON_EXTRAS_CHANGED,
            |f| f.extras.clone(),
            |p, e| aim_service_aidl::write_typed(p, e.as_ref().map(|e| &e.bundle)),
        );
    }

    fn push_volume(self: &Arc<Self>) {
        if self.is_closed() {
            return;
        }
        match self.volume_attributes() {
            Ok(info) => self.perform(controller_cb::ON_VOLUME_INFO_CHANGED, |p| {
                aim_service_aidl::write_typed(p, Some(&info))
            }),
            Err(e) => eprintln!("app host: media_session: pushVolumeUpdate: {}", e.message),
        }
    }

    fn push_event(&self, event: Option<String>, data: Option<KeptBundle>) {
        if self.is_closed() {
            return;
        }
        self.perform(controller_cb::ON_EVENT, |p| {
            p.write_string16(event.as_deref());
            aim_service_aidl::write_typed(p, data.as_ref().map(|d| &d.bundle));
        });
    }

    /// `pushSessionDestroyed`: each callback unlinked and told, then
    /// forgotten.
    fn push_session_destroyed(&self) {
        let holders = {
            let mut f = self.fields.lock().unwrap();
            if !f.destroyed {
                return;
            }
            std::mem::take(&mut f.holders)
        };
        for h in holders {
            self.host.process.clear_death(&h.callback, h.death);
            let mut p = Parcel::new();
            p.write_interface_token(controller_cb::DESCRIPTOR);
            let _ = h
                .callback
                .transact(controller_cb::ON_SESSION_DESTROYED, &p, true);
        }
    }

    /// `getVolumeAttributes`.
    fn volume_attributes(&self) -> Result<PlaybackInfo, Exception> {
        let (kind, attributes) = {
            let f = self.fields.lock().unwrap();
            if f.volume_type == PLAYBACK_TYPE_REMOTE {
                let current = if f.optimistic_volume != -1 {
                    f.optimistic_volume
                } else {
                    f.current_volume
                };
                return Ok(PlaybackInfo {
                    kind: f.volume_type,
                    control: f.volume_control_type,
                    max: f.max_volume,
                    current,
                    attributes: f.audio_attrs.clone(),
                    control_id: f.volume_control_id.clone(),
                });
            }
            (f.volume_type, f.audio_attrs.clone())
        };
        // `getVolumeStream`: the media attributes' stream is the music
        // stream, as the audio policy's product strategies map them.
        if attributes.public_usage() != USAGE_MEDIA || attributes.flags() != 0 {
            return Err(self.host.unimplemented_named(
                "media_session",
                "the volume stream of other audio attributes (#1793)",
            ));
        }
        let (current, max, _) = self.host.music_volume()?;
        Ok(PlaybackInfo {
            kind,
            control: VOLUME_CONTROL_ABSOLUTE,
            max,
            current,
            attributes,
            control_id: None,
        })
    }

    /// `getStateWithUpdatedPosition`.
    fn state_with_updated_position(&self) -> Option<Held> {
        let (state, duration) = {
            let f = self.fields.lock().unwrap();
            if f.destroyed {
                return None;
            }
            (f.playback_state.clone()?, f.duration)
        };
        if matches!(
            state.state,
            STATE_PLAYING | STATE_FAST_FORWARDING | STATE_REWINDING
        ) && state.update_time > 0
        {
            let now = super::alarm_service::elapsed_ms();
            let mut position =
                (state.speed * (now - state.update_time) as f32) as i64 + state.position;
            if duration >= 0 && position > duration {
                position = duration;
            } else if position < 0 {
                position = 0;
            }
            return Some(state.at(position, now).held);
        }
        Some(state.held)
    }

    /// `SessionCb`'s call `code` to the session, after
    /// `tempAllowlistTargetPkgIfPossible`: its controller's package, pid
    /// and uid, then what `args` writes.
    fn session_call(
        &self,
        call: &Call<'_>,
        code: u32,
        package: Option<&str>,
        args: impl FnOnce(&mut Parcel),
    ) -> Result<(), Exception> {
        let (pid, uid) = (call.sender_pid, call.sender_euid as i32);
        if let Some(service) = self.service.upgrade() {
            service.enforce_package_name(package, uid)?;
        }
        let mut p = Parcel::new();
        p.write_interface_token(session_cb::DESCRIPTOR);
        p.write_string16(package);
        p.write_i32(pid);
        p.write_i32(uid);
        args(&mut p);
        if let Err(e) = self.callback.transact(code, &p, true) {
            eprintln!(
                "app host: media_session: Remote failure in {}: {e:?}",
                session_cb::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("", |(_, n)| n)
            );
        }
        Ok(())
    }

    /// `sanitizeMediaMetadata`: an art URI of the content scheme the
    /// session's app may not grant read access to, put null.
    fn sanitize(&self, metadata: Metadata) -> Result<Metadata, Exception> {
        let mut bundle = metadata.bundle;
        let unreadable =
            |_| Exception::new(EX_BAD_PARCELABLE, "a MediaMetadata that does not read");
        for key in ART_URIS {
            let Some(uri) = bundle
                .string(key)
                .map_err(unreadable)?
                .filter(|u| !u.is_empty())
            else {
                continue;
            };
            let uri = crate::package::uri::Uri::parse(&uri);
            if uri.scheme() != Some("content") {
                continue;
            }
            match self.host.check_grant_uri_permission(
                self.owner_uid,
                &self.package,
                uri,
                FLAG_GRANT_READ_URI_PERMISSION,
                self.user,
            ) {
                Ok(_) => {}
                Err(e) if e.code == EX_SECURITY => {
                    bundle = bundle.with_null(key).map_err(unreadable)?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Metadata {
            bundle,
            limit: metadata.limit,
        })
    }

    fn held(&self, raw: Raw) -> Result<Held, Exception> {
        Held::new(raw, &self.host.process).map_err(|_| gone())
    }
}

/// A parcel kept whose file is gone.
fn gone() -> Exception {
    Exception::new(EX_ILLEGAL_STATE, "a file of the parcel is gone")
}

/// `MediaSessionRecord.SessionStub`.
struct SessionStub(Arc<Record>);

impl Service for SessionStub {
    fn descriptor(&self) -> &str {
        session::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let r = &self.0;
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            session::DESTROY_SESSION => {
                session::DestroySession::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(service) = r.service.upgrade() {
                    service.on_session_died(r);
                }
                session::write_destroy_session_reply(&mut p);
                Ok(())
            }
            session::SEND_EVENT => {
                let a =
                    session::SendEvent::<RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let data = a
                        .data
                        .map(|d| KeptBundle::new(d, &r.host.process))
                        .transpose()
                        .map_err(|_| gone())?;
                    let me = r.clone();
                    r.post(move || me.push_event(a.event, data));
                    session::write_send_event_reply(&mut p);
                    Ok(())
                })()
            }
            session::GET_CONTROLLER => {
                session::GetController::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                session::write_get_controller_reply(&mut p, r.controller.get().copied());
                Ok(())
            }
            session::SET_ACTIVE => {
                let a = session::SetActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.fields.lock().unwrap().active = a.active;
                if let Some(service) = r.service.upgrade() {
                    service.on_session_active_state_changed(r);
                }
                session::write_set_active_reply(&mut p);
                Ok(())
            }
            session::SET_FLAGS => {
                let a = session::SetFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let flags = a.flags as i64;
                    if flags & FLAG_EXCLUSIVE_GLOBAL_PRIORITY != 0
                        && r.host
                            .check_permission(Some("android.permission.MODIFY_PHONE_STATE"), uid)?
                            != 0
                    {
                        return Err(Exception::security(
                            "Must hold the MODIFY_PHONE_STATE permission.",
                        ));
                    }
                    r.fields.lock().unwrap().flags = flags;
                    if flags & FLAG_EXCLUSIVE_GLOBAL_PRIORITY != 0
                        && let Some(service) = r.service.upgrade()
                    {
                        service.set_global_priority_session(r);
                    }
                    session::write_set_flags_reply(&mut p);
                    Ok(())
                })()
            }
            session::SET_MEDIA_BUTTON_RECEIVER => {
                let a = session::SetMediaButtonReceiver::<BinderIn>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let pi = a.mbr.and_then(|b| b.0);
                    if let Some(pi) = pi
                        && r.host.intent_sender_kind(pi).0 == INTENT_SENDER_ACTIVITY
                    {
                        if r.host.change_enabled(THROW_FOR_ACTIVITY_MEDIA_BUTTON_RECEIVER, uid)? {
                            return Err(Exception::illegal_argument("The media button receiver cannot be set to an activity."));
                        }
                        eprintln!("app host: media_session: Ignoring invalid media button receiver targeting an activity.");
                        return Ok(());
                    }
                    let holder = ReceiverHolder::create(&r.host, r.user, pi, &r.package)?;
                    r.fields.lock().unwrap().receiver = holder;
                    if let Some(service) = r.service.upgrade() {
                        service.on_media_button_receiver_changed(r);
                    }
                    Ok(())
                })()
                .map(|()| session::write_set_media_button_receiver_reply(&mut p))
            }
            session::SET_MEDIA_BUTTON_BROADCAST_RECEIVER => {
                let a =
                    session::SetMediaButtonBroadcastReceiver::<ComponentName>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                (|| {
                    if let Some(receiver) = &a.broadcast_receiver
                        && receiver.package != r.package
                    {
                        return Err(Exception::illegal_argument(format!(
                            "receiver does not belong to package name provided to MediaSessionRecord. Pkg = {}, Receiver Pkg = {}",
                            r.package, receiver.package
                        )));
                    }
                    let Some(service) = r.service.upgrade() else { return Ok(()) };
                    let receiver = match a.broadcast_receiver {
                        Some(c) if service.media_button_receiver_exists(Some(&c), r.user)? => c,
                        receiver => {
                            if receiver.is_none() {
                                service.media_button_receiver_exists(None, r.user)?;
                            }
                            let name = receiver.map_or_else(|| "null".to_string(), |c| format!("ComponentInfo{{{}/{}}}", c.package, c.class));
                            if r.host.change_enabled(THROW_FOR_INVALID_BROADCAST_RECEIVER, uid)? {
                                return Err(Exception::illegal_argument(format!("Invalid component name: {name}")));
                            }
                            eprintln!("app host: media_session: setMediaButtonBroadcastReceiver(): Ignoring invalid component name={name}");
                            return Ok(());
                        }
                    };
                    r.fields.lock().unwrap().receiver = Some(ReceiverHolder::broadcast(r.user, receiver));
                    service.on_media_button_receiver_changed(r);
                    Ok(())
                })()
                .map(|()| session::write_set_media_button_broadcast_receiver_reply(&mut p))
            }
            session::SET_LAUNCH_PENDING_INTENT => {
                let a = session::SetLaunchPendingIntent::<BinderIn>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let mut f = r.fields.lock().unwrap();
                f.launch_intent_held = match a.pi.as_ref().and_then(|b| b.0) {
                    Some(Binder::Handle(h)) => Some(r.host.process.strong(h)),
                    _ => None,
                };
                f.launch_intent = a.pi.filter(|b| b.0.is_some());
                session::write_set_launch_pending_intent_reply(&mut p);
                Ok(())
            }
            session::SET_METADATA => {
                let a = session::SetMetadata::<Metadata>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let metadata = match a.metadata {
                        Some(m) => {
                            let m = r.sanitize(m)?;
                            let mut p = Parcel::new();
                            m.write_to(&mut p);
                            let raw =
                                captured(&mut p.reader(), |r| Metadata::read_from(r).map(|_| ()))
                                    .map_err(|_| gone())?;
                            Some(r.held(raw)?)
                        }
                        None => None,
                    };
                    {
                        let mut f = r.fields.lock().unwrap();
                        f.duration = a.duration;
                        f.metadata_description = a.metadata_description;
                        f.metadata = metadata;
                    }
                    let me = r.clone();
                    r.post(move || me.push_metadata());
                    session::write_set_metadata_reply(&mut p);
                    Ok(())
                })()
            }
            session::SET_PLAYBACK_STATE => {
                let a = session::SetPlaybackState::<State>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let state = match a.state {
                        Some(s) => Some(KeptState {
                            held: r.held(s.raw)?,
                            state: s.state,
                            position: s.position,
                            speed: s.speed,
                            update_time: s.update_time,
                        }),
                        None => None,
                    };
                    let old = r
                        .fields
                        .lock()
                        .unwrap()
                        .playback_state
                        .as_ref()
                        .map_or(STATE_NONE, |s| s.state);
                    let new = state.as_ref().map_or(STATE_NONE, |s| s.state);
                    let update_priority = ALWAYS_PRIORITY_STATES.contains(&new)
                        || (!TRANSITION_PRIORITY_STATES.contains(&old)
                            && TRANSITION_PRIORITY_STATES.contains(&new));
                    r.fields.lock().unwrap().playback_state = state;
                    if let Some(service) = r.service.upgrade() {
                        service.on_session_playback_state_changed(r, update_priority);
                    }
                    let me = r.clone();
                    r.post(move || me.push_playback_state());
                    session::write_set_playback_state_reply(&mut p);
                    Ok(())
                })()
            }
            session::RESET_QUEUE => {
                session::ResetQueue::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.fields.lock().unwrap().queue = None;
                let me = r.clone();
                r.post(move || me.push_queue());
                session::write_reset_queue_reply(&mut p);
                Ok(())
            }
            session::GET_BINDER_FOR_SET_QUEUE => {
                session::GetBinderForSetQueue::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let binder = r.host.process.add_service(Arc::new(QueueBinder {
                    record: r.clone(),
                    list: Mutex::new((Vec::new(), 0, false)),
                }));
                session::write_get_binder_for_set_queue_reply(&mut p, Some(binder));
                Ok(())
            }
            session::SET_QUEUE_TITLE => {
                let a =
                    session::SetQueueTitle::<Text>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let title = a.title.map(|t| r.held(t.0)).transpose()?;
                    r.fields.lock().unwrap().queue_title = title;
                    let me = r.clone();
                    r.post(move || me.push_queue_title());
                    session::write_set_queue_title_reply(&mut p);
                    Ok(())
                })()
            }
            session::SET_EXTRAS => {
                let a =
                    session::SetExtras::<RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let extras = a
                        .extras
                        .map(|e| KeptBundle::new(e, &r.host.process))
                        .transpose()
                        .map_err(|_| gone())?;
                    r.fields.lock().unwrap().extras = extras;
                    let me = r.clone();
                    r.post(move || me.push_extras());
                    session::write_set_extras_reply(&mut p);
                    Ok(())
                })()
            }
            session::SET_RATING_TYPE => {
                let a = session::SetRatingType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.fields.lock().unwrap().rating_type = a.r#type;
                session::write_set_rating_type_reply(&mut p);
                Ok(())
            }
            session::SET_CURRENT_VOLUME => {
                let a = session::SetCurrentVolume::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.fields.lock().unwrap().current_volume = a.current_volume;
                let me = r.clone();
                r.post(move || me.push_volume());
                session::write_set_current_volume_reply(&mut p);
                Ok(())
            }
            session::SET_PLAYBACK_TO_LOCAL => {
                let a = session::SetPlaybackToLocal::<Attributes>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let changed = {
                    let mut f = r.fields.lock().unwrap();
                    let changed = f.volume_type == PLAYBACK_TYPE_REMOTE;
                    f.volume_type = PLAYBACK_TYPE_LOCAL;
                    f.volume_control_id = None;
                    match a.attributes {
                        Some(attributes) => f.audio_attrs = attributes,
                        None => eprintln!(
                            "app host: media_session: Received null audio attributes, using existing attributes"
                        ),
                    }
                    changed
                };
                if changed {
                    if let Some(service) = r.service.upgrade() {
                        service.on_session_playback_type_changed(r);
                    }
                    let me = r.clone();
                    r.post(move || me.push_volume());
                }
                session::write_set_playback_to_local_reply(&mut p);
                Ok(())
            }
            session::SET_PLAYBACK_TO_REMOTE => {
                let a =
                    session::SetPlaybackToRemote::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let changed = {
                    let mut f = r.fields.lock().unwrap();
                    let changed = f.volume_type == PLAYBACK_TYPE_LOCAL;
                    f.volume_type = PLAYBACK_TYPE_REMOTE;
                    f.volume_control_type = a.control;
                    f.max_volume = a.max;
                    f.volume_control_id = a.control_id;
                    changed
                };
                if changed {
                    if let Some(service) = r.service.upgrade() {
                        service.on_session_playback_type_changed(r);
                    }
                    let me = r.clone();
                    r.post(move || me.push_volume());
                }
                session::write_set_playback_to_remote_reply(&mut p);
                Ok(())
            }
            code => {
                let name = session::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(r
                    .host
                    .unimplemented_named("media_session", &format!("ISession.{name} (#1793)")))
            }
        };
        reply(result.map(|()| p))
    }
}

/// `getBinderForSetQueue`'s `ParcelableListBinder<QueueItem>`: the count,
/// then items until it is reached, the queue set once complete.
struct QueueBinder {
    record: Arc<Record>,
    /// The items so far, the count, and whether it was consumed.
    list: Mutex<(Vec<Held>, i32, bool)>,
}

impl Service for QueueBinder {
    fn descriptor(&self) -> &str {
        ""
    }

    fn has_descriptor(&self) -> bool {
        false
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != FIRST_CALL_TRANSACTION {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        let complete = {
            let mut list = self.list.lock().unwrap();
            if list.2 {
                return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
            }
            if list.0.is_empty() {
                list.1 = call.data.read_i32()?;
            }
            let mut i = list.0.len() as i32;
            while i < list.1 && call.data.read_i32()? != END_OF_PARCEL {
                let Some(raw) = QueueItemIn::read(&mut call.data)?.0 else {
                    return reply(Err(self.record.host.unimplemented_named(
                        "media_session",
                        "a queue item with an icon bitmap (#1793)",
                    )));
                };
                let held = Held::new(raw, &self.record.host.process)?;
                list.0.push(held);
                i += 1;
            }
            if i >= list.1 {
                list.2 = true;
                Some(std::mem::take(&mut list.0))
            } else {
                None
            }
        };
        if let Some(queue) = complete {
            self.record.fields.lock().unwrap().queue = Some(queue);
            let me = self.record.clone();
            self.record.post(move || me.push_queue());
        }
        Ok(Parcel::new())
    }
}

/// `MediaSessionRecord.ControllerStub`.
struct ControllerStub(Arc<Record>);

impl ControllerStub {
    /// `registerCallback`: told the session is gone if it is, else kept
    /// until it dies, once.
    fn register(&self, callback: Option<Binder>) {
        let r = &self.0;
        let Some(Binder::Handle(handle)) = callback else {
            return;
        };
        let mut f = r.fields.lock().unwrap();
        if f.destroyed {
            drop(f);
            let mut p = Parcel::new();
            p.write_interface_token(controller_cb::DESCRIPTOR);
            let _ = r
                .host
                .process
                .transact(handle, controller_cb::ON_SESSION_DESTROYED, &p, true);
            return;
        }
        if f.holders.iter().any(|h| h.callback.handle == handle) {
            return;
        }
        let strong = r.host.process.strong(handle);
        let weak = Arc::downgrade(r);
        let death = r.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(r) = weak.upgrade() {
                    let mut f = r.fields.lock().unwrap();
                    if let Some(i) = f.holders.iter().position(|h| h.callback.handle == handle) {
                        let h = f.holders.remove(i);
                        r.host.process.clear_death(&h.callback, h.death);
                    }
                }
            }),
        );
        f.holders.push(Holder {
            callback: strong,
            death,
        });
    }

    fn unregister(&self, callback: Option<Binder>) {
        let r = &self.0;
        let Some(Binder::Handle(handle)) = callback else {
            return;
        };
        let mut f = r.fields.lock().unwrap();
        if let Some(i) = f.holders.iter().position(|h| h.callback.handle == handle) {
            let h = f.holders.remove(i);
            r.host.process.clear_death(&h.callback, h.death);
        }
    }
}

impl Service for ControllerStub {
    fn descriptor(&self) -> &str {
        controller::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let r = self.0.clone();
        let mut p = Parcel::new();
        macro_rules! transport {
            ($kind:ident, $code:expr, $reply:ident, |$a:ident, $q:ident| $args:expr) => {{
                let $a = controller::$kind::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.session_call(call, $code, $a.package_name.as_deref(), |$q| $args)
                    .map(|()| controller::$reply(&mut p))
            }};
        }
        let result: Result<(), Exception> = match call.code {
            controller::REGISTER_CALLBACK => {
                let a =
                    controller::RegisterCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.register(a.cb);
                controller::write_register_callback_reply(&mut p);
                Ok(())
            }
            controller::UNREGISTER_CALLBACK => {
                let a =
                    controller::UnregisterCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.unregister(a.cb);
                controller::write_unregister_callback_reply(&mut p);
                Ok(())
            }
            controller::GET_PACKAGE_NAME => {
                controller::GetPackageName::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                controller::write_get_package_name_reply(&mut p, &Some(r.package.clone()));
                Ok(())
            }
            controller::GET_TAG => {
                controller::GetTag::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                controller::write_get_tag_reply(&mut p, &r.tag);
                Ok(())
            }
            controller::GET_SESSION_INFO => {
                controller::GetSessionInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                controller::write_get_session_info_reply(
                    &mut p,
                    r.session_info.as_ref().map(|i| &i.bundle),
                );
                Ok(())
            }
            controller::GET_LAUNCH_PENDING_INTENT => {
                controller::GetLaunchPendingIntent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let f = r.fields.lock().unwrap();
                controller::write_get_launch_pending_intent_reply(&mut p, f.launch_intent.as_ref());
                Ok(())
            }
            controller::GET_FLAGS => {
                controller::GetFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                controller::write_get_flags_reply(&mut p, r.fields.lock().unwrap().flags);
                Ok(())
            }
            controller::GET_VOLUME_ATTRIBUTES => {
                controller::GetVolumeAttributes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.volume_attributes()
                    .map(|info| controller::write_get_volume_attributes_reply(&mut p, Some(&info)))
            }
            controller::GET_METADATA => {
                controller::GetMetadata::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let metadata = r.fields.lock().unwrap().metadata.clone();
                controller::write_get_metadata_reply(&mut p, metadata.as_ref());
                Ok(())
            }
            controller::GET_PLAYBACK_STATE => {
                controller::GetPlaybackState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                controller::write_get_playback_state_reply(
                    &mut p,
                    r.state_with_updated_position().as_ref(),
                );
                Ok(())
            }
            controller::GET_QUEUE => {
                controller::GetQueue::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let queue = r.fields.lock().unwrap().queue.clone();
                let slice = queue.map(|q| Slice::new(r.host.process.clone(), QUEUE_ITEM, q));
                controller::write_get_queue_reply(&mut p, slice.as_ref());
                Ok(())
            }
            controller::GET_QUEUE_TITLE => {
                controller::GetQueueTitle::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let title = r.fields.lock().unwrap().queue_title.clone();
                controller::write_get_queue_title_reply(&mut p, title.as_ref());
                Ok(())
            }
            controller::GET_EXTRAS => {
                controller::GetExtras::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let extras = r.fields.lock().unwrap().extras.clone();
                controller::write_get_extras_reply(&mut p, extras.as_ref().map(|e| &e.bundle));
                Ok(())
            }
            controller::GET_RATING_TYPE => {
                controller::GetRatingType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                controller::write_get_rating_type_reply(
                    &mut p,
                    r.fields.lock().unwrap().rating_type,
                );
                Ok(())
            }
            controller::SEND_COMMAND => {
                let a = controller::SendCommand::<RawBundle, BinderIn>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_COMMAND,
                    a.package_name.as_deref(),
                    |q| {
                        q.write_string16(a.command.as_deref());
                        aim_service_aidl::write_typed(q, a.args.as_ref());
                        aim_service_aidl::write_typed(q, a.cb.as_ref());
                    },
                )
                .map(|()| controller::write_send_command_reply(&mut p))
            }
            controller::SEND_CUSTOM_ACTION => {
                let a = controller::SendCustomAction::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_CUSTOM_ACTION,
                    a.package_name.as_deref(),
                    |q| {
                        q.write_string16(a.action.as_deref());
                        aim_service_aidl::write_typed(q, a.args.as_ref());
                    },
                )
                .map(|()| controller::write_send_custom_action_reply(&mut p))
            }
            controller::PREPARE => transport!(
                Prepare,
                session_cb::ON_PREPARE,
                write_prepare_reply,
                |a, q| {
                    let _ = (&a, q);
                }
            ),
            controller::PREPARE_FROM_MEDIA_ID => {
                let a = controller::PrepareFromMediaId::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_PREPARE_FROM_MEDIA_ID,
                    a.package_name.as_deref(),
                    |q| {
                        q.write_string16(a.media_id.as_deref());
                        aim_service_aidl::write_typed(q, a.extras.as_ref());
                    },
                )
                .map(|()| controller::write_prepare_from_media_id_reply(&mut p))
            }
            controller::PREPARE_FROM_SEARCH => {
                let a = controller::PrepareFromSearch::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_PREPARE_FROM_SEARCH,
                    a.package_name.as_deref(),
                    |q| {
                        q.write_string16(a.string.as_deref());
                        aim_service_aidl::write_typed(q, a.extras.as_ref());
                    },
                )
                .map(|()| controller::write_prepare_from_search_reply(&mut p))
            }
            controller::PREPARE_FROM_URI => {
                let a = controller::PrepareFromUri::<ParcelUri, RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_PREPARE_FROM_URI,
                    a.package_name.as_deref(),
                    |q| {
                        aim_service_aidl::write_typed(q, a.uri.as_ref().map(UriOut).as_ref());
                        aim_service_aidl::write_typed(q, a.extras.as_ref());
                    },
                )
                .map(|()| controller::write_prepare_from_uri_reply(&mut p))
            }
            controller::PLAY => transport!(Play, session_cb::ON_PLAY, write_play_reply, |a, q| {
                let _ = (&a, q);
            }),
            controller::PLAY_FROM_MEDIA_ID => {
                let a = controller::PlayFromMediaId::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_PLAY_FROM_MEDIA_ID,
                    a.package_name.as_deref(),
                    |q| {
                        q.write_string16(a.media_id.as_deref());
                        aim_service_aidl::write_typed(q, a.extras.as_ref());
                    },
                )
                .map(|()| controller::write_play_from_media_id_reply(&mut p))
            }
            controller::PLAY_FROM_SEARCH => {
                let a = controller::PlayFromSearch::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_PLAY_FROM_SEARCH,
                    a.package_name.as_deref(),
                    |q| {
                        q.write_string16(a.string.as_deref());
                        aim_service_aidl::write_typed(q, a.extras.as_ref());
                    },
                )
                .map(|()| controller::write_play_from_search_reply(&mut p))
            }
            controller::PLAY_FROM_URI => {
                let a = controller::PlayFromUri::<ParcelUri, RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                r.session_call(
                    call,
                    session_cb::ON_PLAY_FROM_URI,
                    a.package_name.as_deref(),
                    |q| {
                        aim_service_aidl::write_typed(q, a.uri.as_ref().map(UriOut).as_ref());
                        aim_service_aidl::write_typed(q, a.extras.as_ref());
                    },
                )
                .map(|()| controller::write_play_from_uri_reply(&mut p))
            }
            controller::SKIP_TO_QUEUE_ITEM => transport!(
                SkipToQueueItem,
                session_cb::ON_SKIP_TO_TRACK,
                write_skip_to_queue_item_reply,
                |a, q| q.write_i64(a.id)
            ),
            controller::PAUSE => {
                transport!(Pause, session_cb::ON_PAUSE, write_pause_reply, |a, q| {
                    let _ = (&a, q);
                })
            }
            controller::STOP => transport!(Stop, session_cb::ON_STOP, write_stop_reply, |a, q| {
                let _ = (&a, q);
            }),
            controller::NEXT => transport!(Next, session_cb::ON_NEXT, write_next_reply, |a, q| {
                let _ = (&a, q);
            }),
            controller::PREVIOUS => transport!(
                Previous,
                session_cb::ON_PREVIOUS,
                write_previous_reply,
                |a, q| {
                    let _ = (&a, q);
                }
            ),
            controller::FAST_FORWARD => transport!(
                FastForward,
                session_cb::ON_FAST_FORWARD,
                write_fast_forward_reply,
                |a, q| {
                    let _ = (&a, q);
                }
            ),
            controller::REWIND => {
                transport!(Rewind, session_cb::ON_REWIND, write_rewind_reply, |a, q| {
                    let _ = (&a, q);
                })
            }
            controller::SEEK_TO => transport!(
                SeekTo,
                session_cb::ON_SEEK_TO,
                write_seek_to_reply,
                |a, q| q.write_i64(a.pos)
            ),
            controller::SET_PLAYBACK_SPEED => {
                transport!(
                    SetPlaybackSpeed,
                    session_cb::ON_SET_PLAYBACK_SPEED,
                    write_set_playback_speed_reply,
                    |a, q| q.write_f32(a.speed)
                )
            }
            controller::RATE => {
                let a = controller::Rate::<In>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                r.session_call(call, session_cb::ON_RATE, a.package_name.as_deref(), |q| {
                    aim_service_aidl::write_typed(q, a.rating.as_ref())
                })
                .map(|()| controller::write_rate_reply(&mut p))
            }
            code => {
                let name = controller::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(r.host.unimplemented_named(
                    "media_session",
                    &format!("ISessionController.{name} (#1793)"),
                ))
            }
        };
        reply(result.map(|()| p))
    }
}

/// A `Uri` passed on.
struct UriOut<'a>(&'a ParcelUri);

impl WriteParcelable for UriOut<'_> {
    fn write_to(&self, p: &mut Parcel) {
        self.0.0.write(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `PlaybackState` as `writeToParcel` writes it: playing at 1000 ms
    /// at double speed, updated at 5000 ms, with a custom action.
    fn playback_state() -> Parcel {
        let mut p = Parcel::new();
        p.write_i32(STATE_PLAYING);
        p.write_i64(1000);
        p.write_f32(2.0);
        p.write_i64(5000);
        p.write_i64(0); // buffered position
        p.write_i64(0x200); // actions
        p.write_i32(1); // a custom action
        p.write_i32(1);
        p.write_string16(Some("like"));
        clip::write_char_sequence(&mut p, Some("Like"));
        p.write_i32(7); // icon
        crate::bundle::write(&mut p, &[]);
        p.write_i64(-1); // active item
        clip::write_char_sequence(&mut p, None);
        p.write_i32(-1); // no extras
        p
    }

    /// The fields the record reads, and the state with an updated
    /// position (`setState(state, position, speed, updateTime)`) the rest
    /// kept.
    #[test]
    fn playback_states_read_and_update_their_position() {
        let p = playback_state();
        let s = State::read_from(&mut Reader::new(p.data(), &[])).unwrap();
        assert_eq!(
            (s.state, s.position, s.speed, s.update_time),
            (STATE_PLAYING, 1000, 2.0, 5000)
        );
        assert_eq!(s.raw.parcel().data(), p.data());
        let held = Held {
            raw: s.raw,
            _binders: Arc::new(Vec::new()),
        };
        let kept = KeptState {
            held,
            state: s.state,
            position: s.position,
            speed: s.speed,
            update_time: s.update_time,
        };
        assert!(kept.is_active());
        let moved = kept.at(1 << 33 | 3, 9000).held.raw.parcel();
        let again = State::read_from(&mut Reader::new(moved.data(), &[])).unwrap();
        assert_eq!(
            (again.position, again.update_time, again.speed),
            (1 << 33 | 3, 9000, 2.0)
        );
        assert_eq!(&moved.data()[24..], &p.data()[24..]);
    }

    /// A `QueueItem` as `writeParcelable` writes it: read past its
    /// description and id, kept without its class; none for one with an
    /// icon bitmap, and another class does not read.
    #[test]
    fn queue_items_read_as_written() {
        let mut class = Parcel::new();
        class.write_string16(Some(QUEUE_ITEM));
        let mut p = Parcel::new();
        p.write_string16(Some(QUEUE_ITEM));
        p.write_string16(Some("id"));
        clip::write_char_sequence(&mut p, Some("Title"));
        clip::write_char_sequence(&mut p, None);
        clip::write_char_sequence(&mut p, None);
        p.write_string16(None); // no icon
        p.write_string16(Some("android.net.Uri$StringUri"));
        p.write_i32(1);
        p.write_string8(Some("content://art/1"));
        crate::bundle::write(&mut p, &[]);
        p.write_string16(None); // no media URI
        p.write_i64(42);
        let item = QueueItemIn::read(&mut Reader::new(p.data(), &[]))
            .unwrap()
            .0
            .unwrap();
        assert_eq!(item.parcel().data(), &p.data()[class.data().len()..]);
        let mut icon = Parcel::new();
        icon.write_string16(Some(QUEUE_ITEM));
        icon.write_string16(None);
        for _ in 0..3 {
            clip::write_char_sequence(&mut icon, None);
        }
        icon.write_string16(Some("android.graphics.Bitmap"));
        assert!(
            QueueItemIn::read(&mut Reader::new(icon.data(), &[]))
                .unwrap()
                .0
                .is_none()
        );
        let mut other = Parcel::new();
        other.write_string16(Some("android.os.Bundle"));
        assert!(QueueItemIn::read(&mut Reader::new(other.data(), &[])).is_err());
    }
}
