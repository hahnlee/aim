//! `bluetooth_manager`: BluetoothManagerService for the app host (the
//! `com.android.bt` module's `service-bluetooth` at `android-16.0.0_r1`,
//! `BluetoothService`, `BluetoothManagerService`, `BluetoothServiceBinder`,
//! `BtPermissionUtils`), as the system server runs it with its flags as the
//! image sets them (`system_server_remove_extra_thread_jump`,
//! `set_component_available_fix` and `enable_ble_while_disabling_airplane`
//! off): the original Bluetooth app's AdapterService bound (the
//! `android.bluetooth.IBluetooth` service of a system package) and driven
//! through its states (`offToBleOn`, `bleOnToOn`, `onToBleOn`,
//! `bleOnToOff`) on a handler thread of its own, its IBluetooth handed to
//! the apps' adapters (`registerAdapter`) and their callbacks told of it,
//! the state broadcast (`ACTION_BLE_STATE_CHANGED`, `ACTION_STATE_CHANGED`),
//! the persisted `bluetooth_on`, `bluetooth_name` and `bluetooth_address`
//! settings, and the callers' checks. The app runs the original stack over
//! the Bluetooth HAL, a virtual LE controller over the Mac's CoreBluetooth
//! (docs/bluetooth.md).
//!
//! The airplane, satellite and BLE scan settings are read at the start;
//! their later changes, airplane mode on at the start, the user switches,
//! restrictions changes, settings restored, shutdown, Auto On, the HCI snoop
//! log mode and factory reset are not implemented (#1787). The state
//! broadcasts carry no temporary allowlist: every process of the app host
//! is top (#1738).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_bluetooth_ibluetooth as bt;
use aim_service_aidl::android_bluetooth_ibluetoothcallback as bt_callback;
use aim_service_aidl::android_bluetooth_ibluetoothmanager as bm;
use aim_service_aidl::android_bluetooth_ibluetoothmanagercallback as bm_callback;

use super::kept::RawIntent;
use super::permission_checker::{self, AttributionSourceState};
use super::services::HostConnection;
use super::settings_provider::{Caller, SettingsProvider};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;
use crate::package::intent::Intent;
use crate::package::query::Query;

/// `BluetoothAdapter.STATE_*`.
const STATE_OFF: i32 = 10;
const STATE_TURNING_ON: i32 = 11;
const STATE_ON: i32 = 12;
const STATE_TURNING_OFF: i32 = 13;
const STATE_BLE_TURNING_ON: i32 = 14;
const STATE_BLE_ON: i32 = 15;
const STATE_BLE_TURNING_OFF: i32 = 16;
/// `Settings.Global.BLUETOOTH_ON`'s values.
const BLUETOOTH_OFF: i32 = 0;
const BLUETOOTH_ON_BLUETOOTH: i32 = 1;
/// The timings, before `ro.hw_timeout_multiplier`.
const TIMEOUT_BIND_MS: u64 = 4000;
const STATE_TIMEOUT_MS: u64 = 4000;
const SERVICE_RESTART_TIME_MS: u64 = 400;
const ERROR_RESTART_TIME_MS: u64 = 3000;
const ENABLE_DISABLE_DELAY_MS: u64 = 300;
const MAX_ERROR_RESTART_RETRIES: i32 = 6;
const MAX_WAIT_FOR_ENABLE_DISABLE_RETRIES: i32 = 10;
/// `postAndWait`'s limit.
const POST_AND_WAIT: Duration = Duration::from_secs(10);
/// How long `killBluetoothProcess` waits for the app's death.
const DEATH_WAIT: Duration = Duration::from_millis(2000);

const ACTION_IBLUETOOTH: &str = "android.bluetooth.IBluetooth";
const ACTION_BLE_STATE_CHANGED: &str = "android.bluetooth.adapter.action.BLE_STATE_CHANGED";
const ACTION_STATE_CHANGED: &str = "android.bluetooth.adapter.action.STATE_CHANGED";
const EXTRA_PREVIOUS_STATE: &str = "android.bluetooth.adapter.extra.PREVIOUS_STATE";
const EXTRA_STATE: &str = "android.bluetooth.adapter.extra.STATE";
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
/// `Context.BIND_AUTO_CREATE | Context.BIND_IMPORTANT`.
const BIND_FLAGS: i64 = 0x1 | 0x40;
/// `ApplicationInfo.FLAG_SYSTEM`.
const FLAG_SYSTEM: i32 = 1;

const BLUETOOTH_CONNECT: &str = "android.permission.BLUETOOTH_CONNECT";
const BLUETOOTH_PRIVILEGED: &str = "android.permission.BLUETOOTH_PRIVILEGED";
const LOCAL_MAC_ADDRESS: &str = "android.permission.LOCAL_MAC_ADDRESS";
/// `UserManager.DISALLOW_BLUETOOTH`.
const DISALLOW_BLUETOOTH: &str = "no_bluetooth";
/// `ChangeIds.RESTRICT_ENABLE_DISABLE`.
const RESTRICT_ENABLE_DISABLE: i64 = 218_493_289;
/// `BluetoothAdapter.DEFAULT_MAC_ADDRESS`.
const DEFAULT_MAC_ADDRESS: &str = "02:00:00:00:00:00";
const ROOT_APP_ID: i32 = 0;
const SYSTEM_APP_ID: i32 = 1000;
const SHELL_APP_ID: i32 = 2000;
const NFC_APP_ID: i32 = 1027;
/// `PackageManager.SIGNATURE_MATCH`.
const SIGNATURE_MATCH: i32 = 0;
const FEATURE_BLUETOOTH_LE: &str = "android.hardware.bluetooth_le";
const FEATURE_AUTOMOTIVE: &str = "android.hardware.type.automotive";
const FEATURE_WATCH: &str = "android.hardware.type.watch";
const FEATURE_TELEVISION: &str = "android.hardware.type.television";
const FEATURE_LEANBACK: &str = "android.software.leanback";

/// `BluetoothAdapter.nameForState`.
fn name_for_state(state: i32) -> &'static str {
    match state {
        STATE_OFF => "OFF",
        STATE_TURNING_ON => "TURNING_ON",
        STATE_ON => "ON",
        STATE_TURNING_OFF => "TURNING_OFF",
        STATE_BLE_TURNING_ON => "BLE_TURNING_ON",
        STATE_BLE_ON => "BLE_ON",
        STATE_BLE_TURNING_OFF => "BLE_TURNING_OFF",
        _ => "?!?!? (unknown)",
    }
}

/// `isBleState`.
fn is_ble_state(state: i32) -> bool {
    matches!(
        state,
        STATE_BLE_ON | STATE_BLE_TURNING_ON | STATE_BLE_TURNING_OFF
    )
}

/// `BluetoothAdapter.checkBluetoothAddress`: six upper case hex octets.
fn valid_address(address: &str) -> bool {
    let b = address.as_bytes();
    b.len() == 17
        && b.iter().enumerate().all(|(i, c)| {
            if i % 3 == 2 {
                *c == b':'
            } else {
                c.is_ascii_digit() || (b'A'..=b'F').contains(c)
            }
        })
}

/// `Settings.Global.getInt(name, default)`: the default when unset or not
/// a number.
fn int_or(value: Option<String>, default: i32) -> i32 {
    value.and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// `RadioModeListener`'s mode: on while the radios the mode turns off
/// (`radios`) name Bluetooth and the mode is on.
fn radio_mode(radios: Option<&str>, mode: Option<String>) -> bool {
    radios.is_some_and(|r| r.contains("bluetooth")) && int_or(mode, 0) == 1
}

/// The BLE state changes as their BR/EDR users see them
/// (`bluetoothStateChangeHandler`): the BLE states are off.
fn br_edr_state(state: i32) -> i32 {
    if is_ble_state(state) {
        STATE_OFF
    } else {
        state
    }
}

/// The context's attribution source of the system server, as an
/// `AttributionSource` parcel's state.
struct SystemSource;

impl WriteParcelable for SystemSource {
    fn write_to(&self, p: &mut Parcel) {
        super::uri_grants::write_attribution_source(p, SYSTEM_APP_ID, Some("android"));
    }
}

/// The handler's messages (`BluetoothHandler`), with what they carry.
enum Message {
    Enable {
        quiet: bool,
        ble: bool,
    },
    Disable,
    HandleEnableDelayed,
    HandleDisableDelayed {
        disabling: bool,
    },
    ServiceConnected(u32),
    ServiceDisconnected,
    RestartService,
    StateChange {
        prev: i32,
        new: i32,
    },
    TimeoutBind,
    UserUnlocked,
    /// `mHandler.post`.
    Run(Box<dyn FnOnce(&BluetoothManager, &mut Core) + Send>),
}

/// A message's `what`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum What {
    Enable,
    Disable,
    HandleEnableDelayed,
    HandleDisableDelayed,
    ServiceConnected,
    ServiceDisconnected,
    RestartService,
    StateChange,
    TimeoutBind,
    UserUnlocked,
    Run,
}

