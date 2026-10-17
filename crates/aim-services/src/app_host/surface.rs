//! The app host's connection to SurfaceFlinger (`ISurfaceComposer
//! .createConnection`, `ISurfaceComposerClient.createSurface` at
//! `android-16.0.0_r1`, their C++ AIDL backend's wire form): the layers of
//! its windows, and the transactions that place them
//! (`ISurfaceComposer::setTransactionState`, `TransactionState` and
//! `layer_state_t`'s `writeToParcel`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result as ParcelResult};

/// `ISurfaceComposer.createConnection`, `ISurfaceComposerClient
/// .createSurface`: their order in the interfaces.
const CREATE_CONNECTION: u32 = 3;
const CREATE_SURFACE: u32 = 1;
const COMPOSER: &str = "android.gui.ISurfaceComposer";
const CLIENT: &str = "android.gui.ISurfaceComposerClient";
/// The service of SurfaceFlinger's transactions, `BnSurfaceComposer
/// ::SET_TRANSACTION_STATE` and its descriptor.
const TRANSACTIONS: &str = "SurfaceFlinger";
const SET_TRANSACTION_STATE: u32 = 8;
const TRANSACTIONS_DESCRIPTOR: &str = "android.ui.ISurfaceComposer";
/// `ISurfaceComposerClient.eFXSurfaceBufferState`, `eFXSurfaceContainer`:
/// a layer of buffers; a layer of layers.
pub const BUFFER_STATE: i32 = 0x0004_0000;
pub const CONTAINER: i32 = 0x0008_0000;
/// `layer_state_t::ePositionChanged`, `eLayerChanged`, `eCropChanged`.
const POSITION_CHANGED: u64 = 0x1;
const LAYER_CHANGED: u64 = 0x2;
const CROP_CHANGED: u64 = 0x10_0000;
/// `ui::Transform::ROT_INVALID`; `gui::CachingHint::Enabled`.
const ROT_INVALID: u32 = 0x80;
const CACHING_ENABLED: i32 = 1;
/// `FrameTimelineInfo::INVALID_VSYNC_ID`.
const INVALID_VSYNC_ID: i64 = -1;
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

/// The app host's `ISurfaceComposerClient`, and SurfaceFlinger's service
/// of transactions.
pub struct Composer {
    client: Strong,
    handle: u32,
    transactions: Strong,
    /// The transactions' apply token (`SurfaceComposerClient::Transaction
    /// ::getDefaultApplyToken`): they apply in order.
    apply_token: Binder,
    next_id: AtomicU32,
}

/// What a transaction changes of one layer (`SurfaceComposerClient
/// ::Transaction`'s `setPosition`, `setLayer`, `setCrop`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Change {
    pub position: Option<(f32, f32)>,
    pub z: Option<i32>,
    /// Left, top, right, bottom, in the layer's space.
    pub crop: Option<[f32; 4]>,
}

impl Composer {
    pub fn connect(process: &LocalProcess) -> Result<Composer, String> {
        let sf = super::display::surface_flinger(process, super::display::SERVICE)?;
        let mut data = Parcel::new();
        data.write_interface_token(COMPOSER);
        let reply = sf.transact(CREATE_CONNECTION, &data, false).map_err(|s| format!("createConnection: {s}"))?;
        let mut r = reply.reader();
        status(&mut r, "createConnection")?;
        let Some(Binder::Handle(handle)) = r.read_binder().map_err(|s| format!("createConnection reply: {s}"))? else {
            return Err("createConnection: no client".into());
        };
        Ok(Composer {
            client: process.strong(handle),
            handle,
            transactions: super::display::surface_flinger(process, TRANSACTIONS)?,
            apply_token: process.add_service(Arc::new(super::activity::Token)),
            next_id: AtomicU32::new(0),
        })
    }

    /// The client, as a `SurfaceControl` names it.
    pub fn binder(&self) -> Binder {
        Binder::Handle(self.handle)
    }

