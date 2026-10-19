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

/// `MediaRouter2ServiceImpl.RouterRecord`.
struct Router {
    binder: Strong,
    death: u64,
    package: String,
    /// `mDiscoveryPreference`.
    preference: super::media_router2::Preference,
}

/// `IMediaRouter2`'s descriptor and `notifyRouterRegistered`.
const IMEDIA_ROUTER2: &str = "android.media.IMediaRouter2";
const NOTIFY_ROUTER_REGISTERED: u32 = 1;

pub struct MediaRouter {
    host: Arc<AppHost>,
    /// `mAllClientRecords`, by handle.
    clients: Arc<Mutex<HashMap<u32, Client>>>,
    /// `mAllRouterRecords`, by handle.
    routers: Arc<Mutex<HashMap<u32, Router>>>,
}

impl MediaRouter {
    pub fn new(host: Arc<AppHost>) -> Self {
        MediaRouter { host, clients: Arc::new(Mutex::new(HashMap::new())), routers: Arc::new(Mutex::new(HashMap::new())) }
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

    /// `registerRouter2`: the router kept until it dies or unregisters, and
    /// told what it may see (`notifyRouterRegistered`).
    fn register_router(&self, uid: i32, router: Option<Binder>, package: Option<String>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = router else {
            return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "router must not be null"));
        };
        if package.as_deref().is_none_or(str::is_empty) {
            return Err(Exception::illegal_argument("packageName must not be empty"));
        }
        if self.routers.lock().unwrap().contains_key(&handle) {
            eprintln!("app host: media_router: registerRouter2Locked: Same router already exists. packageName={}", package.unwrap_or_default());
            return Ok(());
        }
        let (routes, session) = self.host.media_router2_registered_state(uid)?;
        let binder = self.host.process.strong(handle);
        let routers = Arc::downgrade(&self.routers);
        let process = self.host.process.clone();
        let death = self.host.process.link_to_death(
            &binder,
            Box::new(move || {
                // `RouterRecord.binderDied`: `unregisterRouter2Locked`.
                if let Some(routers) = routers.upgrade()
                    && let Some(r) = routers.lock().unwrap().remove(&handle)
                {
                    process.clear_death(&r.binder, r.death);
                }
            }),
        );
        let mut p = Parcel::new();
        p.write_interface_token(IMEDIA_ROUTER2);
        p.write_i32(routes.len() as i32);
        for route in &routes {
            p.write_i32(1);
            route.write_to(&mut p);
        }
        p.write_i32(1);
        session.write_to(&mut p);
        if let Err(e) = binder.transact(NOTIFY_ROUTER_REGISTERED, &p, true) {
            eprintln!("app host: media_router: notifyRegistered: {e:?}");
        }
        let package = package.unwrap_or_default();
        self.routers.lock().unwrap().insert(handle, Router { binder, death, package, preference: Default::default() });
        Ok(())
    }

    /// `setDiscoveryRequestWithRouter2`: the router's preference, then the
    /// composite one for the providers (`updateDiscoveryPreferenceOnHandler`).
    fn set_router_discovery(&self, uid: i32, router: Option<Binder>, preference: Option<super::media_router2::Preference>) -> Result<(), Exception> {
        let (Some(h), Some(preference)) = (handle(&router), preference) else {
            return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "router and preference must not be null"));
        };
        let mut routers = self.routers.lock().unwrap();
        let Some(r) = routers.get_mut(&h) else {
            eprintln!("app host: media_router: setDiscoveryRequestWithRouter2: Ignoring unknown router.");
            return Ok(());
        };
        if r.preference == preference {
            return Ok(());
        }
        let previous = std::mem::replace(&mut r.preference, preference);
        let all: Vec<_> = routers.values().map(|r| (r.package.clone(), r.preference.clone())).collect();
        let result = self.host.media_router2_discover(uid / PER_USER_RANGE as i32, &all);
        if result.is_err()
            && let Some(r) = routers.get_mut(&h)
        {
            r.preference = previous;
        }
        result
    }

    /// `unregisterRouter2`.
    fn unregister_router(&self, handle: u32) {
        if let Some(r) = self.routers.lock().unwrap().remove(&handle) {
            self.host.process.clear_death(&r.binder, r.death);
        }
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
            // `MediaRouterService.getSystemRoutes`: of the caller's own
            // package (`validatePackageName`); a proxy router's is not
            // implemented.
            mr::GET_SYSTEM_ROUTES => {
                let a = mr::GetSystemRoutes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                (|| {
                    let package = a.caller_package_name.as_deref().unwrap_or_default();
                    if a.caller_package_name.is_none() || !self.host.owns_package(uid, package)? {
                        return Err(Exception::security("callerPackageName does not match calling uid."));
                    }
                    if a.is_proxy_router {
                        return Err(self.host.unimplemented_named("media_router", "a proxy router's system routes (#1663)"));
                    }
                    let routes: Vec<_> = self.host.media_router2_system_routes(uid)?.into_iter().map(Some).collect();
                    mr::write_get_system_routes_reply(&mut p, Some(&routes[..]));
                    Ok(())
                })()
            }
            mr::REGISTER_ROUTER2 => {
                let a = mr::RegisterRouter2::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.register_router(call.sender_euid as i32, a.router, a.package_name).map(|()| p.write_no_exception())
            }
            mr::SET_DISCOVERY_REQUEST_WITH_ROUTER2 => {
                let a = mr::SetDiscoveryRequestWithRouter2::<super::media_router2::Preference>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_router_discovery(call.sender_euid as i32, a.router, a.preference).map(|()| p.write_no_exception())
            }
            mr::UNREGISTER_ROUTER2 => {
                let a = mr::UnregisterRouter2::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match handle(&a.router) {
                    Some(h) => {
                        self.unregister_router(h);
                        p.write_no_exception();
                        Ok(())
                    }
                    None => Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "router must not be null")),
                }
            }
            mr::GET_SYSTEM_SESSION_INFO => {
                mr::GetSystemSessionInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host
                    .media_router2_system_session(call.sender_euid as i32)
                    .map(|s| mr::write_get_system_session_info_reply(&mut p, Some(&s)))
            }
            code => Err(self.host.unimplemented_named(
                "media_router",
                &format!("{} (#1663)", mr::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n)),
            )),
        };
        reply(result.map(|()| p))
    }
}
