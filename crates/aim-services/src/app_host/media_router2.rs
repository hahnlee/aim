//! `media_router`'s `MediaRouter2` system routes (`MediaRouter2ServiceImpl`
//! and `SystemMediaRoute2Provider` at `android-16.0.0_r1`): the device
//! route and the system session a router starts with (`getSystemRoutes`,
//! `getSystemSessionInfo`). With no Bluetooth adapter, the device route
//! comes from `LegacyDeviceRouteController`, over the audio service's
//! routes: the app host's one output is the built-in speaker. Its volume is
//! the music stream's, read when asked (`updateVolume` on
//! `VOLUME_CHANGED_ACTION`). Without the mirroring flag the system session
//! selects the device route and has nothing to transfer to.
//!
//! Routers (`registerRouter2`) are told the routes and system session they
//! may see (`notifyRouterRegistered`): the route providers' routes
//! (`media_route_providers`), then the system provider's.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::{Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use super::AppHost;

/// `SystemMediaRoute2Provider.COMPONENT_NAME.flattenToShortString()`, its
/// `mUniqueId`, and its package (the provider package of its routes).
const PROVIDER_ID: &str = "com.android.server.media/.SystemMediaRoute2Provider";
const PROVIDER_PACKAGE: &str = "com.android.server.media";
const SYSTEM_SESSION_ID: &str = "SYSTEM_SESSION";
/// `MediaRoute2Info.ROUTE_ID_DEVICE`, `ROUTE_ID_DEFAULT`, its constants.
const ROUTE_ID_DEVICE: &str = "DEVICE_ROUTE";
const ROUTE_ID_DEFAULT: &str = "DEFAULT_ROUTE";
const TYPE_BUILTIN_SPEAKER: i32 = 2;
const FLAG_ROUTING_TYPE_REMOTE: i32 = 1 << 2;
const CONNECTION_STATE_CONNECTED: i32 = 2;
const PLAYBACK_VOLUME_FIXED: i32 = 0;
const PLAYBACK_VOLUME_VARIABLE: i32 = 1;
const SUITABILITY_STATUS_SUITABLE_FOR_DEFAULT_TRANSFER: i32 = 0;
const FEATURES: [&str; 3] = [
    "android.media.route.feature.LIVE_AUDIO",
    "android.media.route.feature.LIVE_VIDEO",
    "android.media.route.feature.LOCAL_PLAYBACK",
];
/// `RoutingSessionInfo.TRANSFER_REASON_FALLBACK`; `UserHandle.USER_NULL`.
const TRANSFER_REASON_FALLBACK: i32 = 0;
const USER_NULL: i32 = -10000;
const MODIFY_AUDIO_ROUTING: &str = "android.permission.MODIFY_AUDIO_ROUTING";
const BLUETOOTH_PERMISSIONS_FOR_SYSTEM_ROUTING: [&str; 2] = ["android.permission.BLUETOOTH_CONNECT", "android.permission.BLUETOOTH_SCAN"];

/// A `MediaRoute2Info` of the system provider.
#[derive(Clone)]
pub(super) struct Route {
    id: &'static str,
    name: String,
    is_system: bool,
    provider_package: Option<&'static str>,
    volume_handling: i32,
    volume_max: i32,
    volume: i32,
    suitability: i32,
}

impl Route {
    /// `getId`.
    pub(super) fn unique_id(&self) -> String {
        format!("{PROVIDER_ID}:{}", self.id)
    }
}

impl WriteParcelable for Route {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(self.id));
        crate::clip::write_char_sequence(p, Some(&self.name));
        p.write_i32(FEATURES.len() as i32);
        for f in FEATURES {
            p.write_string16(Some(f));
        }
        p.write_i32(TYPE_BUILTIN_SPEAKER);
        p.write_i32(FLAG_ROUTING_TYPE_REMOTE);
        p.write_bool(self.is_system);
        p.write_string16(None); // icon URI: `writeParcelable(null)`
        crate::clip::write_char_sequence(p, None);
        p.write_i32(CONNECTION_STATE_CONNECTED);
        p.write_string16(None); // client package
        p.write_string16(self.provider_package);
        p.write_i32(self.volume_handling);
        p.write_i32(self.volume_max);
        p.write_i32(self.volume);
        p.write_string16(None); // address
        p.write_i32(0); // deduplication ids
        p.write_i32(-1); // extras: `writeBundle(null)`
        p.write_string16(Some(PROVIDER_ID));
        p.write_bool(false); // visibility restricted
        p.write_i32(0); // allowed packages
        p.write_i32(0); // required permissions
        p.write_i32(self.suitability);
    }
}

/// A `RoutingSessionInfo` of the system provider.
pub(super) struct Session {
    /// None as `getSystemSessionInfo` clears it; the provider's own
    /// sessions have an empty one.
    client_package: Option<String>,
    /// Its selected route, by its original id.
    selected: &'static str,
}

impl WriteParcelable for Session {
    fn write_to(&self, p: &mut Parcel) {
        let unique = format!("{PROVIDER_ID}:{}", self.selected);
        p.write_string16(Some(SYSTEM_SESSION_ID));
        crate::clip::write_char_sequence(p, None);
        p.write_string16(None); // owner package
        p.write_string16(self.client_package.as_deref());
        p.write_string16(Some(PROVIDER_ID));
        p.write_i32(1);
        p.write_string16(Some(&unique));
        for _ in 0..3 {
            p.write_i32(0); // selectable, deselectable, transferable
        }
        p.write_i32(PLAYBACK_VOLUME_FIXED);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(-1); // control hints
        p.write_bool(true);
        p.write_i32(TRANSFER_REASON_FALLBACK);
        p.write_i32(USER_NULL);
        p.write_string16(None);
    }
}

