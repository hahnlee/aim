//! The app host's connection to SurfaceFlinger (`ISurfaceComposer
//! .createConnection`, `ISurfaceComposerClient.createSurface` at
//! `android-16.0.0_r1`, their C++ AIDL backend's wire form): the layers of
//! its windows.

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result as ParcelResult};

/// `ISurfaceComposer.createConnection`, `ISurfaceComposerClient
/// .createSurface`: their order in the interfaces.
const CREATE_CONNECTION: u32 = 3;
const CREATE_SURFACE: u32 = 1;
const COMPOSER: &str = "android.gui.ISurfaceComposer";
const CLIENT: &str = "android.gui.ISurfaceComposerClient";
/// `ISurfaceComposerClient.eFXSurfaceBufferState`: a layer of buffers.
pub const BUFFER_STATE: i32 = 0x0004_0000;
/// `gui::METADATA_*`.
pub const METADATA_OWNER_UID: i32 = 1;
pub const METADATA_WINDOW_TYPE: i32 = 2;
pub const METADATA_OWNER_PID: i32 = 6;

/// A layer SurfaceFlinger made (`CreateSurfaceResult`), alive while held.
pub struct Layer {
    pub handle: Binder,
    pub id: i32,
    pub name: String,
    pub transform_hint: i32,
    _held: Strong,
}

/// The app host's `ISurfaceComposerClient`.
pub struct Composer {
    client: Strong,
    handle: u32,
}

impl Composer {
    pub fn connect(process: &LocalProcess) -> Result<Composer, String> {
        let sf = super::display::surface_flinger(process)?;
        let mut data = Parcel::new();
        data.write_interface_token(COMPOSER);
        let reply = sf.transact(CREATE_CONNECTION, &data, false).map_err(|s| format!("createConnection: {s}"))?;
        let mut r = reply.reader();
        status(&mut r, "createConnection")?;
        match r.read_binder().map_err(|s| format!("createConnection reply: {s}"))? {
            Some(Binder::Handle(handle)) => Ok(Composer { client: process.strong(handle), handle }),
            _ => Err("createConnection: no client".into()),
        }
    }

    /// The client, as a `SurfaceControl` names it.
    pub fn binder(&self) -> Binder {
        Binder::Handle(self.handle)
    }

    /// A root layer `name` with `flags` and the metadata `metadata`
    /// (key, little-endian `int32` value).
    pub fn create_surface(&self, process: &LocalProcess, name: &str, flags: i32, metadata: &[(i32, i32)]) -> Result<Layer, String> {
        let mut data = Parcel::new();
        data.write_interface_token(CLIENT);
        data.write_string16(Some(name));
        data.write_i32(flags);
        data.write_binder(None); // no parent: the display's root
        // `LayerMetadata`: the entries, each a key and a byte vector.
        data.write_i32(1);
        data.write_i32(metadata.len() as i32);
        for (key, value) in metadata {
            data.write_i32(*key);
            data.write_i32(4);
            data.write_i32(*value);
        }
        let reply = self.client.transact(CREATE_SURFACE, &data, false).map_err(|s| format!("createSurface: {s}"))?;
        let mut r = reply.reader();
        status(&mut r, "createSurface")?;
        let layer = (|| -> ParcelResult<Layer> {
            if r.read_i32()? == 0 {
                return Err(aim_binder_host::parcel::BAD_VALUE);
            }
            let _size = r.read_i32()?;
            let handle = r.read_binder()?.ok_or(aim_binder_host::parcel::BAD_VALUE)?;
            let id = r.read_i32()?;
            let name = r.read_string16()?.unwrap_or_default();
            let transform_hint = r.read_i32()?;
            let Binder::Handle(h) = handle else { return Err(aim_binder_host::parcel::BAD_VALUE) };
            Ok(Layer { handle, id, name, transform_hint, _held: process.strong(h) })
        })()
        .map_err(|s| format!("createSurface reply: {s}"))?;
        Ok(layer)
    }
}

fn status(r: &mut Reader<'_>, what: &str) -> Result<(), String> {
    match r.read_exception() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("{what}: {e:?}")),
        Err(s) => Err(format!("{what}: {s}")),
    }
}
