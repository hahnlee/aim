//! `wifiscanner`: WifiScanningServiceImpl for the app host
//! (`android-16.0.0_r1`). Its scanner impls are made for the chip's client
//! interfaces (`setupScannerImpls`), and the app host has none: no
//! wificond or vendor HAL scans, and the Mac's own scans are not read
//! (#1686). So the three state machines stay in their `DefaultState`, as
//! the original's do without a scanner impl: a single or background scan
//! fails as "not available", a PNO scan too, no scan result is cached and
//! no channel is available. The checks are the original's
//! (`enforcePermission`, `WifiPermissionsUtil`), and its clients are kept
//! by their listeners as `mClients` keeps them, until they die or stop.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_net_wifi_iscandatalistener as sdl;
use aim_service_aidl::android_net_wifi_iwifiscanner as ws;
use aim_service_aidl::android_net_wifi_iwifiscannerlistener as wl;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::appops_service::{self, MODE_ALLOWED};
use super::permission_checker::{self, AttributionSourceState};
use super::power_service::WorkSource;
use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::{self, Value};
use crate::package::list_slice::Slice;

const NETWORK_STACK: &str = "android.permission.NETWORK_STACK";
const MAINLINE_NETWORK_STACK: &str = "android.permission.MAINLINE_NETWORK_STACK";
const NETWORK_SETTINGS: &str = "android.permission.NETWORK_SETTINGS";
const NETWORK_SETUP_WIZARD: &str = "android.permission.NETWORK_SETUP_WIZARD";
const NETWORK_MANAGED_PROVISIONING: &str = "android.permission.NETWORK_MANAGED_PROVISIONING";
const RADIO_SCAN_WITHOUT_LOCATION: &str = "android.permission.RADIO_SCAN_WITHOUT_LOCATION";
const LOCATION_HARDWARE: &str = "android.permission.LOCATION_HARDWARE";
const ACCESS_FINE_LOCATION: &str = "android.permission.ACCESS_FINE_LOCATION";
const NEARBY_WIFI_DEVICES: &str = "android.permission.NEARBY_WIFI_DEVICES";
const RENOUNCE_PERMISSIONS: &str = "android.permission.RENOUNCE_PERMISSIONS";
/// `AppOpsManager.OP_FINE_LOCATION`, `OP_WIFI_SCAN`.
const OP_FINE_LOCATION: i32 = 1;
const OP_WIFI_SCAN: i32 = 10;
/// `ParsedUsesPermission.FLAG_NEVER_FOR_LOCATION`.
const USES_PERMISSION_NEVER_FOR_LOCATION: i32 = 0x10000;
/// `Process.ROOT_UID`, `SYSTEM_UID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
/// `Build.VERSION_CODES.UPSIDE_DOWN_CAKE`.
const U: i32 = 34;
/// `WifiScanner.REASON_*`.
const REASON_UNSPECIFIED: i32 = -1;
const REASON_INVALID_REQUEST: i32 = -3;
const REASON_NOT_AUTHORIZED: i32 = -4;
/// `WifiScanner.WIFI_BAND_UNSPECIFIED`.
const WIFI_BAND_UNSPECIFIED: i32 = 0;
/// `WifiScanner.SCAN_TYPE_LOW_LATENCY`, `SCAN_TYPE_HIGH_ACCURACY`.
const SCAN_TYPE_LOW_LATENCY: i32 = 0;
const SCAN_TYPE_HIGH_ACCURACY: i32 = 2;
/// `WifiScanner.GET_AVAILABLE_CHANNELS_EXTRA`.
const GET_AVAILABLE_CHANNELS_EXTRA: &str = "Channels";
/// `WifiManager.EXTRA_PARAM_KEY_ATTRIBUTION_SOURCE`.
const EXTRA_PARAM_KEY_ATTRIBUTION_SOURCE: &str = "EXTRA_PARAM_KEY_ATTRIBUTION_SOURCE";
const ATTRIBUTION_SOURCE: &str = "android.content.AttributionSource";
const SCAN_RESULT: &str = "android.net.wifi.ScanResult";
/// `WifiScanningService.TAG`.
const TAG: &str = "WifiScanningService";

/// No scan result, of which an empty list is written.
enum NoScanResult {}

