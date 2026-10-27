//! `clipboard` (IClipboard): the clipboard as a native service backed by
//! the Mac's pasteboard (ADR 0013), in place of SystemServer's
//! ClipboardService.
//!
//! It follows the original (`ClipboardService.java` at the pinned tag):
//! a primary clip per user and device, set by any app and read only by the
//! focused app, the default input method or a holder of
//! READ_CLIPBOARD_IN_BACKGROUND, each access checked against the caller's
//! package and noted as the READ/WRITE_CLIPBOARD app op; listeners told of
//! every change they may read; a clip copied to the user's related
//! profiles unless a restriction forbids it; the clip cleared an hour after
//! its last use.
//!
//! The Mac takes the emulator's place (EmulatorClipboardMonitor): a clip
//! set on the default device becomes the Mac's text, and text copied on
//! the Mac becomes user 0's clip, set as a mirrored device syncs its
//! clipboard (by the shell, with SystemUI's overlay suppressed). The Mac's
//! text is read only when an app pastes it.
//!
//! What the original does through system_server-internal APIs is done
//! through their binder counterparts (docs/system-services.md); where
//! none exists, this service does without: focus as WindowManager sees
//! it, content capture and autofill access and virtual devices (#430), the paste
//! toast, text classification and statistics (#431).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::android_content_iclipboard as ic;
use aim_service_aidl::android_content_ionprimaryclipchangedlistener as listener;

use crate::clip::{ClipData, ClipDescription};
use crate::pasteboard;
use crate::settings::Settings;
use crate::system::{MODE_ALLOWED, System};

/// `AppOpsManager.OP_READ_CLIPBOARD` and `OP_WRITE_CLIPBOARD`.
pub const OP_READ_CLIPBOARD: i32 = 29;
pub const OP_WRITE_CLIPBOARD: i32 = 30;
/// The app ops its access decisions read.
pub const APP_OPS: [i32; 2] = [OP_READ_CLIPBOARD, OP_WRITE_CLIPBOARD];
/// `Context.DEVICE_ID_DEFAULT` and `DEVICE_ID_INVALID`.
const DEVICE_ID_DEFAULT: i32 = 0;
const DEVICE_ID_INVALID: i32 = -1;
/// `Process.SYSTEM_UID`, `SHELL_UID`, `NOBODY_UID`;
/// `UserHandle.PER_USER_RANGE`.
const SYSTEM_UID: i32 = 1000;
const SHELL_UID: i32 = 2000;
const NOBODY_UID: i32 = 9999;
const PER_USER_RANGE: i32 = 100_000;
/// The package a mirrored device's clipboard sync sets clips as, which
/// SystemUI shows no overlay for when the clip asks it not to
/// (`ClipboardOverlaySuppressionControllerImpl.SHELL_PACKAGE`).
const SHELL_PACKAGE: &str = "com.android.shell";
/// `DeviceConfig.NAMESPACE_CLIPBOARD` and the properties the original
/// reads.
const NAMESPACE: &str = "clipboard";
const AUTO_CLEAR_ENABLED: &str = "auto_clear_enabled";
const AUTO_CLEAR_TIMEOUT: &str = "auto_clear_timeout";
/// `ClipboardManager.DEVICE_CONFIG_SHOW_ACCESS_NOTIFICATIONS`, and its
/// default.
const SHOW_ACCESS_NOTIFICATIONS: &str = "show_access_notifications";
const DEFAULT_SHOW_ACCESS_NOTIFICATIONS: bool = true;
/// `ClipboardService.DEFAULT_CLIPBOARD_TIMEOUT_MILLIS`.
const DEFAULT_AUTO_CLEAR_MILLIS: i64 = 3_600_000;
/// `Settings.Secure.CLIPBOARD_SHOW_ACCESS_NOTIFICATIONS` and
/// `DEFAULT_INPUT_METHOD`.
const SECURE_SHOW_ACCESS_NOTIFICATIONS: &str = "clipboard_show_access_notifications";
const SECURE_DEFAULT_INPUT_METHOD: &str = "default_input_method";
/// `Intent.FLAG_GRANT_READ_URI_PERMISSION`.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 1;
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");
/// How often the Mac's pasteboard is looked at for a change.
const MAC_POLL: Duration = Duration::from_millis(250);

const READ_CLIPBOARD_IN_BACKGROUND: &str = "android.permission.READ_CLIPBOARD_IN_BACKGROUND";
const INTERNAL_SYSTEM_WINDOW: &str = "android.permission.INTERNAL_SYSTEM_WINDOW";
const SET_CLIP_SOURCE: &str = "android.permission.SET_CLIP_SOURCE";
const MANAGE_CLIPBOARD_ACCESS_NOTIFICATION: &str =
    "android.permission.MANAGE_CLIPBOARD_ACCESS_NOTIFICATION";
const DISALLOW_CROSS_PROFILE_COPY_PASTE: &str = "no_cross_profile_copy_paste";
const DISALLOW_SHARE_INTO_MANAGED_PROFILE: &str = "no_sharing_into_profile";

type Result<T> = std::result::Result<T, Exception>;

fn user_of(uid: i32) -> i32 {
    uid / PER_USER_RANGE
}

