//! The Mac's consent to an app's screen capture (`docs/media.md`): the
//! `IScreenCaptureConsent` the consent activity of
//! `config_mediaProjectionPermissionDialogComponent`
//! (java/media-projection) asks, registered as `aim.screen_capture_consent`.
//!
//! It does what SystemUI's `MediaProjectionPermissionActivity` does once it
//! knows the app, with the Mac's sheet as its dialog: an app that may
//! project already (`hasProjectionPermission`: CAPTURE_VIDEO_OUTPUT, or
//! the PROJECT_MEDIA app op) gets its projection without a question;
//! otherwise the display server asks on the app's window. On consent it
//! creates the projection for the whole display
//! (`IMediaProjectionManager.createProjection`, which needs
//! MANAGE_MEDIA_PROJECTION: the system uid has it), or reuses the granted
//! one when system_server asks the user to review a consent the app reuses,
//! and reports the reviewed consent (`setUserReviewGrantedConsentResult`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Parcel, UNKNOWN_TRANSACTION};
use aim_host_display::media::Message;
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm,
    android_media_projection_imediaprojection as projection,
    android_media_projection_imediaprojectionmanager as mpm, android_os_iservicemanager as sm,
    dev_aim_server_iscreencaptureconsent as consent,
    dev_aim_server_iscreencaptureconsentcallback as answer,
};

use super::parcels::LaunchCookie;
use super::{Bridge, RETRY, call, hold};

/// The name the consent activity finds the service by.
const NAME: &str = "aim.screen_capture_consent";
/// The consent activity's package, the only caller.
const ACTIVITY_PACKAGE: &str = "dev.aim.mediaprojection";
/// `MediaProjectionManager.TYPE_SCREEN_CAPTURE`.
const TYPE_SCREEN_CAPTURE: i32 = 0;
/// `Display.DEFAULT_DISPLAY`: the whole screen is shared.
const DEFAULT_DISPLAY: i32 = 0;
/// `ReviewGrantedConsentResult`.
const RECORD_CANCEL: i32 = 0;
const RECORD_CONTENT_DISPLAY: i32 = 1;

/// An app's request, waiting for the user.
struct Request {
    package: String,
    uid: i32,
    review: bool,
    callback: Strong,
}

pub struct Consents {
    process: Arc<LocalProcess>,
    bridge: Weak<Bridge>,
    service: Binder,
    pending: Mutex<(u32, HashMap<u32, Request>)>,
}

/// The service node.
struct Node(Weak<Bridge>);

impl Consents {
    pub fn new(process: &Arc<LocalProcess>, bridge: Weak<Bridge>) -> Consents {
        Consents {
            process: process.clone(),
            service: process.add_service(Arc::new(Node(bridge.clone()))),
            bridge,
            pending: Mutex::new((0, HashMap::new())),
        }
    }

    /// Registers the service with servicemanager, once it answers.
    pub fn register(&self) {
        loop {
            let mut data = Parcel::new();
            sm::AddService {
                name: Some(NAME.into()),
                service: Some(self.service),
                allow_isolated: false,
                // IServiceManager.DUMP_FLAG_PRIORITY_DEFAULT
                dump_priority: 1 << 3,
            }
            .write(&mut data);
            let added = self
                .process
                .transact(0, sm::ADD_SERVICE, &data, false)
                .ok()
                .and_then(|reply| sm::read_add_service_reply(&mut reply.reader()).ok());
            let added = added.is_some_and(|r| r.is_ok());
            // Until servicemanager serves calls.
            if added {
                return;
            }
            std::thread::sleep(RETRY);
        }
    }

    /// `name`'s service, from servicemanager.
    fn service(&self, name: &str) -> Result<Strong, String> {
        self.bridge
            .upgrade()
            .and_then(|b| b.find(name))
            .ok_or(format!("no {name} service"))
    }

    /// Whether `uid` is the consent activity's app.
    fn is_activity(&self, uid: u32) -> bool {
        let packages = self.service("package").and_then(|pm| {
            call(
                &pm,
                pm::GET_PACKAGES_FOR_UID,
                |p| pm::GetPackagesForUid { uid: uid as i32 }.write(p),
                pm::read_get_packages_for_uid_reply,
            )
        });
        packages.is_ok_and(|p| {
            p.unwrap_or_default()
                .iter()
                .any(|p| p.as_deref() == Some(ACTIVITY_PACKAGE))
        })
    }

    fn ask(&self, args: consent::Ask) -> Result<(), String> {
        let (Some(package), Some(Binder::Handle(callback))) = (args.package_name, args.callback)
        else {
            return Err("no package or callback".into());
        };
        let request = Request {
            package,
            uid: args.uid,
            review: args.review_granted_consent,
            callback: self.process.strong(callback),
        };
        let may = self.service("media_projection").and_then(|manager| {
            let may = call(
                &manager,
                mpm::HAS_PROJECTION_PERMISSION,
                |p| {
                    mpm::HasProjectionPermission {
                        process_uid: request.uid,
                        package_name: Some(request.package.clone()),
                    }
                    .write(p)
                },
                mpm::read_has_projection_permission_reply,
            )?;
            Ok((manager, may))
        });
        let (manager, may) = match may {
            Ok(m) => m,
            Err(e) => {
                reply(&request, None);
                return Err(e);
            }
        };
        if may {
            // As a system-privileged recorder is granted: no question.
            let cookie = args.launch_cookie.map(LaunchCookie);
            self.grant(&manager, &request, cookie);
            return Ok(());
        }
        let label = args.label.unwrap_or_else(|| request.package.clone());
        let message = {
            let mut pending = self.pending.lock().unwrap();
            pending.0 = pending.0.wrapping_add(1);
            let id = pending.0;
            let m = Message::Consent {
                id,
                package: request.package.clone(),
                label,
            };
            eprintln!("guest-init: media: {}: consent {id} asked", request.package);
            pending.1.insert(id, request);
            m
        };
        match self.bridge.upgrade() {
            Some(b) => b.send(&message),
            None => self.cancel_all(),
        }
        Ok(())
    }