impl WriteParcelable for NoScanResult {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

/// `writeByteArray`, `createIntArray`, `readString`: read past.
fn skip_array(r: &mut Reader<'_>, item: usize) -> ParcelResult<()> {
    let n = r.read_i32()?;
    if n > 0 {
        r.skip((n as usize * item).next_multiple_of(4))?;
    }
    Ok(())
}

/// `WifiScanner.ScanSettings`, of what its checks read.
struct ScanSettings {
    band: i32,
    scan_type: i32,
    ignore_location_settings: bool,
    hide_from_app_ops: bool,
    channels: i32,
    hidden_networks: i32,
}

impl ReadParcelable for ScanSettings {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let band = r.read_i32()?;
        // periodInMs, reportEvents, numBssidsPerScan, maxScansToCache,
        // maxPeriodInMs, stepCount, isPnoScan.
        for _ in 0..7 {
            r.read_i32()?;
        }
        let scan_type = r.read_i32()?;
        let ignore_location_settings = r.read_i32()? == 1;
        let hide_from_app_ops = r.read_i32()? == 1;
        r.read_i32()?; // mRnrSetting
        r.read_bool()?; // mEnable6GhzPsc
        let channels = r.read_i32()?;
        for _ in 0..channels.max(0) {
            // frequency, dwellTimeMS, passive.
            for _ in 0..3 {
                r.read_i32()?;
            }
        }
        let hidden_networks = r.read_i32()?;
        for _ in 0..hidden_networks.max(0) {
            r.read_string16()?;
        }
        // mVendorIes (`writeTypedList` of `InformationElement`: id, idExt,
        // bytes).
        for _ in 0..r.read_i32()?.max(0) {
            if r.read_i32()? != 0 {
                r.read_i32()?;
                r.read_i32()?;
                skip_array(r, 1)?;
            }
        }
        Ok(ScanSettings {
            band,
            scan_type,
            ignore_location_settings,
            hide_from_app_ops,
            channels: channels.max(0),
            hidden_networks: hidden_networks.max(0),
        })
    }
}

/// `WifiScanner.PnoSettings`, read past: a PNO scan never starts here.
struct PnoSettings;

impl ReadParcelable for PnoSettings {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // isConnected, min5GHzRssi, min24GHzRssi, min6GHzRssi,
        // scanIterations, scanIntervalMultiplier.
        for _ in 0..6 {
            r.read_i32()?;
        }
        for _ in 0..r.read_i32()?.max(0) {
            r.read_string16()?; // ssid
            r.read_i32()?; // flags
            r.read_i32()?; // authBitField
            skip_array(r, 4)?; // frequencies
        }
        Ok(PnoSettings)
    }
}

/// A bundle's entries, read as `Bundle.readFromParcel` keeps them.
struct Extras(HashMap<String, Value>);

impl ReadParcelable for Extras {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Extras(bundle::read(r)?.unwrap_or_default()))
    }
}

/// `new Bundle()` with `putIntegerArrayList(GET_AVAILABLE_CHANNELS_EXTRA,
/// list)`.
struct Channels(Vec<i32>);

impl WriteParcelable for Channels {
    fn write_to(&self, p: &mut Parcel) {
        bundle::write(
            p,
            &[(GET_AVAILABLE_CHANNELS_EXTRA, Value::IntList(self.0.clone()))],
        );
    }
}

/// `WifiNative.getCachedScanResultsFromAllClientIfaces` without a client
/// interface: `new ScanData()` (`writeToParcel`: its id, flags, buckets
/// scanned and bands, then a `ParceledListSlice` of no result).
struct NoScanData(Slice<NoScanResult>);

impl WriteParcelable for NoScanData {
    fn write_to(&self, p: &mut Parcel) {
        for _ in 0..4 {
            p.write_i32(0);
        }
        self.0.write_to(p);
    }
}

/// `ExternalClientInfo`: a client by its listener.
struct Client {
    uid: i32,
    package: Option<String>,
    strong: Arc<Strong>,
    death: u64,
}

pub struct WifiScanner {
    host: Arc<AppHost>,
    /// `mClients`, by the listener's handle.
    clients: Mutex<HashMap<u32, Client>>,
    /// `mSingleScanListeners`' clients.
    listeners: Mutex<Vec<u32>>,
    /// `mVerboseLoggingEnabled`.
    verbose: AtomicBool,
    me: Weak<WifiScanner>,
}

impl WifiScanner {
    pub fn new(host: Arc<AppHost>) -> Arc<WifiScanner> {
        Arc::new_cyclic(|me| WifiScanner {
            host,
            clients: Mutex::default(),
            listeners: Mutex::default(),
            verbose: AtomicBool::new(false),
            me: me.clone(),
        })
    }

