//! The default display as SurfaceFlinger reports it
//! (`android.gui.ISurfaceComposer` at `android-16.0.0_r1`, its C++ AIDL
//! backend's wire form): its modes, density, color and HDR capabilities,
//! which DisplayManager and WindowManager build their display info from.
//! Initial resource metrics come directly from the display owner.

use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, LocalProcess, Received, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, DEAD_OBJECT, Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_os_iservicecallback as sc;
use aim_service_aidl::android_os_iservicemanager as sm;

/// SurfaceFlinger's AIDL service.
pub(super) const SERVICE: &str = "SurfaceFlingerAIDL";
const DESCRIPTOR: &str = "android.gui.ISurfaceComposer";
/// `getPhysicalDisplayIds`, `getStaticDisplayInfo`,
/// `getDynamicDisplayInfoFromId`: their order in the interface.
const GET_PHYSICAL_DISPLAY_IDS: u32 = 6;
const GET_STATIC_DISPLAY_INFO: u32 = 12;
const GET_DYNAMIC_DISPLAY_INFO_FROM_ID: u32 = 13;
const GET_OVERLAY_SUPPORT: u32 = 68;
/// libbinder's `UNEXPECTED_NULL` status.
const UNEXPECTED_NULL: i32 = -2147483640;

/// One of the display's modes (`DisplayMode`).
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub id: i32,
    pub width: i32,
    pub height: i32,
    pub x_dpi: f32,
    pub y_dpi: f32,
    pub supported_hdr_types: Vec<i32>,
    pub peak_refresh_rate: f32,
    pub vsync_rate: f32,
    pub app_vsync_offset: i64,
    pub presentation_deadline: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub id: i64,
    /// `StaticDisplayInfo.density`: dots per inch over 160.
    pub density: f32,
    pub secure: bool,
    pub install_orientation: i32,
    pub modes: Vec<Mode>,
    pub active_mode: i32,
    pub render_frame_rate: f32,
    pub supported_color_modes: Vec<i32>,
    pub active_color_mode: i32,
    /// `HdrCapabilities`: the types and the luminances (max, max average,
    /// min).
    pub hdr_types: Vec<i32>,
    pub hdr_luminance: (f32, f32, f32),
    pub has_arr_support: bool,
    /// `FrameRateCategoryRate`: normal and high.
    pub frame_rate_category_rate: (f32, f32),
    pub supported_refresh_rates: Vec<f32>,
    pub width: i32,
    pub height: i32,
    pub x_dpi: f32,
    pub y_dpi: f32,
    pub refresh_rate: f32,
}

/// Initial resource metrics, owned by the display server and density property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub width: i32,
    pub height: i32,
    pub density_dpi: i32,
}

impl Metrics {
    pub fn project(info: &aim_hostcall::display::Connect, density: &str) -> Result<Self, String> {
        let positive = |v: u32| i32::try_from(v).ok().filter(|v| *v > 0);
        Ok(Self {
            width: positive(info.width).ok_or("invalid display server width")?,
            height: positive(info.height).ok_or("invalid display server height")?,
            density_dpi: density
                .parse::<i32>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or("invalid ro.sf.lcd_density")?,
        })
    }

    pub fn of(display: &Display) -> Self {
        Self {
            width: display.width,
            height: display.height,
            density_dpi: display.density_dpi(),
        }
    }

    pub fn check(self, display: &Display) -> Result<(), String> {
        // SF has already applied install orientation to its reported mode.
        // AIM's composer reports Transform::NONE (android-16.0.0_r1).
        let sf = Self::of(display);
        if self != sf {
            return Err(format!(
                "display metrics mismatch: display server {self:?}, SurfaceFlinger {sf:?} (install orientation {})",
                display.install_orientation
            ));
        }
        Ok(())
    }
}

impl Display {
    pub fn density_dpi(&self) -> i32 {
        (self.density * 160.0).round() as i32
    }

    pub fn active(&self) -> &Mode {
        self.modes
            .iter()
            .find(|m| m.id == self.active_mode)
            .expect("the active mode is a mode")
    }