impl Message {
    fn what(&self) -> What {
        match self {
            Message::Enable { .. } => What::Enable,
            Message::Disable => What::Disable,
            Message::HandleEnableDelayed => What::HandleEnableDelayed,
            Message::HandleDisableDelayed { .. } => What::HandleDisableDelayed,
            Message::ServiceConnected(_) => What::ServiceConnected,
            Message::ServiceDisconnected => What::ServiceDisconnected,
            Message::RestartService => What::RestartService,
            Message::StateChange { .. } => What::StateChange,
            Message::TimeoutBind => What::TimeoutBind,
            Message::UserUnlocked => What::UserUnlocked,
            Message::Run(_) => What::Run,
        }
    }
}

/// A looper's queue: its messages by when they are due, in order sent.
#[derive(Default)]
struct Queue {
    messages: Vec<(Instant, u64, Message)>,
    next: u64,
}

impl Queue {
    fn push(&mut self, at: Instant, m: Message) {
        self.next += 1;
        self.messages.push((at, self.next, m));
    }

    /// The next message due at `now`, else when the first comes.
    fn take(&mut self, now: Instant) -> Result<Message, Option<Instant>> {
        let first = self
            .messages
            .iter()
            .enumerate()
            .min_by_key(|(_, (at, seq, _))| (*at, *seq))
            .map(|(i, (at, _, _))| (i, *at));
        match first {
            Some((i, at)) if at <= now => Ok(self.messages.remove(i).2),
            Some((_, at)) => Err(Some(at)),
            None => Err(None),
        }
    }

    fn has(&self, what: What) -> bool {
        self.messages.iter().any(|(_, _, m)| m.what() == what)
    }

    fn remove(&mut self, what: What) {
        self.messages.retain(|(_, _, m)| m.what() != what);
    }
}

/// `BluetoothAdapterState`: the state, and its waiters.
#[derive(Default)]
struct AdapterState {
    state: Mutex<i32>,
    changed: Condvar,
}

impl AdapterState {
    fn get(&self) -> i32 {
        *self.state.lock().unwrap()
    }

    fn set(&self, state: i32) {
        *self.state.lock().unwrap() = state;
        self.changed.notify_all();
    }

    fn one_of(&self, states: &[i32]) -> bool {
        states.contains(&self.get())
    }

    /// `waitForState`: whether one of `states` came within `timeout`.
    fn wait_for(&self, timeout: Duration, states: &[i32]) -> bool {
        let guard = self.state.lock().unwrap();
        !self
            .changed
            .wait_timeout_while(guard, timeout, |s| !states.contains(s))
            .unwrap()
            .1
            .timed_out()
    }
}

/// The radio modes as their listeners keep them.
#[derive(Default)]
struct Modes {
    /// `AirplaneModeListener.isOn`, `isOnOverrode`.
    airplane: bool,
    airplane_overrode: bool,
    /// `SatelliteModeListener.isOn`.
    satellite: bool,
    /// `BleScanSettingListener.isScanAllowed`.
    scan_allowed: bool,
}

/// What the handler thread keeps.
#[derive(Default)]
struct Core {
    quiet_enable: bool,
    enable_external: bool,
    quiet_enable_external: bool,
    error_recovery_retry_counter: i32,
    wait_for_enable_retry: i32,
    wait_for_disable_retry: i32,
    crashes: i32,
    initialized: bool,
}

pub struct BluetoothManager {
    me: Weak<BluetoothManager>,
    host: Arc<AppHost>,
    settings: Arc<SettingsProvider>,
    queue: Mutex<Queue>,
    wake: Condvar,
    core: Mutex<Core>,
    state: AdapterState,
    /// `mEnable`, read from binder threads too.
    enable: AtomicBool,
    /// `mAdapter`.
    adapter: Mutex<Option<Strong>>,
    /// `mName`, `mAddress`.
    name: Mutex<Option<String>>,
    address: Mutex<Option<String>>,
    modes: Mutex<Modes>,
    /// `mCallbacks`: each adapter's callback and its death link.
    callbacks: Mutex<HashMap<u32, (Strong, u64)>>,
    /// `mBleApps`: each token, its death link and package.
    ble_apps: Mutex<HashMap<u32, (Strong, u64, String)>>,
    /// `mBluetoothCallback`, and the connection to AdapterService.
    adapter_callback: Mutex<Option<Binder>>,
    connection: Arc<dyn HostConnection>,
    multiplier: u64,
    /// `mDeviceConfigAllowAutoOn`.
    auto_on: bool,
}

/// `BluetoothServiceConnection`.
struct Connection(Weak<BluetoothManager>);

impl HostConnection for Connection {
    fn connected(&self, service: Option<Binder>, _dead: bool) {
        let Some(bm) = self.0.upgrade() else { return };
        match service {
            Some(Binder::Handle(h)) => bm.send(Message::ServiceConnected(h)),
            // A binding gone (its process died, or it was let go) is
            // `onServiceDisconnected`.
            _ => bm.send(Message::ServiceDisconnected),
        }
    }
}

/// `mBluetoothCallback`: the adapter's `IBluetoothCallback`.
struct AdapterCallback(Weak<BluetoothManager>);