    fn held(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    fn local_log(&self, message: &str) {
        if self.verbose.load(Ordering::Relaxed) {
            eprintln!("app host: {TAG}: {message}");
        }
    }

    /// `WifiPermissionsUtil.checkPackage`.
    fn check_package(&self, uid: i32, package: Option<&str>) -> Result<String, Exception> {
        let package = package.ok_or_else(|| {
            Exception::security(format!("Checking UID {uid} but Package Name is Null"))
        })?;
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        Ok(package.to_string())
    }

    /// `noteAppOpAllowed(op, package, featureId, uid, null)`.
    fn note_allowed(
        &self,
        op: i32,
        package: &str,
        tag: Option<&str>,
        uid: i32,
    ) -> Result<bool, Exception> {
        Ok(
            appops_service::note_from(&self.host, SYSTEM_UID, op, uid, Some(package), tag)?
                == MODE_ALLOWED,
        )
    }

    /// `WifiPermissionsUtil.enforceCanAccessScanResultsForWifiScanner`.
    fn enforce_can_access_scan_results(
        &self,
        package: Option<&str>,
        tag: Option<&str>,
        uid: i32,
        ignore_location_settings: bool,
        hide_from_app_ops: bool,
    ) -> Result<(), Exception> {
        let package = self.check_package(uid, package)?;
        if self.held(NETWORK_SETTINGS, uid)? || self.held(NETWORK_SETUP_WIZARD, uid)? {
            return Ok(());
        }
        if !super::wifi_service::location_mode_enabled(&self.host)? {
            if ignore_location_settings {
                eprintln!("app host: {TAG}: Request from {package} violated location settings");
            } else {
                return Err(Exception::security(
                    "Location mode is disabled for the device",
                ));
            }
        }
        // `checkCallersFineLocationPermission`, then the hardware's.
        let fine = self.held(ACCESS_FINE_LOCATION, uid)?
            && (if hide_from_app_ops {
                appops_service::mode(&self.host, OP_FINE_LOCATION, uid, Some(&package))?
                    == MODE_ALLOWED
            } else {
                self.note_allowed(OP_FINE_LOCATION, &package, tag, uid)?
            } || ignore_location_settings);
        if !fine || !self.held(LOCATION_HARDWARE, uid)? {
            return Err(Exception::security(format!(
                "UID {uid} has no location permission"
            )));
        }
        if !self.note_allowed(OP_WIFI_SCAN, &package, tag, uid)? {
            return Err(Exception::security(format!(
                "UID {uid} has no wifi scan permission"
            )));
        }
        Ok(())
    }

    /// `enforcePermission`: the wifi stack's requests (NETWORK_STACK),
    /// else a privileged request is refused and the rest need scan
    /// results' access.
    fn enforce_permission(
        &self,
        uid: i32,
        package: Option<&str>,
        tag: Option<&str>,
        privileged: bool,
        ignore_location_settings: bool,
        hide_from_app_ops: bool,
    ) -> Result<(), Exception> {
        if self.held(NETWORK_STACK, uid)? {
            return Ok(());
        }
        if privileged {
            return Err(Exception::security(format!(
                "NetworkStack: Neither user {uid} nor current process has {NETWORK_STACK}."
            )));
        }
        self.enforce_can_access_scan_results(
            package,
            tag,
            uid,
            ignore_location_settings,
            hide_from_app_ops,
        )
    }

    /// `enforcePermission` as the binder calls catch it: whether the
    /// caller is authorized, a failure logged as theirs are.
    fn authorized(
        &self,
        call: &str,
        uid: i32,
        package: Option<&str>,
        tag: Option<&str>,
        flags: (bool, bool, bool),
    ) -> Result<bool, Exception> {
        let (privileged, ignore, hide) = flags;
        match self.enforce_permission(uid, package, tag, privileged, ignore, hide) {
            Ok(()) => Ok(true),
            Err(e) if e.code == aim_binder_host::parcel::EX_SECURITY => {
                eprintln!(
                    "app host: {TAG}: {call}: failed to authorize app: {} uid {uid} AttributionTag {}",
                    package.unwrap_or("null"),
                    tag.unwrap_or("null")
                );
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// Calls a listener, one-way; one that died is not called.
    fn tell(&self, strong: &Strong, code: u32, write: impl FnOnce(&mut Parcel)) {
        let mut p = Parcel::new();
        write(&mut p);
        let _ = strong.transact(code, &p, true);
    }

    /// `notifyFailure` / `replyFailed`.
    fn fail(&self, strong: &Strong, reason: i32, description: &str) {
        self.tell(strong, wl::ON_FAILURE, |p| {
            wl::OnFailure {
                reason,
                description: Some(description.into()),
            }
            .write(p)
        });
    }

    fn listener_strong(&self, listener: Option<Binder>) -> Result<(u32, Arc<Strong>), Exception> {
        match listener {
            Some(Binder::Handle(handle)) => {
                let kept = self
                    .clients
                    .lock()
                    .unwrap()
                    .get(&handle)
                    .map(|c| c.strong.clone());
                Ok((
                    handle,
                    kept.unwrap_or_else(|| Arc::new(self.host.process.strong(handle))),
                ))
            }
            // The app host's own code makes no scan request.
            Some(Binder::Local(_)) => Err(Exception::illegal_argument(
                "wifiscanner: a listener of the app host's own",
            )),
            None => Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke interface method 'android.os.IBinder android.os.IInterface.asBinder()' on a null object reference",
            )),
        }
    }

    /// `ExternalClientInfo` (`linkToDeath`) and `register`: the client of
    /// `handle`, kept until it dies or is cleaned up.
    fn register(&self, handle: u32, strong: Arc<Strong>, uid: i32, package: Option<&str>) {
        let mut clients = self.clients.lock().unwrap();
        if clients.contains_key(&handle) {
            return;
        }
        let me = self.me.clone();
        let death = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(me) = me.upgrade() {
                    me.local_log(&format!("binder died: client listener: {handle}"));
                    me.cleanup(handle);
                }
            }),
        );
        self.local_log(&format!(
            "Registering listener= {handle} uid={uid} packageName={}",
            package.unwrap_or("null")
        ));
        clients.insert(
            handle,
            Client {
                uid,
                package: package.map(Into::into),
                strong,
                death,
            },
        );
    }

    /// `ExternalClientInfo.cleanup`: no PNO, single or background request
    /// is held for it, so it leaves the single scan listeners and
    /// `mClients`.
    fn cleanup(&self, handle: u32) {
        self.listeners.lock().unwrap().retain(|h| *h != handle);
        if let Some(c) = self.clients.lock().unwrap().remove(&handle) {
            self.host.process.clear_death(&c.strong, c.death);
            self.local_log(&format!(
                "Successfully stopped all requests for client ClientInfo[uid={}, package={}]",
                c.uid,
                c.package.as_deref().unwrap_or("null")
            ));
        }
    }

    fn registered(&self, handle: u32) -> bool {
        self.clients.lock().unwrap().contains_key(&handle)
    }

    /// `WifiPermissionsUtil.isTargetSdkLessThan`, false for a package
    /// unknown.
    fn target_sdk_less_than(&self, package: Option<&str>, sdk: i32) -> Result<bool, Exception> {
        let inputs = self.host.inputs()?;
        Ok(package
            .and_then(|p| inputs.packages.application_info(p, 0))
            .is_some_and(|ai| ai.target_sdk_version < sdk))
    }

    /// `WifiPermissionsUtil.enforceNearbyDevicesPermission(source, true,
    /// message)`.
    fn enforce_nearby_devices(
        &self,
        calling_uid: i32,
        source: Option<&AttributionSourceState>,
        message: &str,
    ) -> Result<(), Exception> {
        let source = source.ok_or_else(|| {
            Exception::security("enforceNearbyDevicesPermission attributionSource is null")
        })?;
        // `AttributionSource.checkCallingUid`; a user build prints the
        // source as `Object.toString` does.
        if calling_uid != ROOT_UID
            && calling_uid.rem_euclid(PER_USER_RANGE as i32) != SYSTEM_UID
            && calling_uid != source.uid
        {
            return Err(Exception::security(format!(
                "enforceNearbyDevicesPermission invalid attribution source={ATTRIBUTION_SOURCE}"
            )));
        }
        let uid = source.uid;
        let package = self.check_package(uid, source.package.as_deref())?;
        for bypass in [
            NETWORK_SETTINGS,
            NETWORK_SETUP_WIZARD,
            NETWORK_MANAGED_PROVISIONING,
            NETWORK_STACK,
            MAINLINE_NETWORK_STACK,
            RADIO_SCAN_WITHOUT_LOCATION,
        ] {
            if self.held(bypass, uid)? {
                return Ok(());
            }
        }
        if !permission_checker::data_delivery(
            &self.host,
            NEARBY_WIFI_DEVICES,
            source,
            Some(message),
        )? {
            return Err(Exception::security(format!(
                "package={package} UID={uid} does not have nearby devices permission."
            )));
        }
        // A renouncement of location anywhere along the chain disavows it.
        let mut current = Some(source);
        while let Some(s) = current {
            let renounced = s
                .renounced
                .iter()
                .flatten()
                .any(|p| p.as_deref() == Some(ACCESS_FINE_LOCATION));
            if renounced && self.held(RENOUNCE_PERMISSIONS, s.uid)? {
                return Ok(());
            }
            current = s
                .next
                .as_deref()
                .and_then(<[_]>::first)
                .and_then(Option::as_ref);
        }
        // So does `neverForLocation` on the package's NEARBY_WIFI_DEVICES.
        let (state, _) = self.host.packages()?;
        let never_for_location = state
            .packages
            .get(&package)
            .and_then(|ps| ps.pkg.as_deref())
            .is_some_and(|pkg| {
                pkg.uses_permissions.iter().any(|u| {
                    u.name.as_deref() == Some(NEARBY_WIFI_DEVICES)
                        && u.flags & USES_PERMISSION_NEVER_FOR_LOCATION != 0
                })
            });
        if never_for_location {
            return Ok(());
        }
        if !super::wifi_service::location_mode_enabled(&self.host)? {
            return Err(Exception::security(
                "Location mode is disabled for the device",
            ));
        }
        if permission_checker::data_delivery(
            &self.host,
            ACCESS_FINE_LOCATION,
            source,
            Some(message),
        )? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "package={package}, UID={uid} does not have Fine Location permission"
        )))
    }

    /// `getAvailableChannels`: no channel helper, so no channel.
    fn available_channels(
        &self,
        uid: i32,
        package: Option<&str>,
        tag: Option<&str>,
        source: Option<AttributionSourceState>,
        extras: bool,
    ) -> Result<Channels, Exception> {
        if self.target_sdk_less_than(package, U)? {
            let _system = aim_binder_host::local::clear_calling_identity();
            self.enforce_permission(uid, package, tag, false, false, false)?;
        } else {
            if !extras {
                return Err(Exception::new(
                    EX_NULL_POINTER,
                    "Attempt to invoke virtual method 'android.os.Parcelable android.os.Bundle.getParcelable(java.lang.String)' on a null object reference",
                ));
            }
            self.enforce_nearby_devices(
                uid,
                source.as_ref(),
                &format!("{TAG} getAvailableChannels"),
            )?;
        }
        Ok(Channels(Vec::new()))
    }

    /// `startScan` and the single scan state machine's
    /// `handleScanStartMessage` in its `DefaultState`.
    fn start_scan(
        &self,
        uid: i32,
        handle: u32,
        strong: Arc<Strong>,
        settings: Option<&ScanSettings>,
        package: Option<&str>,
        tag: Option<&str>,
    ) -> Result<(), Exception> {
        let ignore = settings.is_some_and(|s| s.ignore_location_settings);
        let hide = settings.is_some_and(|s| s.hide_from_app_ops);
        if !self.authorized("startScan", uid, package, tag, (false, ignore, hide))? {
            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
            return Ok(());
        }
        self.register(handle, strong.clone(), uid, package);
        // `validateScanRequest` reads the settings the client gave.
        let settings = settings.ok_or_else(|| {
            Exception::new(
                EX_NULL_POINTER,
                "Attempt to read from field 'int android.net.wifi.WifiScanner$ScanSettings.band' on a null object reference",
            )
        })?;
        let network_stack = self.held(NETWORK_STACK, uid)?;
        let valid = !(settings.band == WIFI_BAND_UNSPECIFIED && settings.channels == 0)
            && (SCAN_TYPE_LOW_LATENCY..=SCAN_TYPE_HIGH_ACCURACY).contains(&settings.scan_type)
            && (network_stack
                || settings.hidden_networks == 0 && settings.scan_type == SCAN_TYPE_LOW_LATENCY);
        if !valid {
            self.fail(&strong, REASON_INVALID_REQUEST, "bad request");
            self.cleanup(handle);
            return Ok(());
        }
        if !settings.ignore_location_settings {
            // Scanning is disabled: a regular request is rejected.
            self.fail(&strong, REASON_UNSPECIFIED, "not available");
            self.cleanup(handle);
            return Ok(());
        }
        // An emergency scan waits for the wifi manager to bring scanning
        // up (`setEmergencyScanRequestInProgress`).
        Err(self
            .host
            .unimplemented_named("wifiscanner", "an emergency scan (#1686)"))
    }
}