    /// The default display, from SurfaceFlinger.
    pub fn query(process: &Arc<LocalProcess>) -> Result<Display, String> {
        let sf = wait_for_service(process, SERVICE)?;
        let ids = call(&sf, GET_PHYSICAL_DISPLAY_IDS, |_| {}, ints64)?;
        let id = *ids.first().ok_or("SurfaceFlinger has no display")?;
        let (density, secure, install_orientation) = call(
            &sf,
            GET_STATIC_DISPLAY_INFO,
            |p| p.write_i64(id),
            |r| {
                present(r)?;
                parcelable(r, |r| {
                    let _connection_type = r.read_i32()?;
                    let _port = r.read_i32()?;
                    let density = r.read_f32()?;
                    let secure = r.read_bool()?;
                    // `@nullable DeviceProductInfo`: none on a built-in display.
                    if r.read_i32()? != 0 {
                        return Err(BAD_VALUE);
                    }
                    Ok((density, secure, r.read_i32()?))
                })
            },
        )?;
        let mut display = call(
            &sf,
            GET_DYNAMIC_DISPLAY_INFO_FROM_ID,
            |p| p.write_i64(id),
            |r| {
                present(r)?;
                parcelable(r, |r| {
                    let n = r.read_i32()?;
                    let modes = (0..n.max(0))
                        .map(|_| {
                            present(r)?;
                            parcelable(r, mode)
                        })
                        .collect::<ParcelResult<Vec<_>>>()?;
                    let active_mode = r.read_i32()?;
                    let render_frame_rate = r.read_f32()?;
                    let supported_color_modes = ints(r)?;
                    let active_color_mode = r.read_i32()?;
                    present(r)?;
                    let (hdr_types, hdr_luminance) = parcelable(r, |r| {
                        Ok((ints(r)?, (r.read_f32()?, r.read_f32()?, r.read_f32()?)))
                    })?;
                    let _auto_low_latency = r.read_bool()?;
                    let _game_content_type = r.read_bool()?;
                    let _preferred_boot_mode = r.read_i32()?;
                    let has_arr_support = r.read_bool()?;
                    present(r)?;
                    let frame_rate_category_rate =
                        parcelable(r, |r| Ok((r.read_f32()?, r.read_f32()?)))?;
                    let n = r.read_i32()?;
                    let supported_refresh_rates = (0..n.max(0))
                        .map(|_| r.read_f32())
                        .collect::<ParcelResult<Vec<_>>>()?;
                    Ok(Display {
                        id,
                        density,
                        secure: false,
                        install_orientation: 0,
                        modes,
                        active_mode,
                        render_frame_rate,
                        supported_color_modes,
                        active_color_mode,
                        hdr_types,
                        hdr_luminance,
                        has_arr_support,
                        frame_rate_category_rate,
                        supported_refresh_rates,
                        width: 0,
                        height: 0,
                        x_dpi: 0.0,
                        y_dpi: 0.0,
                        refresh_rate: 0.0,
                    })
                })
            },
        )?;
        let active = display
            .modes
            .iter()
            .find(|m| m.id == display.active_mode)
            .cloned()
            .ok_or("SurfaceFlinger's active mode is not among its modes")?;
        display.secure = secure;
        display.install_orientation = install_orientation;
        (
            display.width,
            display.height,
            display.x_dpi,
            display.y_dpi,
            display.refresh_rate,
        ) = (
            active.width,
            active.height,
            active.x_dpi,
            active.y_dpi,
            active.vsync_rate,
        );
        Ok(display)
    }

    /// `getOverlaySupport`'s reply data after its status, as DisplayManager
    /// returns SurfaceFlinger's `OverlayProperties`.
    pub fn overlay_support(process: &Arc<LocalProcess>) -> Result<Vec<u8>, String> {
        let sf = wait_for_service(process, SERVICE)?;
        call(
            &sf,
            GET_OVERLAY_SUPPORT,
            |_| {},
            |r| {
                let start = r.position();
                present(r)?;
                parcelable(r, |_| Ok(()))?;
                Ok(r.since(start).0.to_vec())
            },
        )
    }
}

