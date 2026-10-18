//! `media_router`: MediaRouterService for the app host
//! (`android-16.0.0_r1`), its `MediaRouter` (v1) clients: their
//! registration, discovery requests and playback state, and the routes a
//! trusted client gets. Those routes come from the remote display
//! providers (`RemoteDisplayProviderWatcher`), which are bound only once a
//! trusted client asks to discover remote displays
//! (`RemoteDisplayProviderProxy.shouldBind`); until then they publish
//! none. Binding them, and `MediaRouter2` and its managers, are not
//! implemented (#1663).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_media_imediarouterservice as mr;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::component_resolver::Info;
use crate::package::intent::Intent;

/// `RemoteDisplayState.SERVICE_INTERFACE`.
const REMOTE_DISPLAY_PROVIDER: &str = "com.android.media.remotedisplay.RemoteDisplayProvider";
/// `MediaRouter.ROUTE_TYPE_REMOTE_DISPLAY`.
const ROUTE_TYPE_REMOTE_DISPLAY: i32 = 1 << 2;
const CONFIGURE_WIFI_DISPLAY: &str = "android.permission.CONFIGURE_WIFI_DISPLAY";

/// `MediaRouterClientState` without routes.
struct NoRoutes;

impl WriteParcelable for NoRoutes {
    fn write_to(&self, p: &mut Parcel) {
        // `writeTypedList` of none.
        p.write_i32(0);
    }
}

/// `ClientRecord`.
struct Client {
    binder: Strong,
    death: u64,
    uid: i32,
    /// Its `UserRecord`'s user.
    user: i32,
    trusted: bool,
    route_types: i32,
    active_scan: bool,
    /// `mSelectedRouteId`.
    selected_route: Option<String>,
}

pub struct MediaRouter {
    host: Arc<AppHost>,
    /// `mAllClientRecords`, by handle.
    clients: Arc<Mutex<HashMap<u32, Client>>>,
}

impl MediaRouter {
    pub fn new(host: Arc<AppHost>) -> Self {
        MediaRouter { host, clients: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// `RemoteDisplayProviderWatcher.scanPackages`: the services of
    /// `user` that would be remote display providers
    /// (`verifyServiceTrusted`: guarded by BIND_REMOTE_DISPLAY, their
    /// package granted REMOTE_DISPLAY_PROVIDER).
    fn providers(&self, user: i32) -> Result<usize, Exception> {
        let (state, resolution) = self.host.packages()?;
        let intent = Intent { action: Some(REMOTE_DISPLAY_PROVIDER.into()), ..Default::default() };
        let found = resolution
            .query_intent_services(&intent, None, 0, user, 1000)
            .map_err(|e| Exception::new(EX_UNSUPPORTED_OPERATION, format!("queryIntentServices: {e:?}")))?;
        let mut count = 0;
        for r in found {
            let Info::Service(si) = r.info else { continue };
            if si.permission.as_deref() != Some("android.permission.BIND_REMOTE_DISPLAY") {
                continue;
            }
            let package = si.info.item.package_name.clone().unwrap_or_default();
            let Some(app_id) = state.packages.get(&package).map(|ps| ps.app_id) else { continue };
            let uid = user * PER_USER_RANGE as i32 + app_id;
            if self.host.check_permission(Some("android.permission.REMOTE_DISPLAY_PROVIDER"), uid)? == 0 {
                count += 1;
            }
        }
        Ok(count)
    }

    /// `registerClientAsUser`.
    fn register(&self, call: &Call<'_>, client: Option<Binder>, package: Option<String>, user: i32) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        // `validatePackageName`.
        let owned = match &package {
            Some(package) => self.host.owns_package(uid, package)?,
            None => false,
        };
        if !owned {
            return Err(Exception::security("packageName must match the calling uid"));
        }
        let user = self.host.handle_incoming_user(uid, user, false, true, Some("registerClientAsUser"))?;
        let trusted = self.host.check_permission(Some(CONFIGURE_WIFI_DISPLAY), uid)? == 0;
        let Some(Binder::Handle(handle)) = client else {
            return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "client"));
        };
        if self.clients.lock().unwrap().contains_key(&handle) {
            return Ok(());
        }
        let binder = self.host.process.strong(handle);
        let clients = Arc::downgrade(&self.clients);
        let process = self.host.process.clone();
        let death = self.host.process.link_to_death(
            &binder,
            Box::new(move || {
                // `ClientRecord.binderDied`: `unregisterClientLocked`.
                if let Some(clients) = clients.upgrade()
                    && let Some(c) = clients.lock().unwrap().remove(&handle)
                {
                    process.clear_death(&c.binder, c.death);
                }
            }),
        );
        self.clients
            .lock()
            .unwrap()
            .insert(handle, Client { binder, death, uid, user, trusted, route_types: 0, active_scan: false, selected_route: None });
        Ok(())
    }