impl Service for WifiScanner {
    fn descriptor(&self) -> &str {
        ws::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            ws::GET_AVAILABLE_CHANNELS => {
                let a = ws::GetAvailableChannels::<Extras>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let source = match a
                    .extras
                    .as_ref()
                    .and_then(|e| e.0.get(EXTRA_PARAM_KEY_ATTRIBUTION_SOURCE))
                {
                    Some(Value::Lazy { start, .. }) => {
                        // `writeParcelable`: the class, then the parcel
                        // of its state.
                        call.data.set_position(*start);
                        if call.data.read_string16().map_err(|_| BAD_VALUE)?.as_deref()
                            != Some(ATTRIBUTION_SOURCE)
                        {
                            return Err(BAD_VALUE);
                        }
                        Some(
                            AttributionSourceState::read_from(&mut call.data)
                                .map_err(|_| BAD_VALUE)?,
                        )
                    }
                    _ => None,
                };
                self.available_channels(
                    uid,
                    a.package_name.as_deref(),
                    a.feature_id.as_deref(),
                    source,
                    a.extras.is_some(),
                )
                .map(|c| ws::write_get_available_channels_reply(&mut p, Some(&c)))
            }
            ws::IS_SCANNING => {
                ws::IsScanning::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.held(LOCATION_HARDWARE, uid) {
                    Ok(true) => {
                        ws::write_is_scanning_reply(&mut p, false);
                        Ok(())
                    }
                    Ok(false) => Err(Exception::security(format!(
                        "UID {uid} does not have hardware Location permission"
                    ))),
                    Err(e) => Err(e),
                }
            }
            ws::SET_SCANNING_ENABLED => {
                let a = ws::SetScanningEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.package_name.as_deref();
                match self.authorized(
                    "setScanningEnabled",
                    uid,
                    package,
                    None,
                    (true, false, false),
                ) {
                    Ok(false) => {
                        ws::write_set_scanning_enabled_reply(&mut p, false);
                        Ok(())
                    }
                    Ok(true) => {
                        self.local_log(&format!(
                            "enable scan: package {} tid {} enable {}",
                            package.unwrap_or("null"),
                            a.tid,
                            a.enable
                        ));
                        if a.enable {
                            // `setupScannerImpls` finds no client interface.
                            for machine in ["single scan", "bgscan scan", "pno scan"] {
                                eprintln!(
                                    "app host: {TAG}: Failed to start {machine} state machine because scanner impl is null"
                                );
                            }
                        }
                        ws::write_set_scanning_enabled_reply(&mut p, true);
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            ws::REGISTER_SCAN_LISTENER => {
                let a = ws::RegisterScanListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (package, tag) = (a.package_name.as_deref(), a.feature_id.as_deref());
                self.listener_strong(a.listener)
                    .and_then(|(handle, strong)| {
                        let flags = (false, false, false);
                        if !self.authorized("registerScanListener", uid, package, tag, flags)? {
                            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
                        } else if self.registered(handle) {
                            let tag = tag.unwrap_or("null");
                            eprintln!(
                                "app host: {TAG}: duplicate client connection: {uid}, \
                             listener={handle} AttributionTag {tag}"
                            );
                        } else {
                            self.register(handle, strong.clone(), uid, package);
                            self.listeners.lock().unwrap().push(handle);
                            self.tell(&strong, wl::ON_SUCCESS, |p| wl::OnSuccess {}.write(p));
                        }
                        ws::write_register_scan_listener_reply(&mut p);
                        Ok(())
                    })
            }
            ws::UNREGISTER_SCAN_LISTENER => {
                let a = ws::UnregisterScanListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (package, tag) = (a.package_name.as_deref(), a.feature_id.as_deref());
                self.listener_strong(a.listener)
                    .and_then(|(handle, strong)| {
                        if !self.authorized(
                            "unregisterScanListener",
                            uid,
                            package,
                            tag,
                            (false, true, false),
                        )? {
                            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
                        } else if !self.registered(handle) {
                            eprintln!(
                                "app host: {TAG}: no client registered: {uid}, listener={handle}"
                            );
                        } else {
                            self.cleanup(handle);
                        }
                        ws::write_unregister_scan_listener_reply(&mut p);
                        Ok(())
                    })
            }
            ws::START_BACKGROUND_SCAN => {
                let a = ws::StartBackgroundScan::<ScanSettings, WorkSource>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let (package, tag) = (a.package_name.as_deref(), a.feature_id.as_deref());
                self.listener_strong(a.listener)
                    .and_then(|(handle, strong)| {
                        if !self.authorized(
                            "startBackgroundScan",
                            uid,
                            package,
                            tag,
                            (false, false, false),
                        )? {
                            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
                        } else {
                            self.register(handle, strong.clone(), uid, package);
                            self.fail(&strong, REASON_UNSPECIFIED, "not available");
                        }
                        ws::write_start_background_scan_reply(&mut p);
                        Ok(())
                    })
            }
            ws::STOP_BACKGROUND_SCAN => {
                let a = ws::StopBackgroundScan::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (package, tag) = (a.package_name.as_deref(), a.feature_id.as_deref());
                self.listener_strong(a.listener)
                    .and_then(|(handle, strong)| {
                        if !self.authorized(
                            "stopBackgroundScan",
                            uid,
                            package,
                            tag,
                            (false, true, false),
                        )? {
                            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
                        } else if !self.registered(handle) {
                            eprintln!("app host: {TAG}: listener not found {handle}");
                        } else {
                            self.fail(&strong, REASON_UNSPECIFIED, "not available");
                        }
                        ws::write_stop_background_scan_reply(&mut p);
                        Ok(())
                    })
            }
            // Authorized, the request goes to the background scan state
            // machine, whose `DefaultState` has no client to answer.
            ws::GET_SCAN_RESULTS => {
                let a = ws::GetScanResults::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.authorized(
                    "getScanResults",
                    uid,
                    a.package_name.as_deref(),
                    a.feature_id.as_deref(),
                    (false, false, false),
                )
                .map(|ok| ws::write_get_scan_results_reply(&mut p, ok))
            }
            ws::START_SCAN => {
                let a = ws::StartScan::<ScanSettings, WorkSource>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.listener_strong(a.listener)
                    .and_then(|(handle, strong)| {
                        self.start_scan(
                            uid,
                            handle,
                            strong,
                            a.settings.as_ref(),
                            a.package_name.as_deref(),
                            a.feature_id.as_deref(),
                        )
                    })
                    .map(|()| ws::write_start_scan_reply(&mut p))
            }
            // No single scan request is held for a client.
            ws::STOP_SCAN => {
                let a = ws::StopScan::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (package, tag) = (a.package_name.as_deref(), a.feature_id.as_deref());
                self.listener_strong(a.listener)
                    .and_then(|(handle, strong)| {
                        if !self.authorized("stopScan", uid, package, tag, (false, true, false))? {
                            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
                        } else if !self.registered(handle) {
                            eprintln!("app host: {TAG}: listener not found {handle}");
                        }
                        ws::write_stop_scan_reply(&mut p);
                        Ok(())
                    })
            }
            // `filterCachedScanResultsByAge` of no cached result.
            ws::GET_SINGLE_SCAN_RESULTS => {
                let a = ws::GetSingleScanResults::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.authorized(
                    "getSingleScanResults",
                    uid,
                    a.package_name.as_deref(),
                    a.feature_id.as_deref(),
                    (false, false, false),
                )
                .map(|_| ws::write_get_single_scan_results_reply::<NoScanResult>(&mut p, Some(&[])))
            }
            ws::GET_CACHED_SCAN_DATA => {
                let a = ws::GetCachedScanData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.listener {
                    None => Err(Exception::new(EX_NULL_POINTER, "listener cannot be null")),
                    Some(listener) => self
                        .enforce_permission(
                            uid,
                            a.package_name.as_deref(),
                            a.feature_id.as_deref(),
                            false,
                            false,
                            false,
                        )
                        .and_then(|()| self.listener_strong(Some(listener)))
                        .map(|(_, strong)| {
                            let data = NoScanData(Slice::new(
                                self.host.process.clone(),
                                SCAN_RESULT,
                                Vec::new(),
                            ));
                            self.tell(&strong, sdl::ON_RESULT, |p| {
                                sdl::OnResult {
                                    scan_data: Some(data),
                                }
                                .write(p)
                            });
                            ws::write_get_cached_scan_data_reply(&mut p)
                        }),
                }
            }
            ws::START_PNO_SCAN | ws::STOP_PNO_SCAN => {
                let start = call.code == ws::START_PNO_SCAN;
                let (listener, package, tag) = if start {
                    let a = ws::StartPnoScan::<ScanSettings, PnoSettings>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.listener, a.package_name, a.feature_id)
                } else {
                    let a = ws::StopPnoScan::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.listener, a.package_name, a.feature_id)
                };
                let name = if start { "startPnoScan" } else { "stopPnoScan" };
                let done = |p: &mut Parcel| {
                    if start {
                        ws::write_start_pno_scan_reply(p)
                    } else {
                        ws::write_stop_pno_scan_reply(p)
                    }
                };
                match listener {
                    None => {
                        eprintln!("app host: {TAG}: listener is null");
                        done(&mut p);
                        Ok(())
                    }
                    listener => self.listener_strong(listener).and_then(|(handle, strong)| {
                        if !self.authorized(
                            name,
                            uid,
                            package.as_deref(),
                            tag.as_deref(),
                            (true, false, false),
                        )? {
                            self.fail(&strong, REASON_NOT_AUTHORIZED, "Not authorized");
                        } else if start || self.registered(handle) {
                            self.fail(&strong, REASON_UNSPECIFIED, "not available");
                        } else {
                            self.local_log("Pno ClientInfo is null in DefaultState");
                        }
                        done(&mut p);
                        Ok(())
                    }),
                }
            }
            ws::ENABLE_VERBOSE_LOGGING => {
                let a = ws::EnableVerboseLogging::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.held(NETWORK_SETTINGS, uid).map(|held| {
                    if held {
                        self.verbose.store(a.enabled, Ordering::Relaxed);
                        self.local_log(&format!(
                            "enableVerboseLogging: uid={uid} enabled={}",
                            a.enabled
                        ));
                    }
                    ws::write_enable_verbose_logging_reply(&mut p)
                })
            }
            code => {
                let name = ws::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("wifiscanner", &format!("{name} (#1686)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ScanSettings.writeToParcel` of a single scan with two channels, a
    /// hidden network and a vendor IE, as `WifiScanner.startScan` sends
    /// it, and what follows it.
    #[test]
    fn scan_settings_are_read_as_written() {
        let mut p = Parcel::new();
        p.write_i32(WIFI_BAND_UNSPECIFIED);
        for v in [0, 3, 0, 0, 0, 0, 0] {
            p.write_i32(v);
        }
        p.write_i32(SCAN_TYPE_HIGH_ACCURACY);
        p.write_i32(1); // ignoreLocationSettings
        p.write_i32(0); // hideFromAppOps
        p.write_i32(0); // mRnrSetting
        p.write_bool(true);
        p.write_i32(2);
        for (frequency, dwell, passive) in [(2412, 0, 0), (5180, 20, 1)] {
            p.write_i32(frequency);
            p.write_i32(dwell);
            p.write_i32(passive);
        }
        p.write_i32(1);
        p.write_string16(Some("hidden"));
        p.write_i32(1); // one InformationElement
        p.write_i32(1);
        p.write_i32(221);
        p.write_i32(0);
        p.write_i32(5);
        p.write_raw(&[1, 2, 3, 4, 5, 0, 0, 0], &[]);
        p.write_string16(Some("after"));
        let mut r = Reader::new(p.data(), &[]);
        let s = ScanSettings::read_from(&mut r).unwrap();
        assert_eq!(
            (
                s.band,
                s.scan_type,
                s.ignore_location_settings,
                s.hide_from_app_ops
            ),
            (WIFI_BAND_UNSPECIFIED, SCAN_TYPE_HIGH_ACCURACY, true, false)
        );
        assert_eq!((s.channels, s.hidden_networks), (2, 1));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("after"));
    }

    /// `PnoSettings.writeToParcel` with a network, read past.
    #[test]
    fn pno_settings_are_read_past() {
        let mut p = Parcel::new();
        for v in [1, -80, -85, -80, 0, 1] {
            p.write_i32(v);
        }
        p.write_i32(1);
        p.write_string16(Some("\"home\""));
        p.write_i32(2);
        p.write_i32(1);
        p.write_i32(3);
        for f in [2412, 2437, 5180] {
            p.write_i32(f);
        }
        p.write_string16(Some("package"));
        let mut r = Reader::new(p.data(), &[]);
        PnoSettings::read_from(&mut r).unwrap();
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("package"));
    }

    /// `new ScanData()` and `putIntegerArrayList` of no channel, as the
    /// original writes them.
    #[test]
    fn empty_replies_are_written_as_the_original() {
        let mut p = Parcel::new();
        Channels(Vec::new()).write_to(&mut p);
        let mut r = Reader::new(p.data(), &[]);
        let Value::Lazy { kind, start, .. } =
            bundle::read(&mut r).unwrap().unwrap()[GET_AVAILABLE_CHANNELS_EXTRA]
        else {
            panic!("not lazy");
        };
        assert_eq!(kind, bundle::VAL_LIST);
        r.set_position(start);
        assert_eq!(r.read_i32(), Ok(0));
    }
}