#[cfg(test)]
impl Display {
    /// What SurfaceFlinger reports for the display server's 1080x1920
    /// panel at 320 dpi (captured on the pinned image).
    pub fn phone_for_test() -> Display {
        let rate = f32::from_bits(0x4270_0001);
        Display {
            id: 0,
            density: 2.0,
            secure: true,
            install_orientation: 0,
            modes: vec![Mode {
                id: 1,
                width: 1080,
                height: 1920,
                x_dpi: 72.0,
                y_dpi: 72.0,
                supported_hdr_types: Vec::new(),
                peak_refresh_rate: rate,
                vsync_rate: rate,
                app_vsync_offset: 1_000_000,
                presentation_deadline: 16_666_666,
            }],
            active_mode: 1,
            render_frame_rate: rate,
            supported_color_modes: vec![0],
            active_color_mode: 0,
            hdr_types: Vec::new(),
            hdr_luminance: (500.0, 500.0, 0.0),
            has_arr_support: false,
            frame_rate_category_rate: (60.0, 90.0),
            supported_refresh_rates: vec![rate],
            width: 1080,
            height: 1920,
            x_dpi: 72.0,
            y_dpi: 72.0,
            refresh_rate: rate,
        }
    }
}

/// `DisplayMode`.
fn mode(r: &mut Reader<'_>) -> ParcelResult<Mode> {
    let id = r.read_i32()?;
    present(r)?;
    let (width, height) = parcelable(r, |r| Ok((r.read_i32()?, r.read_i32()?)))?;
    let (x_dpi, y_dpi) = (r.read_f32()?, r.read_f32()?);
    let supported_hdr_types = ints(r)?;
    let peak_refresh_rate = r.read_f32()?;
    let vsync_rate = r.read_f32()?;
    let app_vsync_offset = r.read_i64()?;
    let _sf_vsync_offset = r.read_i64()?;
    let presentation_deadline = r.read_i64()?;
    Ok(Mode {
        id,
        width,
        height,
        x_dpi,
        y_dpi,
        supported_hdr_types,
        peak_refresh_rate,
        vsync_rate,
        app_vsync_offset,
        presentation_deadline,
    })
}

/// An `int[]`; null as empty.
fn ints(r: &mut Reader<'_>) -> ParcelResult<Vec<i32>> {
    let n = r.read_i32()?;
    (0..n.max(0)).map(|_| r.read_i32()).collect()
}

fn ints64(r: &mut Reader<'_>) -> ParcelResult<Vec<i64>> {
    let n = r.read_i32()?;
    (0..n.max(0)).map(|_| r.read_i64()).collect()
}

/// The service `name` from servicemanager (`checkService`): one of
/// SurfaceFlinger's, vold.
pub(super) fn check_service(process: &LocalProcess, name: &str) -> Result<Strong, String> {
    find_service(process, name)?.ok_or_else(|| format!("no {name}"))
}