fn app_of(uid: i32) -> i32 {
    uid % PER_USER_RANGE
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A primary clip.
enum Primary {
    /// Set by an app; the binders it carries are held with it.
    Clip {
        clip: Box<ClipData>,
        _held: Vec<Strong>,
    },
    /// The Mac's text, read when pasted; its description is fixed.
    Mac { timestamp: i64 },
}

struct Listener {
    strong: Strong,
    death: u64,
    uid: i32,
    package: Option<String>,
    attribution_tag: Option<String>,
}

#[derive(Default)]
struct Clipboard {
    primary: Option<Primary>,
    primary_uid: Option<i32>,
    source_package: Option<String>,
    listeners: Vec<Listener>,
    active_permission_owners: HashSet<String>,
    /// When the clip is cleared (`scheduleAutoClear`).
    clear_at: Option<Instant>,
}

#[derive(Default)]
struct State {
    /// By (user, device).
    clipboards: HashMap<(i32, i32), Clipboard>,
    /// The Mac's change count after this service's own last write.
    mac_written: i64,
    /// The Mac's change count last looked at.
    mac_seen: i64,
}

pub struct ClipboardService {
    system: Arc<System>,
    settings: Arc<Settings>,
    process: Arc<LocalProcess>,
    operations: Mutex<()>,
    state: Mutex<State>,
    permission_owner: Mutex<Option<Arc<crate::clipboard_permissions::UriOwner>>>,
    this: Weak<ClipboardService>,
}

/// Who called, and as whom they act.
struct Caller {
    pid: i32,
    uid: i32,
}

impl ClipboardService {
    pub fn new(
        process: Arc<LocalProcess>,
        system: Arc<System>,
        settings: Arc<Settings>,
    ) -> Arc<Self> {
        let service = Arc::new_cyclic(|this| Self {
            system,
            settings,
            process,
            operations: Mutex::new(()),
            permission_owner: Mutex::new(None),
            state: Mutex::new(State {
                mac_seen: pasteboard::change_count(),
                ..State::default()
            }),
            this: this.clone(),
        });
        let weak = Arc::downgrade(&service);
        let _ = std::thread::Builder::new()
            .name("clipboard-mac".into())
            .spawn(move || {
                while let Some(service) = weak.upgrade() {
                    service.tick();
                    drop(service);
                    std::thread::sleep(MAC_POLL);
                }
            });
        service
    }

    /// Picks up a change of the Mac's pasteboard, and clears clips whose
    /// time is up.
    fn tick(&self) {
        let _operation = self.operations.lock().unwrap();
        let count = pasteboard::change_count();
        let changed = {
            let mut st = self.state.lock().unwrap();
            let changed = count != st.mac_seen && count != st.mac_written;
            st.mac_seen = count;
            changed
        };
        if changed && pasteboard::has_text() {
            let primary = Primary::Mac {
                timestamp: now_millis(),
            };
            if let Err(e) = self.set_primary_on(
                0,
                DEVICE_ID_DEFAULT,
                Some(primary),
                SHELL_UID,
                Some(SHELL_PACKAGE),
            ) {
                eprintln!("clipboard: replacing clip from pasteboard: {}", e.message);
            }
        }
        let due: Vec<(i32, i32, i32)> = {
            let st = self.state.lock().unwrap();
            st.clipboards
                .iter()
                .filter(|(_, c)| c.clear_at.is_some_and(|t| t <= Instant::now()))
                .map(|(&(user, device), c)| (user, device, c.primary_uid.unwrap_or(NOBODY_UID)))
                .collect()
        };
        for (user, device, uid) in due {
            let uid = user * PER_USER_RANGE + app_of(uid);
            self.state
                .lock()
                .unwrap()
                .clipboards
                .entry((user, device))
                .or_default()
                .clear_at = None;
            if let Err(e) = self.set_primary_locked(None, uid, device, None) {
                eprintln!("clipboard: clearing expired clip: {}", e.message);
            }
        }
    }

    /// `getIntendingUserId`.
    fn intending_user(&self, caller: &Caller, package: Option<&str>, user_id: i32) -> Result<i32> {
        let calling_user = user_of(caller.uid);
        if calling_user == user_id {
            return Ok(calling_user);
        }
        self.system.handle_incoming_user(
            caller.pid,
            caller.uid,
            user_id,
            true,
            "checkClipboardServiceCallingUser",
            package,
        )
    }

    /// `getIntendingUid`.
    fn intending_uid(&self, caller: &Caller, package: Option<&str>, user_id: i32) -> Result<i32> {
        Ok(self.intending_user(caller, package, user_id)? * PER_USER_RANGE + app_of(caller.uid))
    }

    /// `getIntendingDeviceId`, for a device without virtual devices: only
    /// the default device has a clipboard (#430).
    fn intending_device(requested: i32) -> i32 {
        if requested == DEVICE_ID_DEFAULT {
            DEVICE_ID_DEFAULT
        } else {
            DEVICE_ID_INVALID
        }
    }

    /// `isDefaultIme`: the package of `Settings.Secure.DEFAULT_INPUT_METHOD`
    /// (`ComponentName.unflattenFromString`).
    fn is_default_ime(&self, user_id: i32, package: &str) -> Result<bool> {
        let ime = self.settings.secure(SECURE_DEFAULT_INPUT_METHOD, user_id)?;
        Ok(ime
            .as_deref()
            .and_then(|c| c.split_once('/'))
            .is_some_and(|(p, _)| p == package))
    }

    /// `clipboardAccessAllowed`.
    #[allow(clippy::too_many_arguments)]
    fn access_allowed(
        &self,
        caller_uid: i32,
        op: i32,
        package: Option<&str>,
        attribution_tag: Option<&str>,
        uid: i32,
        user_id: i32,
        device_id: i32,
        note: bool,
    ) -> Result<bool> {
        let package = package.unwrap_or_default();
        self.system.check_package(uid, package)?;
        if device_id == DEVICE_ID_INVALID {
            return Ok(false);
        }
        // Writing needs no focus; the original's permission and IME
        // lookups decide nothing for it, so they are left out. The rest is
        // the original's disjunction with the mirrored inputs first, so a
        // focused app's read asks no other service.
        let allowed = op != OP_READ_CLIPBOARD
            || (device_id == DEVICE_ID_DEFAULT && self.system.uid_focused(uid)?)
            || self.is_default_ime(user_id, package)?
            || self
                .system
                .package_has_permission(READ_CLIPBOARD_IN_BACKGROUND, package)?
            || (self.system.uid_focused(caller_uid)?
                && self
                    .system
                    .package_has_permission(INTERNAL_SYSTEM_WINDOW, package)?);
        if !allowed {
            eprintln!(
                "clipboard: denying clipboard access to {package}, application is not in focus nor is it a system service for user {user_id}"
            );
            return Ok(false);
        }
        Ok(self
            .system
            .app_op(note, op, uid, package, attribution_tag)?
            == MODE_ALLOWED)
    }

    /// `getClipboardLocked`: whether the user's clipboard exists.
    fn clipboard_exists(&self, user_id: i32, device_id: i32) -> Result<bool> {
        if self
            .state
            .lock()
            .unwrap()
            .clipboards
            .contains_key(&(user_id, device_id))
        {
            return Ok(true);
        }
        if !self.system.user_running(user_id)? {
            return Ok(false);
        }
        self.state
            .lock()
            .unwrap()
            .clipboards
            .insert((user_id, device_id), Clipboard::default());
        Ok(true)
    }

    /// `setPrimaryClipInternalLocked(clip, uid, deviceId, sourcePackage)`:
    /// this user's clipboard, the Mac's, and the related profiles'.
    fn set_primary(
        &self,
        clip: Option<(ClipData, Vec<Strong>)>,
        uid: i32,
        device_id: i32,
        source_package: Option<&str>,
    ) -> Result<()> {
        let _operation = self.operations.lock().unwrap();
        self.set_primary_locked(clip, uid, device_id, source_package)
    }

    fn set_primary_locked(
        &self,
        clip: Option<(ClipData, Vec<Strong>)>,
        uid: i32,
        device_id: i32,
        source_package: Option<&str>,
    ) -> Result<()> {
        let user_id = user_of(uid);
        if device_id == DEVICE_ID_DEFAULT {
            // EmulatorClipboardMonitor.accept: the text, and its HTML,
            // go to the Mac; a clear clears the Mac's pasteboard only
            // while it holds the clip this service put there.
            let first = clip.as_ref().and_then(|(c, _)| c.items.first());
            let text = first.and_then(|i| i.text.clone());
            let html = first.and_then(|i| i.html.clone());
            let mut st = self.state.lock().unwrap();
            if clip.is_some() || st.mac_seen == st.mac_written {
                st.mac_written = pasteboard::set_text(
                    text.as_deref().or(clip.as_ref().map(|_| "")),
                    html.as_deref(),
                );
                st.mac_seen = st.mac_written;
            }
        }
        if !self.clipboard_exists(user_id, device_id)? {
            return Ok(());
        }
        let copy = clip.as_ref().map(|(c, _)| c.clone());
        let primary = clip.map(|(clip, held)| Primary::Clip {
            clip: Box::new(clip),
            _held: held,
        });
        self.set_primary_on(user_id, device_id, primary, uid, source_package)?;
        let related = self.system.profile_ids(user_id)?;
        if related.len() > 1 {
            let copy = if self
                .system
                .has_user_restriction(DISALLOW_CROSS_PROFILE_COPY_PASTE, user_id)?
            {
                None
            } else if let Some(mut clip) = copy {
                clip.fix_uris_light(user_id)
                    .map_err(|s| Exception::illegal_argument(format!("bad clip: status {s}")))?;
                Some(clip)
            } else {
                return Ok(());
            };
            for id in related.into_iter().filter(|&id| id != user_id) {
                if !self
                    .system
                    .has_user_restriction(DISALLOW_SHARE_INTO_MANAGED_PROFILE, id)?
                    && self.clipboard_exists(id, device_id)?
                {
                    let primary = copy.clone().map(|mut clip| {
                        let held = clip
                            .handles()
                            .into_iter()
                            .map(|h| self.process.strong(h))
                            .collect();
                        Primary::Clip {
                            clip: Box::new(clip),
                            _held: held,
                        }
                    });
                    self.set_primary_on(id, device_id, primary, uid, source_package)?;
                }
            }
        }
        Ok(())
    }

    /// `setPrimaryClipInternalNoClassifyLocked`: sets one clipboard's clip
    /// and tells its listeners.
    fn set_primary_on(
        &self,
        user_id: i32,
        device_id: i32,
        primary: Option<Primary>,
        uid: i32,
        source_package: Option<&str>,
    ) -> Result<()> {
        let listeners: Vec<(u32, i32, Option<String>, Option<String>)> = {
            let mut st = self.state.lock().unwrap();
            let board = st.clipboards.entry((user_id, device_id)).or_default();
            self.revoke_uris(board)?;
            board.active_permission_owners.clear();
            if primary.is_none() && board.primary.is_none() {
                return Ok(());
            }
            let primary = primary.map(|p| match p {
                Primary::Clip { mut clip, _held } => {
                    // startClassificationLocked: nothing here classifies
                    // text (#431).
                    clip.description.set_not_classified();
                    clip.description.timestamp = now_millis();
                    Primary::Clip { clip, _held }
                }
                mac => mac,
            });
            board.primary_uid = primary.is_some().then_some(uid);
            board.source_package = primary
                .is_some()
                .then(|| source_package.map(str::to_string))
                .flatten();
            board.primary = primary;
            board
                .listeners
                .iter()
                .map(|l| {
                    (
                        l.strong.handle,
                        l.uid,
                        l.package.clone(),
                        l.attribution_tag.clone(),
                    )
                })
                .collect()
        };
        for (handle, uid, package, tag) in listeners {
            // The original checks with its own identity (the system uid).
            let allowed = self.access_allowed(
                SYSTEM_UID,
                OP_READ_CLIPBOARD,
                package.as_deref(),
                tag.as_deref(),
                uid,
                user_of(uid),
                device_id,
                true,
            );
            if allowed.unwrap_or(false) {
                let mut data = Parcel::new();
                listener::DispatchPrimaryClipChanged {}.write(&mut data);
                let _ = self.process.transact(
                    handle,
                    listener::DISPATCH_PRIMARY_CLIP_CHANGED,
                    &data,
                    true,
                );
            }
        }
        Ok(())
    }

    /// The one owner of all clipboard grants, as in ClipboardService.
    fn permission_owner(&self) -> Result<Arc<crate::clipboard_permissions::UriOwner>> {
        let mut owner = self.permission_owner.lock().unwrap();
        if owner.is_none() {
            *owner = Some(Arc::new(self.system.new_clipboard_uri_owner()?));
        }
        Ok(owner.as_ref().expect("created").clone())
    }

    /// `revokeUris`: the old clip's exact read grants, across all readers.
    /// Called under the clipboard lock, including replacement by Mac text
    /// and automatic clearing, so a paste cannot resurrect an old grant.
    fn revoke_uris(&self, board: &Clipboard) -> Result<()> {
        let owner = self.permission_owner.lock().unwrap().clone();
        if let (Some(owner), Some(Primary::Clip { clip, .. })) = (owner, &board.primary) {
            for (uri, user) in
                clip_content_uris(clip, user_of(board.primary_uid.unwrap_or(NOBODY_UID)))
            {
                owner.revoke(&uri, user)?;
            }
        }
        Ok(())
    }

    /// `addActiveOwnerLocked`, before returning a clip. The app's package
    /// identity was checked by `clipboardAccessAllowed`.
    fn grant_active_owner(&self, user: i32, device: i32, package: &str) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let Some(board) = st.clipboards.get_mut(&(user, device)) else {
            return Ok(());
        };
        if board.active_permission_owners.contains(package) {
            return Ok(());
        }
        if let Some(Primary::Clip { clip, .. }) = &board.primary {
            let uid = board.primary_uid.expect("clip's source uid");
            for (uri, source_user) in clip_content_uris(clip, user_of(uid)) {
                self.permission_owner()?.grant(
                    &self.system,
                    uid,
                    package,
                    &uri,
                    source_user,
                    user,
                )?;
            }
        }
        if board.primary.is_some() {
            board.active_permission_owners.insert(package.into());
        }
        Ok(())
    }

    /// The clip to hand out, the Mac's read now.
    fn clip_of(primary: &Primary) -> Option<ClipData> {
        match primary {
            Primary::Clip { clip, .. } => Some((**clip).clone()),
            Primary::Mac { timestamp } => pasteboard::text()
                .map(|text| ClipData::host(&text, pasteboard::html().as_deref(), *timestamp)),
        }
    }

    fn description_of(primary: &Primary) -> ClipDescription {
        match primary {
            Primary::Clip { clip, .. } => clip.description.clone(),
            Primary::Mac { timestamp } => {
                let html = pasteboard::has_html().then_some("");
                ClipData::host("", html, *timestamp).description
            }
        }
    }

    /// `scheduleAutoClear`, as the clipboard's DeviceConfig says.
    fn schedule_auto_clear(&self, user_id: i32, device_id: i32) -> Result<()> {
        if !self
            .settings
            .config_bool(NAMESPACE, AUTO_CLEAR_ENABLED, true)?
        {
            return Ok(());
        }
        let timeout =
            self.settings
                .config_long(NAMESPACE, AUTO_CLEAR_TIMEOUT, DEFAULT_AUTO_CLEAR_MILLIS)?;
        let at = Instant::now() + Duration::from_millis(timeout.max(0) as u64);
        if let Some(board) = self
            .state
            .lock()
            .unwrap()
            .clipboards
            .get_mut(&(user_id, device_id))
        {
            board.clear_at = Some(at);
        }
        Ok(())
    }

    /// `checkDataOwner`: throws unless `uid` may grant read access to each
    /// item's content URI and intent data.
    fn check_data_owner(&self, clip: &ClipData, uid: i32) -> Result<()> {
        for (uri, user) in clip_content_uris(clip, user_of(uid)) {
            self.system.check_grant_uri_permission(
                uid,
                &uri,
                FLAG_GRANT_READ_URI_PERMISSION,
                user,
            )?;
        }
        Ok(())
    }

    /// `checkAndSetPrimaryClip`.
    #[allow(clippy::too_many_arguments)]
    fn check_and_set(
        &self,
        caller: &Caller,
        clip: Option<ClipData>,
        package: Option<&str>,
        attribution_tag: Option<&str>,
        user_id: i32,
        device_id: i32,
        source_package: Option<&str>,
    ) -> Result<()> {
        let Some(mut clip) = clip.filter(|c| !c.items.is_empty()) else {
            return Err(Exception::illegal_argument("No items"));
        };
        let uid = self.intending_uid(caller, package, user_id)?;
        let user = user_of(uid);
        let device = Self::intending_device(device_id);
        if !self.access_allowed(
            caller.uid,
            OP_WRITE_CLIPBOARD,
            package,
            attribution_tag,
            uid,
            user,
            device,
            true,
        )? {
            return Ok(());
        }
        self.check_data_owner(&clip, uid)?;
        clip.keep_files(&|fd| self.process.file(fd))
            .map_err(|_| Exception::illegal_argument("bad parcel: a file descriptor is gone"))?;
        let held = clip
            .handles()
            .into_iter()
            .map(|h| self.process.strong(h))
            .collect();
        self.set_primary(Some((clip, held)), uid, device, source_package)?;
        self.schedule_auto_clear(user_id, device)
    }

    /// The common part of the read methods: the intending user and device,
    /// or `None` when the caller may not read.
    fn readable(
        &self,
        caller: &Caller,
        package: Option<&str>,
        attribution_tag: Option<&str>,
        user_id: i32,
        device_id: i32,
        note: bool,
    ) -> Result<Option<(i32, i32, i32)>> {
        let uid = self.intending_uid(caller, package, user_id)?;
        let user = user_of(uid);
        let device = Self::intending_device(device_id);
        if !self.access_allowed(
            caller.uid,
            OP_READ_CLIPBOARD,
            package,
            attribution_tag,
            uid,
            user,
            device,
            note,
        )? || self.system.device_locked(user, device_id)?
        {
            return Ok(None);
        }
        Ok(Some((uid, user, device)))
    }

    fn enforce(&self, caller: &Caller, permission: &str) -> Result<()> {
        self.system
            .enforce_permission(permission, caller.pid, caller.uid)
    }

    /// The check of the access notification methods, with their message.
    fn enforce_access_notifications(&self, caller: &Caller) -> Result<()> {
        if self.system.check_permission(
            MANAGE_CLIPBOARD_ACCESS_NOTIFICATION,
            caller.pid,
            caller.uid,
        )? {
            return Ok(());
        }
        Err(Exception::security(
            "areClipboardAccessNotificationsEnable requires permission MANAGE_CLIPBOARD_ACCESS_NOTIFICATION",
        ))
    }

    fn dispatch(&self, call: &mut Call<'_>) -> Result<Option<Parcel>> {
        let caller = Caller {
            pid: call.sender_pid,
            uid: call.sender_euid as i32,
        };
        let mut reply = Parcel::new();
        let r = &mut call.data;
        let status = |e| Exception::illegal_argument(format!("bad parcel: status {e}"));
        match call.code {
            ic::SET_PRIMARY_CLIP => {
                let a = ic::SetPrimaryClip::<ClipData>::read(r).map_err(status)?;
                self.check_and_set(
                    &caller,
                    a.clip,
                    a.calling_package.as_deref(),
                    a.attribution_tag.as_deref(),
                    a.user_id,
                    a.device_id,
                    a.calling_package.as_deref(),
                )?;
                ic::write_set_primary_clip_reply(&mut reply);
            }
            ic::SET_PRIMARY_CLIP_AS_PACKAGE => {
                let a = ic::SetPrimaryClipAsPackage::<ClipData>::read(r).map_err(status)?;
                self.enforce(&caller, SET_CLIP_SOURCE)?;
                self.check_and_set(
                    &caller,
                    a.clip,
                    a.calling_package.as_deref(),
                    a.attribution_tag.as_deref(),
                    a.user_id,
                    a.device_id,
                    a.source_package.as_deref(),
                )?;
                ic::write_set_primary_clip_as_package_reply(&mut reply);
            }
            ic::CLEAR_PRIMARY_CLIP => {
                let a = ic::ClearPrimaryClip::read(r).map_err(status)?;
                let uid = self.intending_uid(&caller, a.calling_package.as_deref(), a.user_id)?;
                let device = Self::intending_device(a.device_id);
                if self.access_allowed(
                    caller.uid,
                    OP_WRITE_CLIPBOARD,
                    a.calling_package.as_deref(),
                    a.attribution_tag.as_deref(),
                    uid,
                    user_of(uid),
                    device,
                    true,
                )? {
                    if let Some(b) = self
                        .state
                        .lock()
                        .unwrap()
                        .clipboards
                        .get_mut(&(a.user_id, a.device_id))
                    {
                        b.clear_at = None;
                    }
                    self.set_primary(None, uid, device, a.calling_package.as_deref())?;
                }
                ic::write_clear_primary_clip_reply(&mut reply);
            }
            ic::GET_PRIMARY_CLIP => {
                let a = ic::GetPrimaryClip::read(r).map_err(status)?;
                let mut clip = None;
                if let Some((uid, user, device)) = self.readable(
                    &caller,
                    a.pkg.as_deref(),
                    a.attribution_tag.as_deref(),
                    a.user_id,
                    a.device_id,
                    true,
                )? && self.clipboard_exists(user, device)?
                {
                    let _operation = self.operations.lock().unwrap();
                    match self.grant_active_owner(
                        user,
                        device,
                        a.pkg.as_deref().unwrap_or_default(),
                    ) {
                        Ok(()) => {
                            clip = self.state.lock().unwrap().clipboards[&(user, device)]
                                .primary
                                .as_ref()
                                .and_then(Self::clip_of);
                        }
                        Err(e) if e.code == aim_binder_host::parcel::EX_SECURITY => {
                            // The original clears a clip whose source lost URI access.
                            self.set_primary_locked(None, uid, device, a.pkg.as_deref())?;
                        }
                        Err(e) => return Err(e),
                    }
                    if clip.is_some() {
                        self.schedule_auto_clear(a.user_id, device)?;
                    }
                }
                ic::write_get_primary_clip_reply(&mut reply, clip.as_ref());
            }
            ic::GET_PRIMARY_CLIP_DESCRIPTION => {
                let a = ic::GetPrimaryClipDescription::read(r).map_err(status)?;
                let mut description = None;
                if let Some((_, user, device)) = self.readable(
                    &caller,
                    a.calling_package.as_deref(),
                    a.attribution_tag.as_deref(),
                    a.user_id,
                    a.device_id,
                    false,
                )? && self.clipboard_exists(user, device)?
                {
                    description = self.state.lock().unwrap().clipboards[&(user, device)]
                        .primary
                        .as_ref()
                        .map(Self::description_of);
                }
                ic::write_get_primary_clip_description_reply(&mut reply, description.as_ref());
            }
            ic::HAS_PRIMARY_CLIP | ic::HAS_CLIPBOARD_TEXT => {
                let (package, tag, user_id, device_id) = if call.code == ic::HAS_PRIMARY_CLIP {
                    let a = ic::HasPrimaryClip::read(r).map_err(status)?;
                    (a.calling_package, a.attribution_tag, a.user_id, a.device_id)
                } else {
                    let a = ic::HasClipboardText::read(r).map_err(status)?;
                    (a.calling_package, a.attribution_tag, a.user_id, a.device_id)
                };
                let mut has = false;
                if let Some((_, user, device)) = self.readable(
                    &caller,
                    package.as_deref(),
                    tag.as_deref(),
                    user_id,
                    device_id,
                    false,
                )? && self.clipboard_exists(user, device)?
                {
                    let st = self.state.lock().unwrap();
                    has = match &st.clipboards[&(user, device)].primary {
                        None => false,
                        Some(_) if call.code == ic::HAS_PRIMARY_CLIP => true,
                        Some(Primary::Clip { clip, .. }) => {
                            clip.items[0].text.as_deref().is_some_and(|t| !t.is_empty())
                        }
                        Some(Primary::Mac { .. }) => pasteboard::has_text(),
                    };
                }
                reply.write_no_exception();
                reply.write_bool(has);
            }
            ic::ADD_PRIMARY_CLIP_CHANGED_LISTENER | ic::REMOVE_PRIMARY_CLIP_CHANGED_LISTENER => {
                let add = call.code == ic::ADD_PRIMARY_CLIP_CHANGED_LISTENER;
                let (listener, package, tag, user_id, device_id) = if add {
                    let a = ic::AddPrimaryClipChangedListener::read(r).map_err(status)?;
                    (
                        a.listener,
                        a.calling_package,
                        a.attribution_tag,
                        a.user_id,
                        a.device_id,
                    )
                } else {
                    let a = ic::RemovePrimaryClipChangedListener::read(r).map_err(status)?;
                    (
                        a.listener,
                        a.calling_package,
                        a.attribution_tag,
                        a.user_id,
                        a.device_id,
                    )
                };
                let uid = self.intending_uid(&caller, package.as_deref(), user_id)?;
                let device = Self::intending_device(device_id);
                if device != DEVICE_ID_INVALID && self.clipboard_exists(user_of(uid), device)? {
                    let Some(Binder::Handle(handle)) = listener else {
                        return Err(Exception::new(
                            aim_binder_host::parcel::EX_NULL_POINTER,
                            "listener",
                        ));
                    };
                    if add {
                        self.add_listener(user_of(uid), device, handle, uid, package, tag);
                    } else {
                        self.remove_listener(user_of(uid), device, handle);
                    }
                }
                reply.write_no_exception();
            }
            ic::GET_PRIMARY_CLIP_SOURCE => {
                let a = ic::GetPrimaryClipSource::read(r).map_err(status)?;
                self.enforce(&caller, SET_CLIP_SOURCE)?;
                let mut source = None;
                if let Some((_, user, device)) = self.readable(
                    &caller,
                    a.calling_package.as_deref(),
                    a.attribution_tag.as_deref(),
                    a.user_id,
                    a.device_id,
                    false,
                )? {
                    let st = self.state.lock().unwrap();
                    if let Some(b) = st
                        .clipboards
                        .get(&(user, device))
                        .filter(|b| b.primary.is_some())
                    {
                        source = b.source_package.clone();
                    }
                }
                ic::write_get_primary_clip_source_reply(&mut reply, &source);
            }
            ic::ARE_CLIPBOARD_ACCESS_NOTIFICATIONS_ENABLED_FOR_USER => {
                let a =
                    ic::AreClipboardAccessNotificationsEnabledForUser::read(r).map_err(status)?;
                self.enforce_access_notifications(&caller)?;
                let default = self.settings.config_bool(
                    NAMESPACE,
                    SHOW_ACCESS_NOTIFICATIONS,
                    DEFAULT_SHOW_ACCESS_NOTIFICATIONS,
                )?;
                // Settings.Secure.getIntForUser: the default for a value
                // that is not a number.
                let enabled = self
                    .settings
                    .secure(SECURE_SHOW_ACCESS_NOTIFICATIONS, a.user_id)?
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(default.into());
                ic::write_are_clipboard_access_notifications_enabled_for_user_reply(
                    &mut reply,
                    enabled != 0,
                );
            }
            ic::SET_CLIPBOARD_ACCESS_NOTIFICATIONS_ENABLED_FOR_USER => {
                let a =
                    ic::SetClipboardAccessNotificationsEnabledForUser::read(r).map_err(status)?;
                self.enforce_access_notifications(&caller)?;
                self.settings.put_secure(
                    SECURE_SHOW_ACCESS_NOTIFICATIONS,
                    if a.enable { "1" } else { "0" },
                    a.user_id,
                )?;
                ic::write_set_clipboard_access_notifications_enabled_for_user_reply(&mut reply);
            }
            DUMP_TRANSACTION => {
                // Binder.onTransact: the original's ClipboardImpl dumps
                // nothing.
                reply.write_no_exception();
            }
            _ => return Ok(None),
        }
        Ok(Some(reply))
    }

    fn add_listener(
        &self,
        user_id: i32,
        device_id: i32,
        handle: u32,
        uid: i32,
        package: Option<String>,
        attribution_tag: Option<String>,
    ) {
        self.remove_listener(user_id, device_id, handle);
        let strong = self.process.strong(handle);
        let this = self.this.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(service) = this.upgrade() {
                    service.forget_listener(user_id, device_id, handle);
                }
            }),
        );
        let mut st = self.state.lock().unwrap();
        st.clipboards
            .entry((user_id, device_id))
            .or_default()
            .listeners
            .push(Listener {
                strong,
                death,
                uid,
                package,
                attribution_tag,
            });
    }

    fn take_listener(&self, user_id: i32, device_id: i32, handle: u32) -> Option<Listener> {
        let mut st = self.state.lock().unwrap();
        let board = st.clipboards.get_mut(&(user_id, device_id))?;
        let at = board
            .listeners
            .iter()
            .position(|l| l.strong.handle == handle)?;
        Some(board.listeners.remove(at))
    }

    fn remove_listener(&self, user_id: i32, device_id: i32, handle: u32) {
        if let Some(l) = self.take_listener(user_id, device_id, handle) {
            self.process.clear_death(&l.strong, l.death);
        }
    }

    /// A listener's process died (`RemoteCallbackList`).
    fn forget_listener(&self, user_id: i32, device_id: i32, handle: u32) {
        drop(self.take_listener(user_id, device_id, handle));
    }
}