    /// A layer `name` with `flags` and the metadata `metadata` (key,
    /// little-endian `int32` value), a child of `parent`, else at the
    /// display's root.
    pub fn create_surface(
        &self,
        process: &LocalProcess,
        name: &str,
        flags: i32,
        parent: Option<&Layer>,
        metadata: &[(i32, i32)],
    ) -> Result<Layer, String> {
        let mut data = Parcel::new();
        data.write_interface_token(CLIENT);
        data.write_string16(Some(name));
        data.write_i32(flags);
        data.write_binder(parent.map(|p| p.handle));
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

    /// Applies `changes` in one transaction, synchronously.
    pub fn apply(&self, changes: &[(&Layer, Change)]) -> Result<(), String> {
        let id = u64::from(std::process::id()) << 32 | u64::from(self.next_id.fetch_add(1, Ordering::Relaxed));
        let changes: Vec<_> = changes.iter().map(|(layer, change)| (layer.handle, layer.id, *change)).collect();
        let data = transaction(id, self.apply_token, &changes);
        let reply = self
            .transactions
            .transact(SET_TRANSACTION_STATE, &data, false)
            .map_err(|s| format!("setTransactionState: {s}"))?;
        match reply.reader().read_i32() {
            Ok(0) | Err(_) => Ok(()),
            Ok(status) => Err(format!("setTransactionState: {status}")),
        }
    }
}

/// `setTransactionState(TransactionState)` of `changes` (each to a
/// layer's handle and id): no display changes, buffers, input commands or
/// callbacks.
fn transaction(id: u64, apply_token: Binder, changes: &[(Binder, i32, Change)]) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(TRANSACTIONS_DESCRIPTOR);
    p.write_i64(id as i64);
    p.write_i32(0); // flags: synchronous
    p.write_i64(0); // no desired present time
    p.write_bool(true); // its timestamp is automatic
    // `FrameTimelineInfo`: none.
    p.write_i32(1);
    p.write_i32(44);
    p.write_i64(INVALID_VSYNC_ID);
    p.write_i32(0);
    p.write_i64(0);
    p.write_bool(false);
    p.write_i64(INVALID_VSYNC_ID);
    p.write_i64(0);
    p.write_binder(Some(apply_token));
    p.write_bool(false); // may contain no buffer
    p.write_bool(false); // no call points logged
    p.write_i32(0); // no display states
    p.write_i32(changes.len() as i32);
    for (handle, id, change) in changes {
        write_layer_state(&mut p, *handle, *id, change);
    }
    p.write_i32(0); // `InputWindowCommands`: no focus requests, no listeners
    p.write_i32(0);
    p.write_i32(0); // no buffers uncached
    p.write_i32(0); // no transactions merged
    p.write_bool(false); // no listener callbacks
    p.write_i32(0);
    p
}

/// `layer_state_t::write` of `change` to the layer `handle`, `id`: what
/// it does not change keeps `layer_state_t`'s defaults.
fn write_layer_state(p: &mut Parcel, handle: Binder, id: i32, change: &Change) {
    let mut what = 0;
    let (x, y) = change.position.inspect(|_| what |= POSITION_CHANGED).unwrap_or((0.0, 0.0));
    let z = change.z.inspect(|_| what |= LAYER_CHANGED).unwrap_or(0);
    // `Rect::INVALID_RECT`, else the crop; written top, left, bottom, right.
    let [left, top, right, bottom] = change.crop.inspect(|_| what |= CROP_CHANGED).unwrap_or([0.0, 0.0, -1.0, -1.0]);
    p.write_binder(Some(handle));
    p.write_i32(id);
    p.write_i64(what as i64);
    p.write_f32(x);
    p.write_f32(y);
    p.write_i32(z);
    p.write_i32(0); // layer stack
    p.write_i32(0); // flags
    p.write_i32(0); // mask
    [1.0, 0.0, 1.0, 0.0].iter().for_each(|v| p.write_f32(*v)); // matrix: dsdx, dtdx, dtdy, dsdy
    [top, left, bottom, right].iter().for_each(|v| p.write_f32(*v));
    p.write_bool(false); // no relative layer
    p.write_bool(false); // no parent for a child
    (0..4).for_each(|_| p.write_f32(0.0)); // color
    p.write_i32(0); // no window info
    write_empty_region(p); // transparent region
    p.write_i32(0); // buffer transform
    p.write_bool(false); // transform to display inverse
    p.write_i32(0); // dataspace UNKNOWN
    p.write_i32(4); // `HdrMetadata`: no valid types
    p.write_i32(0);
    write_empty_region(p); // surface damage
    p.write_i32(-1); // api
    p.write_bool(false); // no sideband stream
    for row in 0..4 {
        (0..4).for_each(|column| p.write_f32(if row == column { 1.0 } else { 0.0 })); // color transform
    }
    p.write_f32(0.0); // corner radius
    p.write_f32(0.0); // client drawn corner radius
    p.write_i32(0); // background blur radius
    p.write_i32(1); // `LayerMetadata`: empty
    p.write_i32(0);
    (0..4).for_each(|_| p.write_f32(0.0)); // background color
    p.write_i32(0); // its dataspace
    p.write_bool(false); // not color space agnostic
    p.write_i32(0); // no listeners
    p.write_f32(0.0); // shadow radius
    p.write_i32(1); // `BorderSettings`: none
    p.write_i32(12);
    p.write_f32(0.0);
    p.write_i32(0);
    p.write_i32(-1); // frame rate selection priority
    p.write_f32(0.0); // frame rate
    (0..4).for_each(|_| p.write_i32(0)); // its compatibility, strategy, default compatibility, category
    p.write_bool(false); // not only smooth switches
    p.write_i32(0); // selection strategy
    p.write_i32(ROT_INVALID as i32); // fixed transform hint
    p.write_bool(false); // no auto refresh
    p.write_bool(false); // dimming
    p.write_i32(0); // no blur regions
    (0..10).for_each(|_| p.write_f32(0.0)); // `StretchEffect`: none
    p.write_i32(1); // `EdgeExtensionParameters`: none
    p.write_i32(20);
    (0..4).for_each(|_| p.write_bool(false));
    for _ in 0..2 {
        [0, 0, -1, -1].iter().for_each(|v| p.write_i32(*v)); // buffer crop, destination frame
    }
    p.write_i32(0); // trusted overlay UNSET
    p.write_i32(0); // drop input mode NONE
    p.write_bool(false); // no buffer data
    p.write_i32(1); // `TrustedPresentationThresholds`: none
    p.write_i32(16);
    p.write_f32(-1.0);
    p.write_f32(-1.0);
    p.write_i32(0);
    p.write_i32(1); // `TrustedPresentationListener`: none
    p.write_binder(None);
    p.write_i32(-1);
    p.write_f32(1.0); // current HDR/SDR ratio
    p.write_f32(1.0); // desired
    p.write_i32(CACHING_ENABLED);
    p.write_bool(false); // no buffer release channel
    p.write_i64(0); // picture profile NONE (`APPLY_PICTURE_PROFILES` is on)
    p.write_i32(0); // app content priority
    p.write_bool(false); // no LUTs
}