fn find_service(process: &LocalProcess, name: &str) -> Result<Option<Strong>, String> {
    let mut data = Parcel::new();
    sm::CheckService {
        name: Some(name.into()),
    }
    .write(&mut data);
    let reply = process
        .transact(0, sm::CHECK_SERVICE, &data, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    match sm::read_check_service_reply(&mut reply.reader()).map_err(|s| format!("{name}: {s}"))? {
        Ok(Some(Binder::Handle(handle))) => Ok(Some(process.strong(handle))),
        _ => Ok(None),
    }
}

/// The service `name` as libbinder's `waitForService` finds it, which
/// SurfaceComposerClient's `ComposerService` and `ComposerServiceAIDL`
/// connect with: now if registered, else once servicemanager reports its
/// registration (`registerForNotifications`). As libbinder, it checks again
/// each second it waited, for a report no pool thread was free to take,
/// and waits without a bound.
pub(super) fn wait_for_service(process: &Arc<LocalProcess>, name: &str) -> Result<Strong, String> {
    if let Some(service) = find_service(process, name)? {
        return Ok(service);
    }
    let waiter = Arc::new(Waiter {
        process: Arc::downgrade(process),
        registered: Mutex::new(None),
        changed: Condvar::new(),
    });
    let callback = process.add_callback(waiter.clone());
    notifications(process, name, callback, sm::REGISTER_FOR_NOTIFICATIONS)?;
    let service = loop {
        let registered = waiter.registered.lock().unwrap();
        let (mut registered, _) = waiter
            .changed
            .wait_timeout_while(registered, Duration::from_secs(1), |r| r.is_none())
            .unwrap();
        if let Some(service) = registered.take() {
            break service;
        }
        drop(registered);
        eprintln!("app host: waited one second for {name}");
        if let Some(service) = find_service(process, name)? {
            break service;
        }
    };
    notifications(process, name, callback, sm::UNREGISTER_FOR_NOTIFICATIONS)?;
    Ok(service)
}

/// `registerForNotifications` or `unregisterForNotifications` of
/// `callback` for `name`.
fn notifications(
    process: &LocalProcess,
    name: &str,
    callback: Binder,
    code: u32,
) -> Result<(), String> {
    let mut data = Parcel::new();
    // Both calls' arguments are the same.
    sm::RegisterForNotifications {
        name: Some(name.into()),
        callback: Some(callback),
    }
    .write(&mut data);
    let reply = process
        .transact(0, code, &data, false)
        .map_err(|s| format!("servicemanager#{code}: {s}"))?;
    sm::read_register_for_notifications_reply(&mut reply.reader())
        .map_err(|s| format!("servicemanager#{code} reply: {s}"))?
        .map_err(|e| format!("servicemanager#{code} for {name}: {e:?}"))
}

/// The `IServiceCallback` of a [`wait_for_service`].
struct Waiter {
    process: Weak<LocalProcess>,
    registered: Mutex<Option<Strong>>,
    changed: Condvar,
}

impl Service for Waiter {
    fn descriptor(&self) -> &str {
        sc::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != sc::ON_REGISTRATION {
            return Err(UNKNOWN_TRANSACTION);
        }
        let Some(Binder::Handle(handle)) = sc::OnRegistration::read(&mut call.data)?.binder else {
            return Err(BAD_VALUE);
        };
        let process = self.process.upgrade().ok_or(DEAD_OBJECT)?;
        *self.registered.lock().unwrap() = Some(process.strong(handle));
        self.changed.notify_all();
        Ok(Parcel::new())
    }
}

/// A call to the service `name` as `ServiceManager.getService` finds it:
/// another process's, or a node of this process itself.
pub(super) fn transact_service(
    process: &LocalProcess,
    name: &str,
    code: u32,
    data: &Parcel,
) -> Result<Received, String> {
    let mut lookup = Parcel::new();
    sm::CheckService {
        name: Some(name.into()),
    }
    .write(&mut lookup);
    let reply = process
        .transact(0, sm::CHECK_SERVICE, &lookup, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    let called = match sm::read_check_service_reply(&mut reply.reader())
        .map_err(|s| format!("{name}: {s}"))?
    {
        Ok(Some(Binder::Handle(handle))) => process.transact(handle, code, data, false),
        Ok(Some(Binder::Local(ptr))) => process
            .local_service(ptr)
            .ok_or_else(|| format!("{name}: no local node"))?
            .transact(code, data, false),
        _ => return Err(format!("no {name}")),
    };
    called.map_err(|s| format!("{name}#{code}: {s}"))
}

/// A call and its reply: the status, then what `read` reads.
fn call<T>(
    sf: &Strong,
    code: u32,
    write: impl FnOnce(&mut Parcel),
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
) -> Result<T, String> {
    let mut data = Parcel::new();
    data.write_interface_token(DESCRIPTOR);
    write(&mut data);
    let reply = sf
        .transact(code, &data, false)
        .map_err(|s| format!("{DESCRIPTOR}#{code}: {s}"))?;
    let mut r = reply.reader();
    match r.read_exception() {
        Ok(Ok(())) => read(&mut r).map_err(|s| format!("{DESCRIPTOR}#{code} reply: {s}")),
        Ok(Err(e)) => Err(format!("{DESCRIPTOR}#{code}: {e:?}")),
        Err(s) => Err(format!("{DESCRIPTOR}#{code} reply: {s}")),
    }
}

/// `Parcel::readParcelable`'s non-null marker.
pub(super) fn present(r: &mut Reader<'_>) -> ParcelResult<()> {
    match r.read_i32()? {
        // `UNEXPECTED_NULL`.
        0 => Err(UNEXPECTED_NULL),
        _ => Ok(()),
    }
}

/// A structured parcelable: its size, which includes the size's own
/// four bytes, then its fields; fields past the ones `read` reads are
/// skipped, as a reader of an older version skips them.
pub(super) fn parcelable<T>(
    r: &mut Reader<'_>,
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
) -> ParcelResult<T> {
    let start = r.position();
    let size = r.read_i32()?;
    let end = start
        .checked_add(usize::try_from(size).map_err(|_| BAD_VALUE)?)
        .filter(|&end| end >= r.position() && end - r.position() <= r.remaining())
        .ok_or(BAD_VALUE)?;
    let value = read(r)?;
    if r.position() > end {
        return Err(BAD_VALUE);
    }
    r.set_position(end);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno, uapi::*};
    use std::collections::HashMap;
    use std::sync::mpsc;

    #[test]
    fn initial_metrics_match_surfaceflinger_and_reject_disagreement() {
        let mut display = Display::phone_for_test();
        let info = aim_hostcall::display::Connect {
            width: 1080,
            height: 1920,
            ..Default::default()
        };
        let metrics = Metrics::project(&info, "320").unwrap();
        assert_eq!(metrics, Metrics::of(&display));
        metrics.check(&display).unwrap();
        display.width = 1920;
        display.height = 1080;
        display.install_orientation = 1;
        let error = metrics.check(&display).unwrap_err();
        assert!(
            error
                .contains("display server Metrics { width: 1080, height: 1920, density_dpi: 320 }")
        );
        assert!(
            error
                .contains("SurfaceFlinger Metrics { width: 1920, height: 1080, density_dpi: 320 }")
        );
        assert!(Metrics::project(&info, "").is_err());
        assert!(Metrics::project(&info, "0").is_err());
        assert!(Metrics::project(&aim_hostcall::display::Connect::default(), "320").is_err());
    }

    #[test]
    fn structured_parcelables_skip_newer_fields() {
        let mut p = Parcel::new();
        p.write_i32(16);
        p.write_i32(7);
        p.write_i32(8);
        p.write_i32(9);
        p.write_i32(10);
        let mut r = p.reader();
        assert_eq!(parcelable(&mut r, |r| r.read_i32()).unwrap(), 7);
        assert_eq!(r.read_i32().unwrap(), 10);
    }

    /// servicemanager's naming and registration notifications.
    struct Manager {
        process: Weak<LocalProcess>,
        names: Mutex<HashMap<String, Strong>>,
        callbacks: Mutex<Vec<(String, Strong)>>,
        waiting: mpsc::Sender<Result<String, String>>,
    }

    impl Service for Manager {
        fn descriptor(&self) -> &str {
            sm::DESCRIPTOR
        }

        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let process = self.process.upgrade().unwrap();
            let mut p = Parcel::new();
            match call.code {
                sm::ADD_SERVICE => {
                    let a = sm::AddService::read(&mut call.data)?;
                    let (Some(name), Some(Binder::Handle(h))) = (a.name, a.service) else {
                        return Err(BAD_VALUE);
                    };
                    for (_, callback) in self
                        .callbacks
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|c| c.0 == name)
                    {
                        let mut data = Parcel::new();
                        sc::OnRegistration {
                            name: Some(name.clone()),
                            binder: Some(Binder::Handle(h)),
                        }
                        .write(&mut data);
                        callback.transact(sc::ON_REGISTRATION, &data, true)?;
                    }
                    self.names.lock().unwrap().insert(name, process.strong(h));
                    sm::write_add_service_reply(&mut p);
                }
                sm::CHECK_SERVICE => {
                    let a = sm::CheckService::read(&mut call.data)?;
                    let names = self.names.lock().unwrap();
                    let found = names.get(a.name.as_deref().unwrap()).map(Strong::binder);
                    sm::write_check_service_reply(&mut p, found);
                }
                sm::REGISTER_FOR_NOTIFICATIONS => {
                    let a = sm::RegisterForNotifications::read(&mut call.data)?;
                    let (Some(name), Some(Binder::Handle(h))) = (a.name, a.callback) else {
                        return Err(BAD_VALUE);
                    };
                    self.callbacks
                        .lock()
                        .unwrap()
                        .push((name.clone(), process.strong(h)));
                    self.waiting.send(Ok(name)).unwrap();
                    sm::write_register_for_notifications_reply(&mut p);
                }
                sm::UNREGISTER_FOR_NOTIFICATIONS => {
                    let a = sm::UnregisterForNotifications::read(&mut call.data)?;
                    let (Some(name), Some(Binder::Handle(h))) = (a.name, a.callback) else {
                        return Err(BAD_VALUE);
                    };
                    self.callbacks
                        .lock()
                        .unwrap()
                        .retain(|c| (c.0.as_str(), c.1.handle) != (name.as_str(), h));
                    sm::write_unregister_for_notifications_reply(&mut p);
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            Ok(p)
        }
    }

    /// SurfaceFlinger's `getOverlaySupport`: one field.
    struct Composer;

    impl Service for Composer {
        fn descriptor(&self) -> &str {
            DESCRIPTOR
        }

        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, GET_OVERLAY_SUPPORT);
            let mut p = Parcel::new();
            p.write_no_exception();
            p.write_i32(1);
            p.write_i32(8);
            p.write_i32(7);
            Ok(p)
        }
    }

    struct NoMemory;

    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
            Err(errno::EFAULT)
        }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
            Err(errno::EFAULT)
        }
        fn get_file(&mut self, _: u32) -> Result<File, i32> {
            Err(errno::EBADF)
        }
        fn install_file(&mut self, _: File) -> Result<u32, i32> {
            Err(errno::EBADF)
        }
        fn close_fd(&mut self, _: u32) {
            panic!("no files")
        }
    }

    #[test]
    fn a_query_before_surfaceflinger_registers_completes_once_it_does() {
        let driver = Driver::new();
        let open = |pid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: 1000,
                    security_context: None,
                },
            )
        };
        let (manager, sf, host) = (open(97261), open(97262), open(97263));
        // The registration a waiting query asks for, or the query's end.
        let (waiting, events) = mpsc::channel();
        let registry = Arc::new(Manager {
            process: Arc::downgrade(&manager),
            names: Mutex::new(HashMap::new()),
            callbacks: Mutex::new(Vec::new()),
            waiting: waiting.clone(),
        });
        let Binder::Local(ptr) = manager.add_service(registry.clone()) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                manager.proc_handle(),
                97261,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        for process in [&manager, &sf, &host] {
            process.start();
        }
        let query = std::thread::spawn({
            let host = host.clone();
            move || {
                let support = Display::overlay_support(&host);
                waiting.send(Err(format!("{support:?}"))).unwrap();
                support
            }
        });
        assert_eq!(events.recv().unwrap(), Ok(SERVICE.to_string()));
        let mut p = Parcel::new();
        sm::AddService {
            name: Some(SERVICE.into()),
            service: Some(sf.add_service(Arc::new(Composer))),
            allow_isolated: false,
            dump_priority: 0,
        }
        .write(&mut p);
        let reply = sf.transact(0, sm::ADD_SERVICE, &p, false).unwrap();
        sm::read_add_service_reply(&mut reply.reader())
            .unwrap()
            .unwrap();
        let support = query.join().unwrap().unwrap();
        let words: Vec<_> = support
            .chunks(4)
            .map(|w| i32::from_le_bytes(w.try_into().unwrap()))
            .collect();
        assert_eq!(words, [1, 8, 7]);
        assert!(registry.callbacks.lock().unwrap().is_empty());
        registry.names.lock().unwrap().clear();
        for process in [&host, &sf, &manager] {
            process.shutdown().unwrap();
        }
    }
}