impl Service for AdapterCallback {
    fn descriptor(&self) -> &str {
        bt_callback::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(bm) = self.0.upgrade() else {
            return reply(Err(Exception::new(
                EX_ILLEGAL_STATE,
                "bluetooth manager gone",
            )));
        };
        let mut p = Parcel::new();
        let result = match call.code {
            bt_callback::ON_BLUETOOTH_STATE_CHANGE => {
                let a = bt_callback::OnBluetoothStateChange::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                bm.send(Message::StateChange {
                    prev: a.prev_state,
                    new: a.new_state,
                });
                bt_callback::write_on_bluetooth_state_change_reply(&mut p);
                Ok(())
            }
            bt_callback::ON_ADAPTER_NAME_CHANGE => {
                let a = bt_callback::OnAdapterNameChange::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match a.name {
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                    Some(name) if name.is_empty() => {
                        Err(Exception::illegal_argument("Invalid Empty name"))
                    }
                    Some(name) => {
                        bm.post(move |bm, _| bm.store_name(name));
                        bt_callback::write_on_adapter_name_change_reply(&mut p);
                        Ok(())
                    }
                }
            }
            bt_callback::ON_ADAPTER_ADDRESS_CHANGE => {
                let a = bt_callback::OnAdapterAddressChange::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match a.address {
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                    Some(address) if !valid_address(&address) => {
                        Err(Exception::illegal_argument("Invalid address"))
                    }
                    Some(address) => {
                        bm.post(move |bm, _| bm.store_address(address));
                        bt_callback::write_on_adapter_address_change_reply(&mut p);
                        Ok(())
                    }
                }
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

impl BluetoothManager {
    /// `BluetoothService`'s `BluetoothManagerService` and its handler
    /// thread: the persisted name, address and state read.
    pub fn new(host: Arc<AppHost>, settings: Arc<SettingsProvider>) -> Arc<Self> {
        let multiplier = host
            .property("ro.hw_timeout_multiplier")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let bm = Arc::new_cyclic(|me: &Weak<BluetoothManager>| BluetoothManager {
            me: me.clone(),
            host: host.clone(),
            settings,
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            core: Mutex::new(Core::default()),
            state: AdapterState {
                state: Mutex::new(STATE_OFF),
                changed: Condvar::new(),
            },
            enable: AtomicBool::new(false),
            adapter: Mutex::new(None),
            name: Mutex::new(None),
            address: Mutex::new(None),
            modes: Mutex::new(Modes::default()),
            callbacks: Mutex::new(HashMap::new()),
            ble_apps: Mutex::new(HashMap::new()),
            adapter_callback: Mutex::new(None),
            connection: Arc::new(Connection(me.clone())),
            multiplier,
            auto_on: super::system_boolean(
                host.property("bluetooth.server.automatic_turn_on")
                    .as_deref(),
                false,
            ),
        });
        *bm.adapter_callback.lock().unwrap() = Some(
            host.process
                .add_service(Arc::new(AdapterCallback(bm.me.clone()))),
        );
        let weak = bm.me.clone();
        let _ = std::thread::Builder::new()
            .name("BluetoothManagerService".into())
            .spawn(move || {
                loop {
                    let Some(bm) = weak.upgrade() else { return };
                    let next = {
                        let mut queue = bm.queue.lock().unwrap();
                        loop {
                            match queue.take(Instant::now()) {
                                Ok(m) => break m,
                                Err(Some(at)) => {
                                    queue = bm
                                        .wake
                                        .wait_timeout(
                                            queue,
                                            at.saturating_duration_since(Instant::now()),
                                        )
                                        .unwrap()
                                        .0
                                }
                                Err(None) => queue = bm.wake.wait(queue).unwrap(),
                            }
                        }
                    };
                    let mut core = bm.core.lock().unwrap();
                    bm.handle(&mut core, next);
                }
            });
        bm
    }

    fn ms(&self, ms: u64) -> Duration {
        Duration::from_millis(ms * self.multiplier)
    }

    fn send(&self, m: Message) {
        self.send_delayed(m, Duration::ZERO);
    }

    fn send_delayed(&self, m: Message, delay: Duration) {
        self.queue.lock().unwrap().push(Instant::now() + delay, m);
        self.wake.notify_all();
    }

    fn post(&self, f: impl FnOnce(&BluetoothManager, &mut Core) + Send + 'static) {
        self.send(Message::Run(Box::new(f)));
    }

    fn has(&self, what: What) -> bool {
        self.queue.lock().unwrap().has(what)
    }

    fn remove(&self, what: What) {
        self.queue.lock().unwrap().remove(what);
    }

    /// `postAndWait`: `f` run on the handler thread, waited for.
    fn post_and_wait<T: Send + 'static>(
        &self,
        f: impl FnOnce(&BluetoothManager, &mut Core) -> T + Send + 'static,
    ) -> Result<T, Exception> {
        let (tx, rx) = mpsc::channel();
        self.post(move |bm, core| {
            let _ = tx.send(f(bm, core));
        });
        rx.recv_timeout(POST_AND_WAIT).map_err(|_| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "bluetooth manager: the handler did not answer in 10 s",
            )
        })
    }

    // Settings, as the system server's content resolver reads and writes
    // them.

    fn setting(&self, table: &str, name: &str) -> Result<Option<String>, Exception> {
        let caller = Caller {
            uid: SYSTEM_APP_ID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([("_user".to_string(), Value::Int(0))]);
        let got =
            self.settings
                .call(&caller, &format!("GET_{table}"), Some(name), &extras, None)?;
        Ok(match got.as_deref() {
            Some([(_, Value::String(value)), ..]) => value.clone(),
            _ => None,
        })
    }

    fn put_setting(&self, table: &str, name: &str, value: &str) -> Result<(), Exception> {
        let caller = Caller {
            uid: SYSTEM_APP_ID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([
            ("_user".to_string(), Value::Int(0)),
            ("value".to_string(), Value::String(Some(value.to_string()))),
        ]);
        self.settings
            .call(&caller, &format!("PUT_{table}"), Some(name), &extras, None)
            .map(|_| ())
    }

    /// `getBluetoothPersistedState(BLUETOOTH_ON_BLUETOOTH)`.
    fn persisted_state(&self) -> i32 {
        match self.setting("global", "bluetooth_on") {
            Ok(value) => int_or(value, BLUETOOTH_ON_BLUETOOTH),
            Err(e) => {
                eprintln!("app host: bluetooth: bluetooth_on: {e:?}");
                BLUETOOTH_ON_BLUETOOTH
            }
        }
    }

    /// `setBluetoothPersistedState`.
    fn set_persisted_state(&self, state: i32) {
        eprintln!("app host: bluetooth: setBluetoothPersistedState({state})");
        if let Err(e) = self.put_setting("global", "bluetooth_on", &state.to_string()) {
            eprintln!("app host: bluetooth: bluetooth_on: {e:?}");
        }
    }

    /// `storeName`.
    fn store_name(&self, name: String) {
        match self.put_setting("secure", "bluetooth_name", &name) {
            Ok(()) => *self.name.lock().unwrap() = Some(name),
            Err(e) => eprintln!("app host: bluetooth: storeName: Failed: {e:?}"),
        }
    }

    /// `storeAddress`.
    fn store_address(&self, address: String) {
        match self.put_setting("secure", "bluetooth_address", &address) {
            Ok(()) => *self.address.lock().unwrap() = Some(address),
            Err(e) => eprintln!("app host: bluetooth: storeAddress: Failed: {e:?}"),
        }
    }

    // The service's start (`BluetoothService`).

    /// The constructor's reads: the local adapter's name and address, the
    /// BLE scan setting, whether Bluetooth was on, and ASHA.
    fn load(&self, core: &mut Core) -> Result<(), Exception> {
        *self.name.lock().unwrap() = self.setting("secure", "bluetooth_name")?;
        *self.address.lock().unwrap() = self.setting("secure", "bluetooth_address")?;
        self.modes.lock().unwrap().scan_allowed =
            int_or(self.setting("global", "ble_scan_always_enabled")?, 0) != 0;
        if self.persisted_state() != BLUETOOTH_OFF {
            core.enable_external = true;
        }
        Ok(())
    }

    /// `onUserStarting` of the system user: `handleOnBootPhase`, once.
    pub fn on_user_starting(&self) {
        self.post(|bm, core| {
            if core.initialized {
                return;
            }
            core.initialized = true;
            if let Err(e) = bm
                .load(core)
                .and_then(|()| bm.internal_handle_on_boot_phase(core))
            {
                eprintln!("app host: bluetooth: start: {e:?}");
            }
        });
    }

    /// `onUserUnlocking` of the system user.
    pub fn on_user_unlocking(&self) {
        self.send(Message::UserUnlocked);
    }

    /// `initialize`: the airplane and satellite modes as their listeners
    /// read them at the start.
    fn initialize(&self) -> Result<(), Exception> {
        // `AirplaneModeListener.initialize`: Wi-Fi learns that Bluetooth
        // has the airplane enhancement mode.
        let enhancement = int_or(self.setting("global", "apm_enhancement_enabled")?, 1);
        self.put_setting(
            "global",
            "apm_enhancement_enabled",
            &enhancement.to_string(),
        )?;
        let airplane = radio_mode(
            self.setting("global", "airplane_mode_radios")?.as_deref(),
            self.setting("global", "airplane_mode_on")?,
        );
        if airplane {
            return Err(self
                .host
                .unimplemented_named("bluetooth_manager", "airplane mode at the start (#1787)"));
        }
        let satellite = radio_mode(
            self.setting("global", "satellite_mode_radios")?.as_deref(),
            self.setting("global", "satellite_mode_enabled")?,
        );
        let mut modes = self.modes.lock().unwrap();
        (modes.airplane, modes.airplane_overrode, modes.satellite) = (false, false, satellite);
        Ok(())
    }

    /// `internalHandleOnBootPhase`.
    fn internal_handle_on_boot_phase(&self, core: &mut Core) -> Result<(), Exception> {
        self.initialize()?;
        if self.bluetooth_disallowed()? {
            return Ok(());
        }
        // The app host never boots in safe mode.
        if core.enable_external && self.persisted_state() == BLUETOOTH_ON_BLUETOOTH {
            eprintln!("app host: bluetooth: internalHandleOnBootPhase: Auto-enabling Bluetooth.");
            self.send_enable_msg(core.quiet_enable_external, false);
        } else if self.auto_on {
            return Err(self
                .host
                .unimplemented_named("bluetooth_manager", "Auto On (#1787)"));
        }
        Ok(())
    }

    /// `isBluetoothDisallowed`.
    fn bluetooth_disallowed(&self) -> Result<bool, Exception> {
        Ok(self.host.user_restrictions()?.contains(DISALLOW_BLUETOOTH))
    }

    // The handler (`BluetoothHandler.handleMessage`).

    fn handle(&self, core: &mut Core, m: Message) {
        match m {
            Message::Run(f) => f(self, core),
            Message::Enable { quiet, ble } => self.handle_enable_message(core, quiet, ble),
            Message::Disable => self.handle_disable_message(core),
            Message::HandleEnableDelayed => self.handle_enable_delayed(core),
            Message::HandleDisableDelayed { disabling } => {
                self.handle_disable_delayed(core, disabling)
            }
            Message::ServiceConnected(handle) => {
                self.remove(What::TimeoutBind);
                let adapter = self.host.process.strong(handle);
                *self.adapter.lock().unwrap() = Some(adapter);
                self.propagate_foreground_user_id(0);
                let callback = self.adapter_callback.lock().unwrap().clone();
                self.adapter_call("registerCallback", bt::REGISTER_CALLBACK, |p| {
                    bt::RegisterCallback {
                        callback,
                        attribution_source: Some(SystemSource),
                    }
                    .write(p)
                });
                self.off_to_ble_on(core);
                self.send_bluetooth_service_up_callback();
                if !self.enable.load(Ordering::Relaxed) {
                    self.wait_for_state(&[STATE_ON]);
                    self.on_to_ble_on(core);
                }
            }
            Message::StateChange { prev, new } => {
                eprintln!(
                    "app host: bluetooth: MESSAGE_BLUETOOTH_STATE_CHANGE: prevState={} newState={}",
                    name_for_state(prev),
                    name_for_state(new)
                );
                if self.adapter.lock().unwrap().is_none() {
                    eprintln!(
                        "app host: bluetooth: State change received after bluetooth has crashed"
                    );
                    return;
                }
                self.bluetooth_state_change_handler(core, prev, new);
                let enable = self.enable.load(Ordering::Relaxed);
                // An error from TURNING_ON to OFF: unbind and rebind.
                if prev == STATE_BLE_TURNING_ON && new == STATE_OFF && enable {
                    self.recover_from_error(core, false);
                }
                if prev == STATE_TURNING_ON && new == STATE_BLE_ON && enable {
                    self.recover_from_error(core, true);
                }
                // Enabled while shutting down: wait for the process to go,
                // then restart it.
                if prev == STATE_BLE_TURNING_OFF
                    && new == STATE_OFF
                    && self.enable.load(Ordering::Relaxed)
                {
                    eprintln!(
                        "app host: bluetooth: Entering STATE_OFF but mEnabled is true; restarting."
                    );
                    self.wait_for_state(&[STATE_OFF]);
                    self.send_delayed(Message::RestartService, self.service_restart(core));
                }
                if (new == STATE_ON || new == STATE_BLE_ON)
                    && core.error_recovery_retry_counter != 0
                {
                    eprintln!("app host: bluetooth: bluetooth is recovered from error");
                    core.error_recovery_retry_counter = 0;
                }
            }
            Message::ServiceDisconnected => {
                eprintln!("app host: bluetooth: MESSAGE_BLUETOOTH_SERVICE_DISCONNECTED");
                if !self.reset_adapter() {
                    return;
                }
                // `addCrashLog`.
                core.crashes += 1;
                if self.enable.swap(false, Ordering::Relaxed) {
                    self.send_delayed(Message::RestartService, self.service_restart(core));
                }
                self.send_bluetooth_service_down_callback();
                if self.state.one_of(&[STATE_TURNING_ON, STATE_ON]) {
                    self.bluetooth_state_change_handler(core, STATE_ON, STATE_TURNING_OFF);
                }
                if self.state.one_of(&[STATE_TURNING_OFF]) {
                    self.bluetooth_state_change_handler(core, STATE_TURNING_OFF, STATE_OFF);
                }
                self.remove(What::StateChange);
                self.state.set(STATE_OFF);
            }
            Message::RestartService => {
                core.error_recovery_retry_counter += 1;
                eprintln!(
                    "app host: bluetooth: MESSAGE_RESTART_BLUETOOTH_SERVICE: retry count={}",
                    core.error_recovery_retry_counter
                );
                if core.error_recovery_retry_counter < MAX_ERROR_RESTART_RETRIES {
                    // Enabled without persisting: the setting stays as it
                    // was across the service's restart.
                    self.enable.store(true, Ordering::Relaxed);
                    self.handle_enable();
                } else {
                    self.reset_adapter();
                    eprintln!("app host: bluetooth: Reach maximum retry to restart Bluetooth!");
                }
            }
            Message::TimeoutBind => eprintln!("app host: bluetooth: MESSAGE_TIMEOUT_BIND"),
            Message::UserUnlocked => {
                if self.enable.load(Ordering::Relaxed)
                    && !self.is_binding()
                    && self.adapter.lock().unwrap().is_none()
                {
                    eprintln!("app host: bluetooth: Enabled but not bound; retrying after unlock");
                    self.handle_enable();
                }
            }
        }
    }

    fn service_restart(&self, core: &Core) -> Duration {
        self.ms((core.error_recovery_retry_counter as u64 + 1) * SERVICE_RESTART_TIME_MS)
    }

    fn wait_for_state(&self, states: &[i32]) -> bool {
        self.state.wait_for(self.ms(STATE_TIMEOUT_MS), states)
    }

    fn is_binding(&self) -> bool {
        self.has(What::TimeoutBind)
    }

    /// A one-way call to the adapter; its failure logged, as the original
    /// logs a `RemoteException`.
    fn adapter_call(&self, name: &str, code: u32, write: impl FnOnce(&mut Parcel)) {
        let Some(adapter) = self.adapter() else {
            eprintln!("app host: bluetooth: {name}: mAdapter is null");
            return;
        };
        let mut p = Parcel::new();
        write(&mut p);
        if let Err(s) = adapter.transact(code, &p, true) {
            eprintln!("app host: bluetooth: Unable to call {name}(): {s}");
        }
    }

    /// `mAdapter`'s binder, held for a call.
    fn adapter(&self) -> Option<Strong> {
        let handle = self.adapter.lock().unwrap().as_ref().map(|a| a.handle)?;
        Some(self.host.process.strong(handle))
    }

    /// `propagateForegroundUserId`.
    fn propagate_foreground_user_id(&self, user: i32) {
        self.adapter_call("setForegroundUserId", bt::SET_FOREGROUND_USER_ID, |p| {
            bt::SetForegroundUserId {
                user_id: user,
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
    }

    fn handle_enable_message(&self, core: &mut Core, quiet: bool, ble: bool) {
        if self.has(What::HandleDisableDelayed) || self.has(What::HandleEnableDelayed) {
            // Enabling or disabling now: wait for it.
            self.send_delayed(
                Message::Enable { quiet, ble },
                self.ms(ENABLE_DISABLE_DELAY_MS),
            );
            return;
        }
        self.remove(What::RestartService);
        self.enable.store(true, Ordering::Relaxed);
        if !ble {
            self.set_persisted_state(BLUETOOTH_ON_BLUETOOTH);
        }
        if self
            .state
            .one_of(&[STATE_BLE_TURNING_ON, STATE_TURNING_ON, STATE_ON])
        {
            eprintln!(
                "app host: bluetooth: Already enabled. Current state={}",
                name_for_state(self.state.get())
            );
            return;
        }
        if self.state.one_of(&[STATE_BLE_ON]) && ble {
            return;
        }
        if self.state.one_of(&[STATE_BLE_ON]) {
            self.ble_on_to_on(core);
            return;
        }
        if self.adapter.lock().unwrap().is_some() {
            // Wait for STATE_OFF and the previous process's exit.
            core.wait_for_enable_retry = 0;
            self.send_delayed(
                Message::HandleEnableDelayed,
                self.ms(ENABLE_DISABLE_DELAY_MS),
            );
            return;
        }
        core.quiet_enable = quiet;
        self.handle_enable();
    }

    fn handle_disable_message(&self, core: &mut Core) {
        if self.has(What::HandleDisableDelayed)
            || self.is_binding()
            || self.has(What::HandleEnableDelayed)
        {
            self.send_delayed(Message::Disable, self.ms(ENABLE_DISABLE_DELAY_MS));
            return;
        }
        self.remove(What::RestartService);
        if self.enable.load(Ordering::Relaxed) && self.adapter.lock().unwrap().is_some() {
            core.wait_for_disable_retry = 0;
            self.send_delayed(
                Message::HandleDisableDelayed { disabling: false },
                self.ms(ENABLE_DISABLE_DELAY_MS),
            );
        } else {
            self.enable.store(false, Ordering::Relaxed);
            self.on_to_ble_on(core);
        }
    }

    /// `resolveSystemService`: the one system package's service for
    /// `action`.
    fn resolve_system_service(&self) -> Result<(String, String), Exception> {
        let (_, resolution) = self.host.live_packages()?;
        let intent = Intent {
            action: Some(ACTION_IBLUETOOTH.into()),
            ..Default::default()
        };
        let found = resolution
            .query_intent_services(&intent, None, 0, 0, SYSTEM_APP_ID)
            .map_err(|e| {
                self.host.unimplemented_named(
                    "bluetooth_manager",
                    &format!("queryIntentServices: {e:?}"),
                )
            })?;
        let mut results = Vec::new();
        for r in found {
            let crate::package::component_resolver::Info::Service(si) = r.info else {
                continue;
            };
            if si.info.application_info.flags & FLAG_SYSTEM != 0 {
                results.push((
                    si.info.item.package_name.clone().unwrap_or_default(),
                    si.info.item.name.clone().unwrap_or_default(),
                ));
            }
        }
        match results.len() {
            0 => Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("No services can handle intent {ACTION_IBLUETOOTH}"),
            )),
            1 => Ok(results.remove(0)),
            _ => Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("Multiples services can handle intent {ACTION_IBLUETOOTH}: {results:?}"),
            )),
        }
    }

    /// `bindToAdapter`.
    fn bind_to_adapter(&self) {
        let (package, class) = match self.resolve_system_service() {
            Ok(found) => found,
            Err(e) => return eprintln!("app host: bluetooth: bindToAdapter: {e:?}"),
        };
        self.send_delayed(Message::TimeoutBind, self.ms(TIMEOUT_BIND_MS));
        eprintln!("app host: bluetooth: Start binding to the Bluetooth service {package}/{class}");
        let bound = self.host.bind_service_from_host(
            Some(ACTION_IBLUETOOTH),
            &package,
            &class,
            0,
            BIND_FLAGS,
            0,
            self.connection.clone(),
        );
        if !matches!(bound, Ok(true)) {
            eprintln!("app host: bluetooth: Fail to bind to {package}/{class}: {bound:?}");
            self.host.unbind_service_from_host(&self.connection);
            self.remove(What::TimeoutBind);
        }
    }

    /// `resetAdapter`.
    fn reset_adapter(&self) -> bool {
        if self.adapter.lock().unwrap().take().is_none() {
            return false;
        }
        self.host.unbind_service_from_host(&self.connection);
        true
    }

    /// `handleEnable`.
    fn handle_enable(&self) {
        if self.adapter.lock().unwrap().is_none() && !self.is_binding() {
            self.bind_to_adapter();
        }
    }

    fn handle_enable_delayed(&self, core: &mut Core) {
        // Turning off: wait for STATE_OFF.
        if !self.state.one_of(&[STATE_OFF]) {
            if core.wait_for_enable_retry < MAX_WAIT_FOR_ENABLE_DISABLE_RETRIES {
                core.wait_for_enable_retry += 1;
                self.send_delayed(
                    Message::HandleEnableDelayed,
                    self.ms(ENABLE_DISABLE_DELAY_MS),
                );
                return;
            }
            eprintln!("app host: bluetooth: Wait for STATE_OFF timeout");
        }
        core.wait_for_enable_retry = 0;
        self.send_delayed(Message::RestartService, self.service_restart(core));
    }

    fn handle_disable_delayed(&self, core: &mut Core, disabling: bool) {
        if !disabling {
            // Turning on: wait for STATE_ON.
            if !self.state.one_of(&[STATE_ON]) {
                if core.wait_for_disable_retry < MAX_WAIT_FOR_ENABLE_DISABLE_RETRIES {
                    core.wait_for_disable_retry += 1;
                    self.send_delayed(
                        Message::HandleDisableDelayed { disabling: false },
                        self.ms(ENABLE_DISABLE_DELAY_MS),
                    );
                    return;
                }
                eprintln!("app host: bluetooth: Wait for STATE_ON timeout");
            }
            core.wait_for_disable_retry = 0;
            self.enable.store(false, Ordering::Relaxed);
            self.on_to_ble_on(core);
            self.send_delayed(
                Message::HandleDisableDelayed { disabling: true },
                self.ms(ENABLE_DISABLE_DELAY_MS),
            );
        } else if self.state.one_of(&[STATE_ON]) {
            // Turning off: wait to leave STATE_ON.
            if core.wait_for_disable_retry < MAX_WAIT_FOR_ENABLE_DISABLE_RETRIES {
                core.wait_for_disable_retry += 1;
                self.send_delayed(
                    Message::HandleDisableDelayed { disabling: true },
                    self.ms(ENABLE_DISABLE_DELAY_MS),
                );
                return;
            }
            eprintln!("app host: bluetooth: Wait for exiting STATE_ON timeout");
        }
    }

    fn off_to_ble_on(&self, core: &mut Core) {
        if !self.state.one_of(&[STATE_OFF]) {
            return eprintln!(
                "app host: bluetooth: offToBleOn: Impossible transition from {}",
                name_for_state(self.state.get())
            );
        }
        let quiet = core.quiet_enable;
        self.adapter_call("offToBleOn", bt::OFF_TO_BLE_ON, |p| {
            bt::OffToBleOn {
                quiet_mode: quiet,
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
        self.bluetooth_state_change_handler(core, STATE_OFF, STATE_BLE_TURNING_ON);
    }

    fn on_to_ble_on(&self, core: &mut Core) {
        if !self.state.one_of(&[STATE_ON]) {
            return eprintln!(
                "app host: bluetooth: onToBleOn: Impossible transition from {}",
                name_for_state(self.state.get())
            );
        }
        self.adapter_call("onToBleOn", bt::ON_TO_BLE_ON, |p| {
            bt::OnToBleOn {
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
        self.bluetooth_state_change_handler(core, STATE_ON, STATE_TURNING_OFF);
    }

    fn ble_on_to_on(&self, core: &mut Core) {
        if !self.state.one_of(&[STATE_BLE_ON]) {
            return eprintln!(
                "app host: bluetooth: bleOnToOn: Impossible transition from {}",
                name_for_state(self.state.get())
            );
        }
        self.adapter_call("bleOnToOn", bt::BLE_ON_TO_ON, |p| {
            bt::BleOnToOn {
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
        self.bluetooth_state_change_handler(core, STATE_BLE_ON, STATE_TURNING_ON);
    }

    fn ble_on_to_off(&self, core: &mut Core) {
        if !self.state.one_of(&[STATE_BLE_ON]) {
            return eprintln!(
                "app host: bluetooth: bleOnToOff: Impossible transition from {}",
                name_for_state(self.state.get())
            );
        }
        self.adapter_call("bleOnToOff", bt::BLE_ON_TO_OFF, |p| {
            bt::BleOnToOff {
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
        self.bluetooth_state_change_handler(core, STATE_BLE_ON, STATE_BLE_TURNING_OFF);
    }

    /// `broadcastIntentStateChange`: to every user.
    fn broadcast_state_change(&self, action: &str, prev: i32, new: i32) {
        let intent = RawIntent::new(
            action,
            FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT,
            &[
                (EXTRA_PREVIOUS_STATE, Value::Int(prev)),
                (EXTRA_STATE, Value::Int(new)),
            ],
        );
        self.host.broadcasts.send_from_system(&self.host, intent);
    }

    /// `bluetoothStateChangeHandler`.
    fn bluetooth_state_change_handler(&self, core: &mut Core, prev: i32, new: i32) {
        if self.state.one_of(&[new]) {
            return;
        }
        if new == STATE_OFF {
            // Off: the adapters' service goes, and so does the binding.
            self.send_bluetooth_service_down_callback();
            self.unbind_and_finish();
        }
        self.state.set(new);
        self.broadcast_state_change(ACTION_BLE_STATE_CHANGED, prev, new);
        let (prev_br_edr, new_br_edr) = (br_edr_state(prev), br_edr_state(new));
        if prev_br_edr != new_br_edr {
            if new_br_edr == STATE_OFF {
                self.broadcast_to_adapters(bm_callback::ON_BLUETOOTH_OFF, None);
                self.send_br_edr_down_callback(core);
            }
            self.broadcast_state_change(ACTION_STATE_CHANGED, prev_br_edr, new_br_edr);
        }
        if new == STATE_ON {
            self.broadcast_to_adapters(bm_callback::ON_BLUETOOTH_ON, None);
        } else if new == STATE_BLE_ON && prev == STATE_BLE_TURNING_ON {
            self.continue_from_ble_on_state(core);
        }
    }

    /// `continueFromBleOnState`.
    fn continue_from_ble_on_state(&self, core: &mut Core) {
        if !self.state.one_of(&[STATE_BLE_ON]) {
            return eprintln!(
                "app host: bluetooth: continueFromBleOnState: Impossible transition from {}",
                name_for_state(self.state.get())
            );
        }
        if !core.enable_external && !self.ble_app_present() {
            self.enable.store(false, Ordering::Relaxed);
            self.ble_on_to_off(core);
            return;
        }
        if self.persisted_state() == BLUETOOTH_ON_BLUETOOTH || !self.ble_app_present() {
            self.ble_on_to_on(core);
            self.set_persisted_state(BLUETOOTH_ON_BLUETOOTH);
        }
    }

    /// `sendBrEdrDownCallback`.
    fn send_br_edr_down_callback(&self, core: &mut Core) {
        if self.adapter.lock().unwrap().is_none() {
            return;
        }
        let stay = {
            let modes = self.modes.lock().unwrap();
            modes.scan_allowed && !modes.airplane
        };
        if stay && self.ble_app_present() {
            // Stays in BLE_ON: the GATT clients go.
            self.adapter_call("unregAllGattClient", bt::UNREG_ALL_GATT_CLIENT, |p| {
                bt::UnregAllGattClient {
                    attribution_source: Some(SystemSource),
                }
                .write(p)
            });
        } else {
            self.ble_on_to_off(core);
        }
    }

    /// `unbindAndFinish`: the adapter's callback dropped, the binding let
    /// go and the app's process killed and waited for.
    fn unbind_and_finish(&self) {
        self.remove(What::StateChange);
        let Some(adapter) = self.adapter() else {
            return;
        };
        let callback = self.adapter_callback.lock().unwrap().clone();
        self.adapter_call("unregisterCallback", bt::UNREGISTER_CALLBACK, |p| {
            bt::UnregisterCallback {
                callback,
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
        let (died, death) = mpsc::channel();
        let cookie = self.host.process.link_to_death(
            &adapter,
            Box::new(move || {
                let _ = died.send(());
            }),
        );
        // Unbound first, so no `onServiceDisconnected` comes.
        self.host.unbind_service_from_host(&self.connection);
        // `killBluetoothProcess`.
        let mut p = Parcel::new();
        bt::KillBluetoothProcess {}.write(&mut p);
        match adapter.transact(bt::KILL_BLUETOOTH_PROCESS, &p, true) {
            Err(s) => eprintln!("app host: bluetooth: killBluetoothProcess(): {s}"),
            Ok(_) => {
                if death.recv_timeout(DEATH_WAIT).is_err() {
                    eprintln!(
                        "app host: bluetooth: killBluetoothProcess(): Bluetooth death not received after > 2000ms"
                    );
                }
            }
        }
        self.host.process.clear_death(&adapter, cookie);
        // The activity manager learns of the death.
        std::thread::sleep(Duration::from_millis(100));
        *self.adapter.lock().unwrap() = None;
        self.remove(What::TimeoutBind);
    }

    /// `recoverBluetoothServiceFromError`.
    fn recover_from_error(&self, core: &mut Core, clear_ble: bool) {
        eprintln!("app host: bluetooth: recoverBluetoothServiceFromError");
        let callback = self.adapter_callback.lock().unwrap().clone();
        self.adapter_call("unregisterCallback", bt::UNREGISTER_CALLBACK, |p| {
            bt::UnregisterCallback {
                callback,
                attribution_source: Some(SystemSource),
            }
            .write(p)
        });
        std::thread::sleep(Duration::from_millis(500));
        self.on_to_ble_on(core);
        self.wait_for_state(&[STATE_OFF]);
        self.send_bluetooth_service_down_callback();
        self.reset_adapter();
        self.remove(What::StateChange);
        self.state.set(STATE_OFF);
        if clear_ble {
            self.clear_ble_apps();
        }
        self.enable.store(false, Ordering::Relaxed);
        self.send_delayed(Message::RestartService, self.ms(ERROR_RESTART_TIME_MS));
    }

    /// `broadcastToAdapters` of the one-way `code`, with the service's
    /// binder for `onBluetoothServiceUp`.
    fn broadcast_to_adapters(&self, code: u32, service: Option<Binder>) {
        let handles: Vec<u32> = self.callbacks.lock().unwrap().keys().copied().collect();
        for callback in handles.into_iter().map(|h| self.host.process.strong(h)) {
            let mut p = Parcel::new();
            p.write_interface_token(bm_callback::DESCRIPTOR);
            if code == bm_callback::ON_BLUETOOTH_SERVICE_UP {
                p.write_binder(service.clone());
            }
            if let Err(s) = callback.transact(code, &p, true) {
                eprintln!(
                    "app host: bluetooth: RemoteException while calling the adapters' callback #{code}: {s}"
                );
            }
        }
    }

    fn send_bluetooth_service_up_callback(&self) {
        let service = self
            .adapter
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| Binder::Handle(a.handle));
        self.broadcast_to_adapters(bm_callback::ON_BLUETOOTH_SERVICE_UP, service);
    }

    fn send_bluetooth_service_down_callback(&self) {
        self.broadcast_to_adapters(bm_callback::ON_BLUETOOTH_SERVICE_DOWN, None);
    }

    fn send_enable_msg(&self, quiet: bool, ble: bool) {
        self.send(Message::Enable { quiet, ble });
    }

    fn send_disable_msg(&self) {
        self.send(Message::Disable);
    }

    // The BLE apps (`mBleApps`).

    fn ble_app_present(&self) -> bool {
        !self.ble_apps.lock().unwrap().is_empty()
    }

    fn clear_ble_apps(&self) {
        let gone: Vec<_> = self.ble_apps.lock().unwrap().drain().collect();
        for (_, (strong, death, _)) in gone {
            self.host.process.clear_death(&strong, death);
        }
    }

    /// `updateBleAppCount`.
    fn update_ble_app_count(
        &self,
        token: u32,
        enable: bool,
        package: &str,
    ) -> Result<usize, Exception> {
        let mut apps = self.ble_apps.lock().unwrap();
        if enable && !apps.contains_key(&token) {
            let strong = self.host.process.strong(token);
            let weak = self.me.clone();
            let package_name = package.to_string();
            let death = self.host.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(bm) = weak.upgrade() {
                        eprintln!(
                            "app host: bluetooth: Binder is dead - unregister {package_name}"
                        );
                        let _ = bm.update_ble_app_count(token, false, &package_name);
                    }
                }),
            );
            apps.insert(token, (strong, death, package.to_string()));
        } else if !enable && let Some((strong, death, _)) = apps.remove(&token) {
            self.host.process.clear_death(&strong, death);
        }
        Ok(apps.len())
    }

    // The binder's calls, on the handler (`postAndWait`).

    fn enable_ble(&self, package: &str, token: u32) -> Result<bool, Exception> {
        let modes = self.modes.lock().unwrap();
        if (modes.airplane && !self.enable.load(Ordering::Relaxed))
            || modes.satellite
            || !modes.scan_allowed
        {
            return Ok(false);
        }
        drop(modes);
        self.update_ble_app_count(token, true, package)?;
        if self.state.one_of(&[
            STATE_ON,
            STATE_BLE_ON,
            STATE_TURNING_ON,
            STATE_TURNING_OFF,
            STATE_BLE_TURNING_ON,
        ]) {
            return Ok(true);
        }
        self.send_enable_msg(false, true);
        Ok(true)
    }

    fn disable_ble(&self, core: &mut Core, package: &str, token: u32) -> Result<bool, Exception> {
        if self.state.one_of(&[STATE_OFF]) {
            return Ok(false);
        }
        self.update_ble_app_count(token, false, package)?;
        if self.state.one_of(&[STATE_BLE_ON]) && !self.ble_app_present() {
            if self.enable.load(Ordering::Relaxed) {
                // `disableBleScanMode`.
                if self.state.one_of(&[STATE_ON]) {
                    self.enable.store(false, Ordering::Relaxed);
                }
            }
            if !core.enable_external {
                self.send_br_edr_down_callback(core);
            }
        }
        Ok(true)
    }

    fn enable_from_binder(&self, core: &mut Core, quiet: bool) -> bool {
        if self.modes.lock().unwrap().satellite {
            return false;
        }
        core.quiet_enable_external = quiet;
        core.enable_external = true;
        self.send_enable_msg(quiet, false);
        true
    }

    fn disable_from_binder(&self, core: &mut Core, persist: bool) -> bool {
        if persist {
            self.set_persisted_state(BLUETOOTH_OFF);
        }
        core.enable_external = false;
        self.send_disable_msg();
        true
    }

    // The binder's checks (`BtPermissionUtils`).

    /// `checkConnectPermissionForDataDelivery`.
    fn connect_permission(&self, source: &AttributionSourceState) -> Result<bool, Exception> {
        match permission_checker::data_delivery_from_datasource(
            &self.host,
            BLUETOOTH_CONNECT,
            source,
        )? {
            permission_checker::GRANTED => Ok(true),
            permission_checker::HARD_DENIED => Err(Exception::security(format!(
                "Need {BLUETOOTH_CONNECT} permission"
            ))),
            _ => Ok(false),
        }
    }

    /// `checkIfCallerIsForegroundUser`: of the app host's one user, or NFC,
    /// SystemUI or the shell.
    fn caller_is_foreground_user(&self, uid: i32) -> Result<bool, Exception> {
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        Ok(uid / PER_USER_RANGE as i32 == 0
            || app_id == NFC_APP_ID
            || app_id == SHELL_APP_ID
            || self.system_ui_app_id()? == Some(app_id))
    }

    /// SystemUI's uid, when installed as a system package.
    fn system_ui_app_id(&self) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.live_packages()?;
        let system = |ps: &&crate::package::model::PackageState| {
            ps.pkg
                .as_deref()
                .is_some_and(|p| p.is(crate::package::pkg::booleans::SYSTEM))
        };
        Ok(state
            .packages
            .get("com.android.systemui")
            .filter(system)
            .map(|ps| ps.app_id))
    }

    /// `callerCanToggle`: empty if allowed, else why not.
    fn caller_can_toggle(
        &self,
        uid: i32,
        source: &AttributionSourceState,
        require_foreground: bool,
    ) -> Result<String, Exception> {
        if self.bluetooth_disallowed()? {
            return Ok("Bluetooth is not allowed".into());
        }
        // `checkBluetoothPermissions`.
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        if !matches!(app_id, SYSTEM_APP_ID | SHELL_APP_ID | ROOT_APP_ID) {
            if let Some(package) = source.package.as_deref()
                && !self.host.owns_package(uid, package)?
            {
                return Err(Exception::security(format!(
                    "Package {package} does not belong to {uid}"
                )));
            }
            if require_foreground && !self.caller_is_foreground_user(uid)? {
                return Ok("Missing Bluetooth permission".into());
            }
            if !self.connect_permission(source)? {
                return Ok("Missing Bluetooth permission".into());
            }
        }
        if require_foreground && !self.compat_change_restriction(uid, source.package.as_deref())? {
            return Ok("Caller does not match restriction criteria".into());
        }
        Ok(String::new())
    }

    /// `checkCompatChangeRestriction`: an app targeting T or later toggles
    /// only privileged, a system app, or an owner (there is none).
    fn compat_change_restriction(
        &self,
        uid: i32,
        package: Option<&str>,
    ) -> Result<bool, Exception> {
        if !self.host.change_enabled(RESTRICT_ENABLE_DISABLE, uid)? {
            return Ok(true);
        }
        let (state, resolution) = self.host.live_packages()?;
        let privileged = self
            .host
            .check_permission(Some(BLUETOOTH_PRIVILEGED), uid)?
            == 0
            || Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: SYSTEM_APP_ID,
            }
            .check_uid_signatures_captured(uid, SYSTEM_APP_ID, false)
            .map_err(|e| self.host.unimplemented_named("bluetooth_manager", e.0))??
                == SIGNATURE_MATCH;
        let system = package
            .map(|p| self.host.application_info(p, 0, Some(&state)))
            .transpose()?
            .flatten()
            .is_some_and(|ai| ai.flags & FLAG_SYSTEM != 0);
        // No device or profile owner: device_policy keeps none.
        Ok(privileged || system)
    }

    /// `enforcePrivileged`.
    fn enforce_privileged(&self, uid: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(BLUETOOTH_PRIVILEGED), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "Neither user {uid} nor current process has {BLUETOOTH_PRIVILEGED}."
            )));
        }
        Ok(())
    }

    /// `isBleScanAvailable`.
    fn ble_scan_available(&self) -> bool {
        let modes = self.modes.lock().unwrap();
        !(modes.airplane && !self.enable.load(Ordering::Relaxed))
            && !modes.satellite
            && modes.scan_allowed
    }

    /// `registerAdapter`: the adapter's IBluetooth, if bound.
    fn register_adapter(&self, callback: u32) -> Option<Binder> {
        let mut callbacks = self.callbacks.lock().unwrap();
        if !callbacks.contains_key(&callback) {
            let strong = self.host.process.strong(callback);
            let weak = self.me.clone();
            let death = self.host.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(bm) = weak.upgrade()
                        && let Some((strong, death)) =
                            bm.callbacks.lock().unwrap().remove(&callback)
                    {
                        bm.host.process.clear_death(&strong, death);
                    }
                }),
            );
            callbacks.insert(callback, (strong, death));
        }
        drop(callbacks);
        self.adapter
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| Binder::Handle(a.handle))
    }

    fn unregister_adapter(&self, callback: u32) {
        if let Some((strong, death)) = self.callbacks.lock().unwrap().remove(&callback) {
            self.host.process.clear_death(&strong, death);
        }
    }

    /// `getAddress` and `getName`: of the app host's user with
    /// BLUETOOTH_CONNECT; the address only with LOCAL_MAC_ADDRESS.
    fn local(
        &self,
        uid: i32,
        source: &AttributionSourceState,
        address: bool,
    ) -> Result<Option<String>, Exception> {
        if !self.connect_permission(source)? {
            return Ok(None);
        }
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        if app_id != SYSTEM_APP_ID && !self.caller_is_foreground_user(uid)? {
            return Ok(None);
        }
        if !address {
            return Ok(self.name.lock().unwrap().clone());
        }
        if self.host.check_permission(Some(LOCAL_MAC_ADDRESS), uid)? != 0 {
            return Ok(Some(DEFAULT_MAC_ADDRESS.into()));
        }
        Ok(self.address.lock().unwrap().clone())
    }

    /// `isHearingAidProfileSupported`: with BLE, ASHA as
    /// `bluetooth.profile.asha.central.enabled` says, else off on cars,
    /// watches and TVs.
    fn hearing_aid_supported(&self) -> Result<bool, Exception> {
        let (state, _) = self.host.live_packages()?;
        let feature = |name: &str| {
            state
                .system
                .features
                .iter()
                .any(|(n, v)| n == name && *v >= 0)
        };
        if !feature(FEATURE_BLUETOOTH_LE) {
            return Ok(false);
        }
        let by_default = !(feature(FEATURE_AUTOMOTIVE)
            || feature(FEATURE_WATCH)
            || feature(FEATURE_TELEVISION)
            || feature(FEATURE_LEANBACK));
        Ok(super::system_boolean(
            self.host
                .property("bluetooth.profile.asha.central.enabled")
                .as_deref(),
            by_default,
        ))
    }
}

/// `requireNonNull` of an argument.
fn required<T>(value: Option<T>, what: &str) -> Result<T, Exception> {
    value.ok_or_else(|| Exception::new(EX_NULL_POINTER, what.to_string()))
}

/// The service (`BluetoothServiceBinder`).
pub struct BluetoothManagerService(pub Arc<BluetoothManager>);

impl Service for BluetoothManagerService {
    fn descriptor(&self) -> &str {
        bm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let bm = &self.0;
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            bm::REGISTER_ADAPTER => {
                let a = bm::RegisterAdapter::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.callback {
                    Some(Binder::Handle(h)) => {
                        let service = bm.register_adapter(h);
                        bm::write_register_adapter_reply(&mut p, service);
                        Ok(())
                    }
                    _ => Err(Exception::new(
                        EX_NULL_POINTER,
                        "Callback cannot be null in registerAdapter",
                    )),
                }
            }
            bm::UNREGISTER_ADAPTER => {
                let a = bm::UnregisterAdapter::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.callback {
                    Some(Binder::Handle(h)) => {
                        bm.unregister_adapter(h);
                        bm::write_unregister_adapter_reply(&mut p);
                        Ok(())
                    }
                    _ => Err(Exception::new(
                        EX_NULL_POINTER,
                        "Callback cannot be null in unregisterAdapter",
                    )),
                }
            }
            bm::ENABLE | bm::ENABLE_NO_AUTO_CONNECT => {
                let quiet = call.code == bm::ENABLE_NO_AUTO_CONNECT;
                let source = if quiet {
                    bm::EnableNoAutoConnect::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .attribution_source
                } else {
                    bm::Enable::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .attribution_source
                };
                (|| {
                    let source = required(source, "AttributionSource cannot be null in enable")?;
                    let error = bm.caller_can_toggle(uid, &source, !quiet)?;
                    if !error.is_empty() {
                        eprintln!("app host: bluetooth: enable(): FAILED: {error}");
                        return Ok(false);
                    }
                    if quiet && uid.rem_euclid(PER_USER_RANGE as i32) != NFC_APP_ID {
                        return Err(Exception::security(
                            "No permission to enable Bluetooth quietly",
                        ));
                    }
                    bm.post_and_wait(move |bm, core| bm.enable_from_binder(core, quiet))
                })()
                .map(|ok| match quiet {
                    false => bm::write_enable_reply(&mut p, ok),
                    true => bm::write_enable_no_auto_connect_reply(&mut p, ok),
                })
            }
            bm::DISABLE => {
                let a = bm::Disable::<AttributionSourceState>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let source = required(
                        a.attribution_source,
                        "AttributionSource cannot be null in disable",
                    )?;
                    if !a.persist {
                        bm.enforce_privileged(uid)?;
                    }
                    let error = bm.caller_can_toggle(uid, &source, true)?;
                    if !error.is_empty() {
                        eprintln!("app host: bluetooth: disable(): FAILED: {error}");
                        return Ok(false);
                    }
                    let persist = a.persist;
                    bm.post_and_wait(move |bm, core| bm.disable_from_binder(core, persist))
                })()
                .map(|ok| bm::write_disable_reply(&mut p, ok))
            }
            bm::GET_STATE => {
                bm::GetState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bm::write_get_state_reply(&mut p, bm.state.get());
                Ok(())
            }
            bm::GET_ADDRESS | bm::GET_NAME => {
                let address = call.code == bm::GET_ADDRESS;
                let source = if address {
                    bm::GetAddress::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .attribution_source
                } else {
                    bm::GetName::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .attribution_source
                };
                required(source, "AttributionSource cannot be null in getAddress")
                    .and_then(|s| bm.local(uid, &s, address))
                    .map(|value| match address {
                        true => bm::write_get_address_reply(&mut p, &value),
                        false => bm::write_get_name_reply(&mut p, &value),
                    })
            }
            bm::IS_BLE_SCAN_AVAILABLE => {
                bm::IsBleScanAvailable::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bm::write_is_ble_scan_available_reply(&mut p, bm.ble_scan_available());
                Ok(())
            }
            bm::ENABLE_BLE | bm::DISABLE_BLE => {
                let enable = call.code == bm::ENABLE_BLE;
                let (source, token) = if enable {
                    let a = bm::EnableBle::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.attribution_source, a.b)
                } else {
                    let a = bm::DisableBle::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.attribution_source, a.b)
                };
                (|| {
                    let source = required(source, "AttributionSource cannot be null in enableBle")?;
                    let Some(Binder::Handle(token)) = token else {
                        return Err(Exception::new(
                            EX_NULL_POINTER,
                            "IBinder cannot be null in enableBle",
                        ));
                    };
                    let error = bm.caller_can_toggle(uid, &source, false)?;
                    if !error.is_empty() {
                        eprintln!("app host: bluetooth: enableBle(): FAILED: {error}");
                        return Ok(false);
                    }
                    let package = source.package.clone().unwrap_or_default();
                    bm.post_and_wait(move |bm, core| {
                        if enable {
                            bm.enable_ble(&package, token)
                        } else {
                            bm.disable_ble(core, &package, token)
                        }
                    })?
                })()
                .map(|ok| match enable {
                    true => bm::write_enable_ble_reply(&mut p, ok),
                    false => bm::write_disable_ble_reply(&mut p, ok),
                })
            }
            bm::IS_HEARING_AID_PROFILE_SUPPORTED => {
                bm::IsHearingAidProfileSupported::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bm.hearing_aid_supported()
                    .map(|s| bm::write_is_hearing_aid_profile_supported_reply(&mut p, s))
            }
            bm::IS_AUTO_ON_SUPPORTED => {
                bm::IsAutoOnSupported::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bm.enforce_privileged(uid).and_then(|()| {
                    if bm.auto_on {
                        return Err(bm
                            .host
                            .unimplemented_named("bluetooth_manager", "Auto On (#1787)"));
                    }
                    bm::write_is_auto_on_supported_reply(&mut p, false);
                    Ok(())
                })
            }
            bm::IS_AUTO_ON_ENABLED | bm::SET_AUTO_ON_ENABLED => {
                bm.enforce_privileged(uid).and_then(|()| {
                    if bm.auto_on {
                        return Err(bm
                            .host
                            .unimplemented_named("bluetooth_manager", "Auto On (#1787)"));
                    }
                    Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        "AutoOnFeature is not supported in current config",
                    ))
                })
            }
            code => {
                let name = bm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(bm
                    .host
                    .unimplemented_named("bluetooth_manager", &format!("{name} (#1787)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handler's queue: due messages in order, a later one waited
    /// for, `hasMessages` and `removeMessages` by kind.
    #[test]
    fn the_queue_runs_messages_as_a_looper_does() {
        let mut q = Queue::default();
        let now = Instant::now();
        q.push(
            now + Duration::from_millis(300),
            Message::HandleEnableDelayed,
        );
        q.push(now, Message::Disable);
        q.push(
            now,
            Message::Enable {
                quiet: false,
                ble: true,
            },
        );
        assert!(q.has(What::HandleEnableDelayed));
        assert!(matches!(q.take(now), Ok(Message::Disable)));
        assert!(matches!(
            q.take(now),
            Ok(Message::Enable {
                quiet: false,
                ble: true
            })
        ));
        assert_eq!(
            q.take(now).err(),
            Some(Some(now + Duration::from_millis(300)))
        );
        q.remove(What::HandleEnableDelayed);
        assert_eq!(q.take(now).err(), Some(None));
    }

    /// The BLE states are off to BR/EDR users; the state's waiters.
    #[test]
    fn states_are_seen_as_the_adapter_reports_them() {
        assert_eq!(
            [
                STATE_BLE_TURNING_ON,
                STATE_BLE_ON,
                STATE_BLE_TURNING_OFF,
                STATE_TURNING_ON
            ]
            .map(br_edr_state),
            [STATE_OFF, STATE_OFF, STATE_OFF, STATE_TURNING_ON]
        );
        let state = AdapterState {
            state: Mutex::new(STATE_OFF),
            changed: Condvar::new(),
        };
        assert!(state.wait_for(Duration::ZERO, &[STATE_OFF]));
        assert!(!state.wait_for(Duration::from_millis(10), &[STATE_ON]));
        state.set(STATE_ON);
        assert!(state.one_of(&[STATE_ON, STATE_BLE_ON]));
    }

    /// `checkBluetoothAddress` and the radio modes' settings.
    #[test]
    fn addresses_and_radio_modes_are_read_as_the_original_reads_them() {
        assert!(valid_address("00:11:22:AA:BB:CC"));
        assert!(!valid_address("00:11:22:aa:bb:cc"));
        assert!(!valid_address("00-11-22-AA-BB-CC"));
        assert!(radio_mode(Some("cell,bluetooth,wifi"), Some("1".into())));
        assert!(!radio_mode(Some("cell,wifi"), Some("1".into())));
        assert!(!radio_mode(None, Some("1".into())));
        assert!(!radio_mode(Some("bluetooth"), Some("x".into())));
        assert_eq!(int_or(Some("2".into()), 1), 2);
        assert_eq!(int_or(None, 1), 1);
    }
}