fn clip_content_uris(
    clip: &ClipData,
    default_user: i32,
) -> impl Iterator<Item = (String, i32)> + '_ {
    clip.items
        .iter()
        .flat_map(|i| [&i.uri, &i.intent_data])
        .flatten()
        .filter_map(move |uri| content_uri(uri, default_user))
}

/// A `content:` URI as `ContentProvider.getUriWithoutUserId` and
/// `getUserIdFromUri(uri, default_user)` split it: without the user in
/// its authority (`user@authority`), and that user. None for another
/// scheme.
fn content_uri(uri: &str, default_user: i32) -> Option<(String, i32)> {
    use crate::package::uri::{Uri, encode, parse_int};
    let parsed = Uri::parse(uri);
    if parsed.scheme() != Some("content") {
        return None;
    }
    let authority = parsed.authority();
    let (user, authority) = match authority.as_deref().and_then(|a| a.rsplit_once('@')) {
        Some((user, authority)) => (parse_int(user).unwrap_or(-10_000), Some(authority)),
        None => (default_user, authority.as_deref()),
    };
    Some((
        parsed
            .with_encoded_authority(authority.map(encode).as_deref())
            .as_str()
            .into(),
        user,
    ))
}

impl Service for ClipboardService {
    fn descriptor(&self) -> &str {
        ic::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // A clip's bundles may carry some; so does a dump.
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut reply = Parcel::new();
        match self.dispatch(call) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(UNKNOWN_TRANSACTION),
            Err(exception) => {
                reply.write_exception(&exception);
                Ok(reply)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_user_off_content_uris() {
        assert_eq!(
            content_uri("content://10@media/external/1?x#f", 0),
            Some(("content://media/external/1?x#f".into(), 10))
        );
        assert_eq!(
            content_uri("content://com.example.files/a", 10),
            Some(("content://com.example.files/a".into(), 10))
        );
        assert_eq!(content_uri("https://example.com", 0), None);
    }

    use crate::app_host::uri_grants::UriGrantsManager;
    use aim_service_aidl::android_app_iurigrantsmanager as ugm;
    use aim_service_aidl::{ReadParcelable, WriteParcelable};

    fn clip(uri: &str) -> ClipData {
        let mut p = Parcel::new();
        ClipData::new_plain_text(None, None)
            .description
            .write_to(&mut p);
        p.write_i32(0); // icon
        p.write_i32(1); // item count
        p.write_i32(1);
        p.write_string8(None); // null text
        p.write_string8(None); // HTML
        p.write_i32(0);
        p.write_i32(0); // intent, sender
        p.write_i32(1);
        p.write_i32(1);
        p.write_string8(Some(uri));
        p.write_i32(0);
        p.write_i32(0); // activity info, links
        ClipData::read_from(&mut p.reader()).unwrap()
    }

    fn service(system: Arc<System>) -> ClipboardService {
        let process = system.process();
        ClipboardService {
            system: system.clone(),
            settings: Settings::new(process.clone(), system),
            process,
            operations: Mutex::new(()),
            state: Mutex::new(State::default()),
            permission_owner: Mutex::new(None),
            this: Weak::new(),
        }
    }

    fn count(host: &Arc<crate::app_host::AppHost>, uid: u32, package: &str) -> i32 {
        let mut p = Parcel::new();
        ugm::GetUriPermissions {
            package_name: Some(package.into()),
            incoming: true,
            persisted_only: false,
        }
        .write(&mut p);
        let mut call = Call {
            code: ugm::GET_URI_PERMISSIONS,
            flags: 0,
            sender_pid: 42,
            sender_euid: uid,
            data: p.reader(),
        };
        let reply = UriGrantsManager(host.clone()).transact(&mut call).unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1); // typed slice
        r.read_i32().unwrap()
    }

    /// ClipboardService.checkDataOwner/addActiveOwnerLocked/revokeUris and
    /// UriGrantsManagerService.grantUriPermissionFromOwner, android-16.0.0_r1.
    #[test]
    fn a_paste_grants_read_until_replacement_and_rechecks_the_source() {
        crate::system::with_discovery_session(|host, publish, _| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    let mut provider = crate::package::pkg::Provider::default();
                    provider.main.component.name = "p.Provider".into();
                    provider.main.component.package_name = "p".into();
                    provider.main.enabled = true;
                    provider.authority = Some("p.files".into());
                    provider.grant_uri_permissions = true;
                    pkg.providers.push(provider);
                }
            });
            let system = host.package_system().unwrap();
            system.set_native_uri_grants(host);
            let service = service(system.clone());
            let clip = clip("content://p.files/item");
            service.check_data_owner(&clip, 10100).unwrap();
            assert_eq!(
                service.check_data_owner(&clip, 10101).unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert_eq!(count(host, 10101, "q"), 0);
            service
                .set_primary_on(
                    0,
                    0,
                    Some(Primary::Clip {
                        clip: Box::new(clip.clone()),
                        _held: vec![],
                    }),
                    10100,
                    Some("p"),
                )
                .unwrap();
            assert_eq!(count(host, 10101, "q"), 0);
            service.grant_active_owner(0, 0, "q").unwrap();
            assert_eq!(count(host, 10101, "q"), 1);
            service.check_data_owner(&clip, 10101).unwrap(); // a granted reader may copy
            assert_eq!(
                system
                    .check_grant_uri_permission(10101, "content://p.files/item", 2, 0)
                    .unwrap_err()
                    .code,
                aim_binder_host::parcel::EX_SECURITY
            ); // clipboard grants no write access
            service.grant_active_owner(0, 0, "q").unwrap();
            assert_eq!(count(host, 10101, "q"), 1);
            service
                .set_primary_on(
                    0,
                    0,
                    Some(Primary::Mac { timestamp: 0 }),
                    SHELL_UID,
                    Some(SHELL_PACKAGE),
                )
                .unwrap();
            assert_eq!(count(host, 10101, "q"), 0);
            assert!(
                service.state.lock().unwrap().clipboards[&(0, 0)]
                    .active_permission_owners
                    .is_empty()
            );
            assert_eq!(
                service.check_data_owner(&clip, 10101).unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
            service
                .set_primary_on(
                    0,
                    0,
                    Some(Primary::Clip {
                        clip: Box::new(clip),
                        _held: vec![],
                    }),
                    10100,
                    Some("p"),
                )
                .unwrap();
            service.grant_active_owner(0, 0, "q").unwrap();
            assert_eq!(count(host, 10101, "q"), 1);
            service
                .set_primary_on(0, 0, None, 10100, Some("p"))
                .unwrap();
            assert_eq!(count(host, 10101, "q"), 0);
        });
    }

    #[test]
    fn profile_uris_keep_the_source_user_in_the_returned_parcel() {
        let mut original = clip("content://p.files/item");
        original.fix_uris_light(10).unwrap();
        assert_eq!(
            original.items[0].uri.as_deref(),
            Some("content://10@p.files/item")
        );
        let mut p = Parcel::new();
        original.write_to(&mut p);
        let copied = ClipData::read_from(&mut p.reader()).unwrap();
        assert_eq!(
            clip_content_uris(&copied, 0).collect::<Vec<_>>(),
            vec![("content://p.files/item".into(), 10)]
        );
        for (uri, expected) in [
            (
                "content://%31%30@p.files/item",
                Some(("content://p.files/item".into(), 10)),
            ),
            (
                "content://bad@p.files/item",
                Some(("content://p.files/item".into(), -10000)),
            ),
            ("file:///tmp/a", None),
        ] {
            assert_eq!(content_uri(uri, 0), expected);
        }
        let mut encoded = clip("content://p%2Efiles/item");
        encoded.fix_uris_light(10).unwrap();
        assert_eq!(
            encoded.items[0].uri.as_deref(),
            Some("content://10@p%2Efiles/item")
        );
        assert_eq!(
            clip_content_uris(&encoded, 0).collect::<Vec<_>>(),
            vec![("content://p.files/item".into(), 10)]
        );
        assert_eq!(
            content_uri("content://p%2Efiles/item", 0),
            Some(("content://p.files/item".into(), 0))
        );
        let mut clip = clip("content://12@p.files/item");
        clip.fix_uris_light(10).unwrap();
        assert_eq!(
            clip.items[0].uri.as_deref(),
            Some("content://12@p.files/item")
        );
    }
}