    fn unregister(&self, handle: u32) {
        if let Some(c) = self.clients.lock().unwrap().remove(&handle) {
            self.host.process.clear_death(&c.binder, c.death);
        }
    }
}

/// The handle of a client argument.
fn handle(client: &Option<Binder>) -> Option<u32> {
    match client {
        Some(Binder::Handle(h)) => Some(*h),
        _ => None,
    }
}

impl Service for MediaRouter {
    fn descriptor(&self) -> &str {
        mr::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            mr::REGISTER_CLIENT_AS_USER => {
                let a = mr::RegisterClientAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.register(call, a.client, a.package_name, a.user_id).map(|()| p.write_no_exception())
            }
            mr::UNREGISTER_CLIENT => {
                let a = mr::UnregisterClient::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(h) = handle(&a.client) {
                    self.unregister(h);
                }
                p.write_no_exception();
                Ok(())
            }
            // A client without CONFIGURE_WIFI_DISPLAY is ignored; a trusted
            // one's group (`registerClientGroupIdLocked`) is not modelled.
            mr::REGISTER_CLIENT_GROUP_ID => {
                mr::RegisterClientGroupId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.check_permission(Some(CONFIGURE_WIFI_DISPLAY), call.sender_euid as i32) {
                    Ok(0) => Err(self.host.unimplemented_named("media_router", "a client group (#1663)")),
                    Ok(_) => {
                        eprintln!(
                            "app host: Ignoring client group request because the client doesn't have the CONFIGURE_WIFI_DISPLAY permission."
                        );
                        p.write_no_exception();
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            // `ClientRecord.getState`: a trusted client's routes, none
            // without providers; nothing for another.
            mr::GET_STATE => {
                let a = mr::GetState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let trusted = handle(&a.client).and_then(|h| self.clients.lock().unwrap().get(&h).map(|c| c.trusted));
                mr::write_get_state_reply(&mut p, (trusted == Some(true)).then_some(&NoRoutes));
                Ok(())
            }
            mr::IS_PLAYBACK_ACTIVE => {
                let a = mr::IsPlaybackActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = handle(&a.client).and_then(|h| self.clients.lock().unwrap().get(&h).map(|c| c.uid));
                let active = uid.is_some_and(|uid| self.host.playback.lock().unwrap().uid_active(uid));
                mr::write_is_playback_active_reply(&mut p, active);
                Ok(())
            }
            // `setDiscoveryRequestLocked`: only the system may discover
            // remote displays, which binds the providers
            // (`updateDiscoveryRequest`); without one, nothing is
            // discovered.
            mr::SET_DISCOVERY_REQUEST => {
                let a = mr::SetDiscoveryRequest::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let client = handle(&a.client).and_then(|h| self.clients.lock().unwrap().get(&h).map(|c| (c.trusted, c.user)));
                let discovering = client.filter(|(trusted, _)| *trusted && a.route_types & ROUTE_TYPE_REMOTE_DISPLAY != 0);
                match discovering.map_or(Ok(0), |(_, user)| self.providers(user)) {
                    Ok(0) => {
                        if let Some(h) = handle(&a.client)
                            && let Some(c) = self.clients.lock().unwrap().get_mut(&h)
                        {
                            c.route_types = if c.trusted { a.route_types } else { a.route_types & !ROUTE_TYPE_REMOTE_DISPLAY };
                            c.active_scan = a.active_scan;
                        }
                        p.write_no_exception();
                        Ok(())
                    }
                    Ok(_) => Err(self.host.unimplemented_named("media_router", "binding a remote display provider (#1663)")),
                    Err(e) => Err(e),
                }
            }
            // `setSelectedRouteLocked`: the system routes (the default audio
            // and Bluetooth A2DP routes) are not global; a trusted client's
            // explicit change of a global route selects it at its provider,
            // which is not bound.
            mr::SET_SELECTED_ROUTE => {
                let a = mr::SetSelectedRoute::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let packages = &self.host.inputs()?.packages;
                    let string = |name: &str| {
                        packages
                            .framework_string(name)
                            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("no {name}")))
                    };
                    let system = [string("default_audio_route_id")?, string("bluetooth_a2dp_audio_route_id")?];
                    let global = |id: &Option<String>| id.clone().filter(|id| !system.contains(id));
                    let mut clients = self.clients.lock().unwrap();
                    if let Some(c) = handle(&a.client).and_then(|h| clients.get_mut(&h)) {
                        let old = global(&c.selected_route);
                        let new = global(&a.route_id);
                        if new != old && a.explicit && c.trusted {
                            return Err(self.host.unimplemented_named("media_router", "selecting a global route (#1663)"));
                        }
                        c.selected_route = a.route_id;
                    }
                    p.write_no_exception();
                    Ok(())
                })()
            }
            code => Err(self.host.unimplemented_named(
                "media_router",
                &format!("{} (#1663)", mr::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n)),
            )),
        };
        reply(result.map(|()| p))
    }
}
