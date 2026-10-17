//! The default display as SurfaceFlinger reports it
//! (`android.gui.ISurfaceComposer` at `android-16.0.0_r1`, its C++ AIDL
//! backend's wire form): its modes, density, color and HDR capabilities,
//! which DisplayManager and WindowManager build their display info and
//! configuration from.

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result as ParcelResult};
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
    pub fn query(process: &LocalProcess) -> Result<Display, String> {
        let sf = check_service(process, SERVICE)?;
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
                        return Err(aim_binder_host::parcel::BAD_VALUE);
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
    pub fn overlay_support(process: &LocalProcess) -> Result<Vec<u8>, String> {
        let sf = check_service(process, SERVICE)?;
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
    let mut data = Parcel::new();
    sm::CheckService {
        name: Some(name.into()),
    }
    .write(&mut data);
    let reply = process
        .transact(0, sm::CHECK_SERVICE, &data, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    match sm::read_check_service_reply(&mut reply.reader())
        .map_err(|s| format!("{name}: {s}"))?
    {
        Ok(Some(Binder::Handle(handle))) => Ok(process.strong(handle)),
        _ => Err(format!("no {name}")),
    }
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
fn present(r: &mut Reader<'_>) -> ParcelResult<()> {
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
        .checked_add(usize::try_from(size).map_err(|_| aim_binder_host::parcel::BAD_VALUE)?)
        .filter(|&end| end >= r.position() && end - r.position() <= r.remaining())
        .ok_or(aim_binder_host::parcel::BAD_VALUE)?;
    let value = read(r)?;
    if r.position() > end {
        return Err(aim_binder_host::parcel::BAD_VALUE);
    }
    r.set_position(end);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