/// A `RouteDiscoveryPreference`: its preferred features and whether it
/// asks for an active scan.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Preference {
    pub features: Vec<String>,
    pub active_scan: bool,
}

impl aim_service_aidl::ReadParcelable for Preference {
    fn read_from(r: &mut aim_binder_host::parcel::Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let list = |r: &mut aim_binder_host::parcel::Reader<'_>| -> aim_binder_host::parcel::Result<Vec<String>> {
            let n = r.read_i32()?;
            (0..n.max(0)).map(|_| Ok(r.read_string16()?.unwrap_or_default())).collect()
        };
        let features = list(r)?;
        list(r)?; // package order
        list(r)?; // allowed packages
        let active_scan = r.read_bool()?;
        crate::clip::bundle(r)?; // extras
        Ok(Preference { features, active_scan })
    }
}

impl AppHost {
    /// `checkCallerHasSystemRoutingPermissions`.
    pub(super) fn has_system_routing_permissions(&self, uid: i32) -> Result<bool, Exception> {
        if self.check_permission(Some(MODIFY_AUDIO_ROUTING), uid)? == 0 {
            return Ok(true);
        }
        for permission in BLUETOOTH_PERMISSIONS_FOR_SYSTEM_ROUTING {
            if self.check_permission(Some(permission), uid)? != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `LegacyDeviceRouteController.createRouteFromAudioInfo` of the
    /// speaker route, at the music stream's volume.
    fn device_route(&self) -> Result<Route, Exception> {
        let inputs = self.inputs()?;
        let name = inputs.packages.framework_string("default_audio_route_name").unwrap_or_default();
        // `getBuiltInSpeakerSuitabilityStatus`: the configured status, if
        // a known one.
        let suitability = match inputs.packages.framework_integer("config_mediaRouter_builtInSpeakerSuitability") {
            Some(s @ 0..=2) => s,
            _ => SUITABILITY_STATUS_SUITABLE_FOR_DEFAULT_TRANSFER,
        };
        let (volume, volume_max, fixed) = self.music_volume()?;
        Ok(Route {
            id: ROUTE_ID_DEVICE,
            name,
            is_system: false,
            provider_package: None,
            volume_handling: if fixed { PLAYBACK_VOLUME_FIXED } else { PLAYBACK_VOLUME_VARIABLE },
            volume_max,
            volume,
            suitability,
        })
    }

    /// The system provider's route as `setProviderState` gives it
    /// (`setUniqueId` and `setSystemRouteProvider`), and its default route.
    pub(super) fn media_router2_system_route_forms(&self) -> Result<(Route, Route), Exception> {
        let route = self.device_route()?;
        let default = Route { id: ROUTE_ID_DEFAULT, is_system: true, ..route };
        Ok((Route { id: ROUTE_ID_DEVICE, is_system: true, provider_package: Some(PROVIDER_PACKAGE), ..default.clone() }, default))
    }

    /// `getSystemRoutes(callerPackageName, false)`: the system provider's
    /// routes (its device route) for a caller with system routing
    /// permissions, else its default route.
    pub(super) fn media_router2_system_routes(&self, uid: i32) -> Result<Vec<Route>, Exception> {
        let (privileged, default) = self.media_router2_system_route_forms()?;
        Ok(vec![if self.has_system_routing_permissions(uid)? { privileged } else { default }])
    }

    /// `notifyRouterRegistered`'s routes and system session for a router
    /// of `uid`: the system provider's (its session, client package
    /// empty) with system routing permissions, its default route and
    /// session else.
    pub(super) fn media_router2_registered_state(&self, uid: i32) -> Result<(Vec<Route>, Session), Exception> {
        let routes = self.media_router2_system_routes(uid)?;
        let selected = if self.has_system_routing_permissions(uid)? { ROUTE_ID_DEVICE } else { ROUTE_ID_DEFAULT };
        Ok((routes, Session { client_package: Some(String::new()), selected }))
    }

    /// `getSystemSessionInfo(null, null, false)`: the system session for
    /// a caller with system routing permissions, its default session
    /// (selecting the default route) else, with no client package.
    pub(super) fn media_router2_system_session(&self, uid: i32) -> Result<Session, Exception> {
        let selected = if self.has_system_routing_permissions(uid)? { ROUTE_ID_DEVICE } else { ROUTE_ID_DEFAULT };
        Ok(Session { client_package: None, selected })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default session as `RoutingSessionInfo.writeToParcel` writes
    /// it: its unique selected route id, no other routes, a fixed volume,
    /// a system session with no transfer initiator.
    #[test]
    fn the_default_session_is_written_as_the_original_writes_it() {
        let mut p = Parcel::new();
        Session { client_package: None, selected: ROUTE_ID_DEFAULT }.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string16().unwrap().as_deref(), Some(SYSTEM_SESSION_ID));
        assert_eq!(crate::clip::char_sequence(&mut r).unwrap(), None);
        assert_eq!((r.read_string16().unwrap(), r.read_string16().unwrap()), (None, None));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some(PROVIDER_ID));
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("com.android.server.media/.SystemMediaRoute2Provider:DEFAULT_ROUTE"));
        let rest: Vec<i32> = (0..7).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(rest, [0, 0, 0, PLAYBACK_VOLUME_FIXED, 0, 0, -1]);
        assert!(r.read_bool().unwrap());
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (TRANSFER_REASON_FALLBACK, USER_NULL));
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.remaining(), 0);
    }
}