/// An empty `Region` (`LightFlattenable`): its size, one empty rectangle.
fn write_empty_region(p: &mut Parcel) {
    p.write_i32(20);
    p.write_i32(1);
    (0..4).for_each(|_| p.write_i32(0));
}

fn status(r: &mut Reader<'_>, what: &str) -> Result<(), String> {
    match r.read_exception() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("{what}: {e:?}")),
        Err(s) => Err(format!("{what}: {s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `layer_state_t` system_server sent to change a layer's flags
    /// (`setTransactionState`, captured on the pinned image), from the
    /// stability after its surface's binder to its end; the transaction's
    /// fields from its flags to its apply token.
    const ORIGINAL_LAYER_STATE: &str = concat!(
        "0c0000003400000040000000000000000000000000000000000000000000000000000000010000000000803f00000000",
        "0000803f000000000000000000000000000080bf000080bf000000000000000000000000000000000000000000000000",
        "000000001400000001000000000000000000000000000000000000000000000000000000000000000400000000000000",
        "140000000100000000000000000000000000000000000000ffffffff000000000000803f000000000000000000000000",
        "000000000000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f",
        "000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000",
        "00000000010000000c0000000000000000000000ffffffff000000000000000000000000000000000000000000000000",
        "000000008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000100000014000000000000000000000000000000000000000000000000000000ffffffff",
        "ffffffff0000000000000000ffffffffffffffff0000000000000000000000000100000010000000000080bf000080bf",
        "0000000001000000852a6273000000000000000000000000000000000000000000000000ffffffff0000803f0000803f",
        "010000000000000000000000000000000000000000000000",
    );
    const ORIGINAL_TRANSACTION_HEAD: &str = concat!(
        "00000000000000000000000001000000010000002c000000ffffffffffffffff00000000000000000000000000000000",
        "ffffffffffffffff0000000000000000",
    );

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn writes_layer_state_as_the_original() {
        // The original's change: flags (`eFlagsChanged`), mask 1; ours has
        // no flags to change, so the bytes are compared with those set.
        let mut p = Parcel::new();
        write_layer_state(&mut p, Binder::Handle(0x81), 0x34, &Change::default());
        let mut ours = p.data()[24..].to_vec();
        ours[8..16].copy_from_slice(&0x40u64.to_le_bytes());
        ours[36..40].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(hex(&ours), ORIGINAL_LAYER_STATE);
    }

    #[test]
    fn a_transaction_places_layers() {
        let change = Change { position: Some((100.0, 50.0)), z: Some(3), crop: Some([0.0, 0.0, 640.0, 480.0]) };
        let p = transaction(5, Binder::Local(1), &[(Binder::Handle(9), 7, change)]);
        let data = p.data();
        let after_token = 4 * 3 + 4 + 2 * ("android.ui.ISurfaceComposer".len() + 1);
        assert_eq!(&data[after_token..after_token + 8], &5u64.to_le_bytes());
        assert_eq!(hex(&data[after_token + 8..after_token + 8 + 64]), ORIGINAL_TRANSACTION_HEAD);
        let state = after_token + 8 + 64 + 28 + 4 * 4;
        assert_eq!(&data[state - 4..state], &1i32.to_le_bytes());
        let fields = &data[state + 28..];
        let word = |i: usize| <[u8; 4]>::try_from(&fields[4 * i..4 * i + 4]).unwrap();
        assert_eq!(i32::from_le_bytes(word(0)), 7);
        assert_eq!(u64::from_le_bytes(fields[4..12].try_into().unwrap()), POSITION_CHANGED | LAYER_CHANGED | CROP_CHANGED);
        assert_eq!((f32::from_le_bytes(word(3)), f32::from_le_bytes(word(4)), i32::from_le_bytes(word(5))), (100.0, 50.0, 3));
        // The crop: top, left, bottom, right.
        let crop: Vec<f32> = (13..17).map(|i| f32::from_le_bytes(word(i))).collect();
        assert_eq!(crop, [0.0, 0.0, 480.0, 640.0]);
        // The whole state, then the commands, uncache, merges and callbacks.
        let mut one = Parcel::new();
        write_layer_state(&mut one, Binder::Handle(9), 7, &change);
        assert_eq!(data.len(), state + one.data().len() + 4 * 6);
    }
}