    /// The user's answer to request `id`.
    pub fn answered(&self, id: u32, allowed: bool) {
        let Some(request) = self.pending.lock().unwrap().1.remove(&id) else {
            return;
        };
        match self.service("media_projection") {
            Ok(manager) if allowed => self.grant(&manager, &request, None),
            Ok(manager) => self.deny(&manager, &request),
            Err(e) => {
                eprintln!("guest-init: media: consent: {e}");
                reply(&request, None);
            }
        }
    }

    /// Every request waiting is declined: nobody is left to ask.
    pub fn cancel_all(&self) {
        let pending: Vec<Request> = self
            .pending
            .lock()
            .unwrap()
            .1
            .drain()
            .map(|(_, r)| r)
            .collect();
        for request in pending {
            match self.service("media_projection") {
                Ok(manager) => self.deny(&manager, &request),
                Err(_) => reply(&request, None),
            }
        }
    }

    /// The consent is given: the projection, to the activity
    /// (`MediaProjectionServiceHelper.createOrReuseProjection`, then
    /// `setReviewedConsentIfNeeded`).
    fn grant(&self, manager: &Strong, request: &Request, cookie: Option<LaunchCookie>) {
        let reused = if request.review {
            call(
                manager,
                mpm::GET_PROJECTION,
                |p| {
                    mpm::GetProjection {
                        process_uid: request.uid,
                        package_name: Some(request.package.clone()),
                    }
                    .write(p)
                },
                |r| Ok(mpm::read_get_projection_reply(r)?.map(|b| hold(&self.process, b))),
            )
        } else {
            Ok(None)
        };
        let created = reused.and_then(|reused| match reused {
            Some(p) => Ok(Some(p)),
            None => call(
                manager,
                mpm::CREATE_PROJECTION,
                |p| {
                    mpm::CreateProjection {
                        process_uid: request.uid,
                        package_name: Some(request.package.clone()),
                        r#type: TYPE_SCREEN_CAPTURE,
                        permanent_grant: false,
                        display_id: DEFAULT_DISPLAY,
                    }
                    .write(p)
                },
                |r| Ok(mpm::read_create_projection_reply(r)?.map(|b| hold(&self.process, b))),
            ),
        });
        let granted = match created {
            Ok(Some(granted)) => granted,
            Ok(None) => {
                eprintln!("guest-init: media: {}: no projection", request.package);
                return reply(request, None);
            }
            Err(e) => {
                eprintln!("guest-init: media: {}: projection: {e}", request.package);
                return reply(request, None);
            }
        };
        if let Some(cookie) = cookie
            && let Err(e) = call(
                &granted,
                projection::SET_LAUNCH_COOKIE,
                |p| {
                    projection::SetLaunchCookie {
                        launch_cookie: Some(cookie),
                    }
                    .write(p)
                },
                projection::read_set_launch_cookie_reply,
            )
        {
            eprintln!(
                "guest-init: media: {}: setLaunchCookie: {e}",
                request.package
            );
        }
        if request.review {
            reviewed(manager, RECORD_CONTENT_DISPLAY, Some(granted.binder()));
        }
        reply(request, Some(granted.binder()));
    }

    /// The user declined.
    fn deny(&self, manager: &Strong, request: &Request) {
        if request.review {
            reviewed(manager, RECORD_CANCEL, None);
        }
        reply(request, None);
    }
}

/// `IMediaProjectionManager.setUserReviewGrantedConsentResult`: the
/// result is an AIDL enum, an int.
fn reviewed(manager: &Strong, result: i32, projection: Option<Binder>) {
    let done = call(
        manager,
        mpm::SET_USER_REVIEW_GRANTED_CONSENT_RESULT,
        |p| {
            p.write_interface_token(mpm::DESCRIPTOR);
            p.write_i32(result);
            p.write_binder(projection);
        },
        |r| r.read_exception(),
    );
    if let Err(e) = done {
        eprintln!("guest-init: media: setUserReviewGrantedConsentResult: {e}");
    }
}

/// Tells the activity the projection, or that there is none.
fn reply(request: &Request, projection: Option<Binder>) {
    let mut data = Parcel::new();
    answer::OnResult { projection }.write(&mut data);
    if let Err(s) = request.callback.transact(answer::ON_RESULT, &data, true) {
        eprintln!(
            "guest-init: media: {}: consent answer: status {s}",
            request.package
        );
    }
}

impl Service for Node {
    fn descriptor(&self) -> &str {
        consent::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(bridge) = self.0.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        if call.code != consent::ASK {
            return Err(UNKNOWN_TRANSACTION);
        }
        // SystemUI's activity alone may ask, as the app it names; so this
        // one.
        if !bridge.consents.is_activity(call.sender_euid) {
            eprintln!(
                "guest-init: media: consent asked by uid {}: refused",
                call.sender_euid
            );
            return Ok(Parcel::new());
        }
        let args = consent::Ask::read(&mut call.data)?;
        let package = args.package_name.clone().unwrap_or_default();
        if let Err(e) = bridge.consents.ask(args) {
            eprintln!("guest-init: media: {package}: consent: {e}");
        }
        Ok(Parcel::new())
    }
}
