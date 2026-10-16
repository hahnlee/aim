//! The default display as SurfaceFlinger reports it
//! (`android.gui.ISurfaceComposer` at `android-16.0.0_r1`, its C++ AIDL
//! backend's wire form): the active mode's size and refresh rate and the
//! display's density, which DisplayManager and WindowManager build their
//! display info and configuration from.

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::android_os_iservicemanager as sm;

/// SurfaceFlinger's AIDL service.
const SERVICE: &str = "SurfaceFlingerAIDL";
const DESCRIPTOR: &str = "android.gui.ISurfaceComposer";
/// `getPhysicalDisplayIds`, `getStaticDisplayInfo`,
/// `getDynamicDisplayInfoFromId`: their order in the interface.
const GET_PHYSICAL_DISPLAY_IDS: u32 = 6;
const GET_STATIC_DISPLAY_INFO: u32 = 12;
const GET_DYNAMIC_DISPLAY_INFO_FROM_ID: u32 = 13;
/// libbinder's `UNEXPECTED_NULL` status.
const UNEXPECTED_NULL: i32 = -2147483640;

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub id: i64,
    pub width: i32,
    pub height: i32,
    pub x_dpi: f32,
    pub y_dpi: f32,
    pub refresh_rate: f32,
    /// `StaticDisplayInfo.density`: dots per inch over 160.
    pub density: f32,
}

impl Display {
    pub fn density_dpi(&self) -> i32 {
        (self.density * 160.0).round() as i32
    }

    /// The default display, from SurfaceFlinger.
    pub fn query(process: &LocalProcess) -> Result<Display, String> {
        let sf = surface_flinger(process)?;
        let ids = call(
            &sf,
            GET_PHYSICAL_DISPLAY_IDS,
            |_| {},
            |r| {
                let n = r.read_i32()?;
                (0..n.max(0))
                    .map(|_| r.read_i64())
                    .collect::<ParcelResult<Vec<_>>>()
            },
        )?;
        let id = *ids.first().ok_or("SurfaceFlinger has no display")?;
        let density = call(
            &sf,
            GET_STATIC_DISPLAY_INFO,
            |p| p.write_i64(id),
            |r| {
                present(r)?;
                parcelable(r, |r| {
                    let _connection_type = r.read_i32()?;
                    let _port = r.read_i32()?;
                    r.read_f32()
                })
            },
        )?;
        let (modes, active) = call(
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
                            parcelable(r, |r| {
                                let id = r.read_i32()?;
                                present(r)?;
                                let (width, height) =
                                    parcelable(r, |r| Ok((r.read_i32()?, r.read_i32()?)))?;
                                let (x_dpi, y_dpi) = (r.read_f32()?, r.read_f32()?);
                                let hdr = r.read_i32()?;
                                for _ in 0..hdr.max(0) {
                                    r.read_i32()?;
                                }
                                let _peak = r.read_f32()?;
                                let vsync = r.read_f32()?;
                                Ok((id, width, height, x_dpi, y_dpi, vsync))
                            })
                        })
                        .collect::<ParcelResult<Vec<_>>>()?;
                    Ok((modes, r.read_i32()?))
                })
            },
        )?;
        let (_, width, height, x_dpi, y_dpi, refresh_rate) = modes
            .into_iter()
            .find(|m| m.0 == active)
            .ok_or("SurfaceFlinger's active mode is not among its modes")?;
        Ok(Display {
            id,
            width,
            height,
            x_dpi,
            y_dpi,
            refresh_rate,
            density,
        })
    }
}

fn surface_flinger(process: &LocalProcess) -> Result<Strong, String> {
    let mut data = Parcel::new();
    sm::CheckService {
        name: Some(SERVICE.into()),
    }
    .write(&mut data);
    let reply = process
        .transact(0, sm::CHECK_SERVICE, &data, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    match sm::read_check_service_reply(&mut reply.reader())
        .map_err(|s| format!("{SERVICE}: {s}"))?
    {
        Ok(Some(Binder::Handle(handle))) => Ok(process.strong(handle)),
        _ => Err(format!("no {SERVICE}")),
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
fn parcelable<T>(
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
