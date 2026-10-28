//! SurfaceControl.Transaction's native payload at android-16.0.0_r1:
//! libs/gui/{SurfaceComposerClient,TransactionState,LayerState}.cpp.
//! The state is forwarded losslessly after checking its field boundaries;
//! SurfaceFlinger owns the values, buffers, fences and input commands.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_driver::{
    File,
    uapi::{BINDER_TYPE_FD, FlatBinderObject},
};
use aim_binder_host::local::LocalProcess;
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader, Result as ParcelResult};

use super::TRANSACTIONS_DESCRIPTOR;

pub(in crate::app_host) struct DrawTransaction {
    id: i64,
    flags: u32,
    timeline: Parcel,
    displays: Parcel,
    layers: Vec<LayerState>,
    commands: Parcel,
    merged: Vec<i64>,
    has_callbacks: bool,
    callbacks: Vec<Parcel>,
}

#[derive(Debug)]
pub(in crate::app_host) enum Error {
    Parcel(i32),
    Unsupported(&'static str),
}

impl From<i32> for Error {
    fn from(status: i32) -> Self {
        Self::Parcel(status)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parcel(status) => write!(f, "invalid Transaction parcel: {status}"),
            Self::Unsupported(field) => write!(f, "unsupported Transaction field: {field} (#1431)"),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

impl DrawTransaction {
    pub(super) fn one_way(&self) -> bool {
        self.flags & 0x20 != 0
    }

    /// After AIDL's typed-object marker. Java writes a second marker for
    /// a live native Transaction; a closed Transaction contains only 0.
    pub(in crate::app_host) fn read(
        r: &mut Reader<'_>,
        process: &LocalProcess,
    ) -> Result<Option<Self>> {
        Self::read_with_files(r, &|fd| process.file(fd))
    }

    fn read_with_files(
        r: &mut Reader<'_>,
        file: &impl Fn(u32) -> Option<File>,
    ) -> Result<Option<Self>> {
        if !r.read_bool()? {
            return Ok(None);
        }
        let id = r.read_i64()?;
        let flags = r.read_u32()?;
        r.read_i64()?; // merge does not inherit desired present time
        r.read_bool()?; // nor isAutoTimestamp
        let start = r.position();
        sized_parcelable(r)?; // FrameTimelineInfo
        let timeline = copy(r, start, file)?;
        r.read_binder()?; // merge deliberately does not inherit mApplyToken
        let start = r.position();
        r.read_bool()?; // may contain buffer
        r.read_bool()?; // log call points
        for _ in 0..count(r, 112)? {
            r.read_binder()?; // display token
            r.read_binder()?; // IGraphicBufferProducer
            r.skip(56)?;
        }
        let displays = copy(r, start, file)?;
        let mut layers = Vec::new();
        for _ in 0..count(r, 4)? {
            layers.push(layer(r, file)?);
        }
        let start = r.position();
        for _ in 0..count(r, 4)? {
            sized_parcelable(r)?; // gui::FocusRequest
        }
        for _ in 0..count(r, 28)? {
            r.read_binder()?; // IWindowInfosReportedListener
        }
        for _ in 0..count(r, 36)? {
            r.read_binder()?; // uncache buffer's token
            r.read_i64()?;
        }
        let commands = copy(r, start, file)?;
        let mut merged = Vec::new();
        let merged_count = count(r, 8)?;
        if merged_count > 10 {
            return Err(Error::Unsupported(
                "merge history exceeding the pinned limit",
            ));
        }
        for _ in 0..merged_count {
            merged.push(r.read_i64()?);
        }
        let mut has_callbacks = r.read_bool()?;
        let mut state_callbacks = Vec::new();
        for _ in 0..count(r, 32)? {
            let start = r.position();
            r.read_binder()?;
            callbacks(r)?;
            state_callbacks.push(copy(r, start, file)?);
        }
        // Transaction::apply distributes the separate client callback map
        // into the corresponding layer states (or transaction listeners).
        let map_size = count(r, 32)?;
        has_callbacks |= map_size != 0;
        for _ in 0..map_size {
            let start = r.position();
            r.read_binder()?;
            let ids = count(r, 4)?;
            for _ in 0..ids {
                callback_id(r)?;
            }
            let listener = copy(r, start, file)?;
            let surfaces = count(r, 4)?;
            for _ in 0..surfaces {
                let (handle, id) = surface_control_body(r)?;
                if ids == 0 {
                    continue;
                }
                let index = match layers.iter().position(|l| l.handle == handle) {
                    Some(i) => i,
                    None => {
                        let mut state = Parcel::new();
                        super::write_layer_state(&mut state, handle, id, &super::Change::default());
                        layers.push(layer(&mut state.reader(), &|_| None)?);
                        layers.len() - 1
                    }
                };
                layers[index].add_listener(&listener);
            }
            if ids != 0 && surfaces == 0 {
                state_callbacks.push(listener);
            }
        }
        Ok(Some(Self {
            id,
            flags,
            timeline,
            displays,
            layers,
            commands,
            merged,
            has_callbacks,
            callbacks: state_callbacks,
        }))
    }

    pub(super) fn transaction(&self, id: u64, apply_token: Binder) -> Parcel {
        let mut p = Parcel::new();
        p.write_interface_token(TRANSACTIONS_DESCRIPTOR);
        p.write_i64(id as i64);
        // Transaction::apply normalizes mutually cancelling early wakeup bits.
        let wakeup = 0x08 | 0x10;
        p.write_u32(if self.flags & wakeup == wakeup {
            self.flags & !wakeup
        } else {
            self.flags
        });
        p.write_i64(0);
        p.write_bool(true);
        append(&mut p, &self.timeline);
        p.write_binder(Some(apply_token));
        append(&mut p, &self.displays);
        p.write_i32(self.layers.len() as i32);
        for layer in &self.layers {
            append(&mut p, &layer.state);
        }
        append(&mut p, &self.commands);
        // TransactionState::merge keeps at most MAX_MERGE_HISTORY_LENGTH
        // (10) ids, prepending the transaction just merged.
        let history = self.merged.len().min(9);
        p.write_i32((history + 1) as i32);
        p.write_i64(self.id);
        for id in &self.merged[..history] {
            p.write_i64(*id);
        }
        p.write_bool(self.has_callbacks);
        p.write_i32(self.callbacks.len() as i32);
        for listener in &self.callbacks {
            append(&mut p, listener);
        }
        p
    }
}

fn append(to: &mut Parcel, from: &Parcel) {
    to.write_raw_files(from.data(), from.objects(), from.files());
}

fn copy(r: &Reader<'_>, start: usize, file: &impl Fn(u32) -> Option<File>) -> Result<Parcel> {
    let (bytes, objects) = r.since(start);
    let mut files = Vec::new();
    for &at in &objects {
        let offset = usize::try_from(at).map_err(|_| BAD_VALUE)?;
        let raw = bytes
            .get(offset..offset.checked_add(24).ok_or(BAD_VALUE)?)
            .ok_or(BAD_VALUE)?;
        let object = FlatBinderObject::decode(raw);
        if object.kind == BINDER_TYPE_FD {
            let fd = u32::try_from(object.binder).map_err(|_| BAD_VALUE)?;
            files.push((at, file(fd).ok_or(BAD_VALUE)?));
        } else {
            // Validate recorded binder objects, including those nested in
            // sized AIDL parcelables. Preserve their original stability.
            let mut object_reader = Reader::new(bytes, &objects);
            object_reader.set_position(offset);
            object_reader.read_binder()?;
        }
    }
    let mut p = Parcel::new();
    p.write_raw_files(bytes, &objects, &files);
    Ok(p)
}

fn count(r: &mut Reader<'_>, min_bytes: usize) -> ParcelResult<usize> {
    let n = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
    if n > r.remaining() / min_bytes {
        return Err(BAD_VALUE);
    }
    Ok(n)
}

/// C++ Parcel::writeParcelable adds a presence word. Generated AIDL
/// parcelables then include a size starting at the size word itself.
fn sized_parcelable(r: &mut Reader<'_>) -> ParcelResult<()> {
    if r.read_bool()? {
        let size = r.read_i32()?;
        if size < 4 || size % 4 != 0 {
            return Err(BAD_VALUE);
        }
        r.skip(size as usize - 4)?;
    }
    Ok(())
}

/// LightFlattenable's byte length and padded payload (Region, HdrMetadata).
fn light(r: &mut Reader<'_>) -> ParcelResult<()> {
    let n = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
    r.skip(n)
}

/// Flattenable's byte length, fd count, payload, then Binder fd objects
/// (GraphicBuffer and Fence). No host graphics import is needed here.
fn flattened(r: &mut Reader<'_>) -> ParcelResult<()> {
    let n = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
    let fds = count(r, 24)?;
    r.skip(n)?;
    for _ in 0..fds {
        r.read_fd()?;
    }
    Ok(())
}

fn callback_id(r: &mut Reader<'_>) -> ParcelResult<()> {
    if r.read_bool()? {
        r.skip(12)?;
    } // custom CallbackId: uint64 id, int32 type
    Ok(())
}

fn callbacks(r: &mut Reader<'_>) -> ParcelResult<()> {
    for _ in 0..count(r, 4)? {
        callback_id(r)?;
    }
    Ok(())
}

fn listeners(r: &mut Reader<'_>) -> ParcelResult<()> {
    for _ in 0..count(r, 32)? {
        r.read_binder()?;
        callbacks(r)?;
    }
    Ok(())
}

fn surface_control_body(r: &mut Reader<'_>) -> ParcelResult<(Binder, i32)> {
    r.read_binder()?; // client
    let handle = r.read_binder()?.ok_or(BAD_VALUE)?;
    let id = r.read_i32()?;
    r.read_string16()?;
    r.skip(16)?; // transform hint, width, height, format
    Ok((handle, id))
}

fn surface_control(r: &mut Reader<'_>) -> ParcelResult<()> {
    if r.read_bool()? {
        surface_control_body(r)?;
    }
    Ok(())
}

struct LayerState {
    handle: Binder,
    state: Parcel,
    listeners: usize,
    listeners_end: usize,
}

/// Append a byte range while rebasing all Binder and owned fd objects.
fn range(to: &mut Parcel, from: &Parcel, start: usize, end: usize) {
    let mut r = from.reader();
    r.set_position(end);
    let (bytes, objects) = r.since(start);
    let files: Vec<_> = from
        .files()
        .iter()
        .filter(|(at, _)| (*at as usize) >= start && (*at as usize) < end)
        .map(|(at, f)| (*at - start as u64, f.clone()))
        .collect();
    to.write_raw_files(bytes, &objects, &files);
}

impl LayerState {
    fn add_listener(&mut self, listener: &Parcel) {
        let mut p = Parcel::new();
        range(&mut p, &self.state, 0, self.listeners);
        let mut r = self.state.reader();
        r.set_position(self.listeners);
        let count = r.read_i32().unwrap(); // validated by layer()
        p.write_i32(count + 1);
        range(&mut p, &self.state, self.listeners + 4, self.listeners_end);
        append(&mut p, listener);
        let added = p.position() - self.listeners_end;
        range(
            &mut p,
            &self.state,
            self.listeners_end,
            self.state.position(),
        );
        // surface binder (28), layer id (4), then uint64 what.
        let what = u32::from_le_bytes(p.data()[32..36].try_into().unwrap());
        p.set_i32_at(32, (what | 0x2000_0000) as i32);
        self.listeners_end += added;
        self.state = p;
    }
}

fn buffer(r: &mut Reader<'_>) -> ParcelResult<()> {
    if r.read_bool()? {
        // writeParcelable<BufferData>
        r.read_i32()?; // BufferDataChange flags
        if r.read_bool()? {
            flattened(r)?;
        }
        if r.read_bool()? {
            flattened(r)?;
        }
        r.read_i64()?; // frame number
        r.read_binder()?; // release listener
        r.read_binder()?; // release endpoint
        r.read_binder()?; // cached buffer token
        r.read_i64()?; // cached buffer id
        r.read_bool()?; // has barrier
        r.read_i64()?; // barrier frame number
        r.read_i32()?; // producer id
        r.read_i64()?; // dequeue time
    }
    Ok(())
}

fn layer(r: &mut Reader<'_>, file: &impl Fn(u32) -> Option<File>) -> Result<LayerState> {
    let start = r.position();
    let handle = r.read_binder()?.ok_or(BAD_VALUE)?;
    r.skip(68)?; // id, what, position, z, stack, flags, mask, matrix, crop
    surface_control(r)?; // relative layer
    surface_control(r)?; // parent for child
    r.skip(16)?; // color
    if r.read_bool()? {
        return Err(Error::Unsupported("WindowInfo"));
    }
    light(r)?; // transparent region
    r.skip(12)?; // buffer transform, inverse, dataspace
    light(r)?; // HDR metadata
    light(r)?; // damage
    r.read_i32()?; // API
    if r.read_bool()? {
        let fds = count(r, 24)?;
        let ints = count(r, 4)?;
        for _ in 0..fds {
            r.read_fd()?;
        }
        r.skip(ints * 4)?;
    }
    r.skip(76)?; // color transform, corner radii, blur radius
    if r.read_bool()? {
        // LayerMetadata (custom Parcelable, no size)
        for _ in 0..count(r, 8)? {
            r.read_i32()?;
            light(r)?; // byte vector
        }
    }
    r.skip(24)?; // background color, dataspace, color space agnostic
    let listeners_start = r.position() - start;
    listeners(r)?;
    let listeners_end = r.position() - start;
    r.read_f32()?; // shadow radius
    sized_parcelable(r)?; // BorderSettings
    r.skip(44)?; // frame rate fields, fixed transform, auto refresh, dimming
    let regions = count(r, 40)?;
    r.skip(regions * 40)?;
    r.skip(40)?; // StretchEffect
    sized_parcelable(r)?; // EdgeExtensionParameters
    r.skip(40)?; // buffer crop, destination frame, trusted overlay, drop input
    if r.read_bool()? {
        buffer(r)?;
    }
    sized_parcelable(r)?; // TrustedPresentationThresholds
    if r.read_bool()? {
        // TrustedPresentationListener (custom Parcelable)
        r.read_binder()?;
        r.read_i32()?;
    }
    r.skip(12)?; // HDR/SDR ratios, caching hint
    if r.read_bool()? && r.read_bool()? {
        // BufferReleaseChannel::ProducerEndpoint
        r.read_string16()?;
        r.read_fd()?;
    }
    r.skip(12)?; // picture profile handle and app content priority (enabled in pin)
    if r.read_bool()? {
        return Err(Error::Unsupported("DisplayLuts"));
    }
    Ok(LayerState {
        handle,
        state: copy(r, start, file)?,
        listeners: listeners_start,
        listeners_end,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Change, write_layer_state};
    use super::*;

    fn original_layer() -> Parcel {
        let hex = super::super::tests::ORIGINAL_LAYER_STATE;
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let mut p = Parcel::new();
        p.write_binder(Some(Binder::Local(2)));
        // The captured fixture starts with the surface binder's stability.
        p.write_raw(&bytes[4..], &[]);
        p
    }

    fn decode(p: &Parcel) -> DrawTransaction {
        DrawTransaction::read_with_files(&mut p.reader(), &|fd| {
            p.files().iter().find_map(|(at, file)| {
                let object = FlatBinderObject::decode(&p.data()[*at as usize..*at as usize + 24]);
                (object.binder as u32 == fd).then(|| file.clone())
            })
        })
        .unwrap()
        .unwrap()
    }

    fn native(layer: &Parcel, map: &Parcel) -> Parcel {
        let base = decode(&super::super::draw_for_test());
        let mut p = Parcel::new();
        p.write_bool(true);
        p.write_i64(123);
        p.write_u32(0x3a); // animation, early wakeup start/end, one way
        p.write_i64(987);
        p.write_bool(false);
        append(&mut p, &base.timeline);
        p.write_binder(Some(Binder::Local(4)));
        p.write_bool(true);
        p.write_bool(true);
        p.write_i32(1); // display
        p.write_binder(Some(Binder::Local(5)));
        p.write_binder(Some(Binder::Local(6)));
        for v in 0..14 {
            p.write_i32(v);
        }
        p.write_i32(1);
        append(&mut p, layer);
        p.write_i32(1); // FocusRequest: sized AIDL parcelable
        p.write_i32(1);
        p.write_i32(52);
        p.write_binder(Some(Binder::Local(7)));
        p.write_string16(Some(""));
        p.write_i64(9);
        p.write_i32(0);
        p.write_i32(1); // window-info listener
        p.write_binder(Some(Binder::Local(8)));
        p.write_i32(1); // uncache
        p.write_binder(Some(Binder::Local(9)));
        p.write_i64(0x12345678);
        p.write_i32(2); // merged IDs
        p.write_i64(121);
        p.write_i64(122);
        p.write_bool(false);
        p.write_i32(0); // state callbacks
        append(&mut p, map);
        p
    }

    fn no_map() -> Parcel {
        let mut p = Parcel::new();
        p.write_i32(0);
        p
    }

    #[test]
    fn captured_original_layer_and_transaction_fields_round_trip() {
        let layer = original_layer();
        let p = native(&layer, &no_map());
        let draw = decode(&p);
        assert_eq!(draw.layers[0].state.data(), layer.data());
        assert_eq!(draw.layers[0].state.objects(), layer.objects());
        let applied = draw.transaction(456, Binder::Local(10));
        let mut r = applied.reader();
        r.enforce_interface(TRANSACTIONS_DESCRIPTOR).unwrap();
        assert_eq!(r.read_i64(), Ok(456));
        assert_eq!(r.read_u32(), Ok(0x22)); // only early wakeups cancel
        assert_eq!(r.read_i64(), Ok(0)); // receiver's timestamp
        assert_eq!(r.read_bool(), Ok(true));
        sized_parcelable(&mut r).unwrap();
        assert_eq!(r.read_binder(), Ok(Some(Binder::Local(10))));
        let start = r.position();
        r.skip(draw.displays.position()).unwrap();
        assert_eq!(r.since(start).0, draw.displays.data());
        assert_eq!(r.read_i32(), Ok(1));
        let start = r.position();
        r.skip(layer.position()).unwrap();
        assert_eq!(r.since(start).0, layer.data());
        let start = r.position();
        r.skip(draw.commands.position()).unwrap();
        assert_eq!(r.since(start).0, draw.commands.data());
        assert_eq!(r.read_i32(), Ok(3));
        for id in [123, 121, 122] {
            assert_eq!(r.read_i64(), Ok(id));
        }
        assert_eq!(r.read_bool(), Ok(false));
        assert_eq!(r.read_i32(), Ok(0));
        assert_eq!(r.remaining(), 0);
        assert_eq!(applied.objects().len(), 7);
    }

    fn callback_map(ids: bool, surfaces: &[Binder]) -> Parcel {
        let mut p = Parcel::new();
        p.write_i32(1);
        p.write_binder(Some(Binder::Local(11)));
        p.write_i32(i32::from(ids));
        if ids {
            p.write_i32(1); // CallbackId
            p.write_i64(55);
            p.write_i32(1); // ON_COMMIT
        }
        p.write_i32(surfaces.len() as i32);
        for surface in surfaces {
            p.write_binder(Some(Binder::Local(12)));
            p.write_binder(Some(*surface));
            p.write_i32(13);
            p.write_string16(Some("callback surface"));
            for v in 0..4 {
                p.write_i32(v);
            }
        }
        p
    }

    #[test]
    fn client_callback_map_is_distributed_as_native_apply() {
        let original = original_layer();
        for surfaces in [&[][..], &[Binder::Local(2)][..], &[Binder::Local(14)][..]] {
            let draw = decode(&native(&original, &callback_map(true, surfaces)));
            assert!(draw.has_callbacks);
            if surfaces.is_empty() {
                assert_eq!(draw.callbacks.len(), 1);
                assert_eq!(draw.layers[0].state.data(), original.data());
            } else {
                assert!(draw.callbacks.is_empty());
                let layer = draw
                    .layers
                    .iter()
                    .find(|l| l.handle == surfaces[0])
                    .unwrap();
                let mut r = layer.state.reader();
                r.set_position(32);
                assert_ne!(r.read_i64().unwrap() & 0x2000_0000, 0);
                r.set_position(layer.listeners);
                assert_eq!(r.read_i32(), Ok(1));
                assert_eq!(r.read_binder(), Ok(Some(Binder::Local(11))));
                assert_eq!(r.read_i32(), Ok(1));
                callback_id(&mut r).unwrap();
                assert_eq!(r.position(), layer.listeners_end);
                assert_eq!(
                    draw.layers.len(),
                    if surfaces[0] == Binder::Local(2) {
                        1
                    } else {
                        2
                    }
                );
            }
        }
        let draw = decode(&native(
            &original,
            &callback_map(false, &[Binder::Local(2)]),
        ));
        assert!(draw.has_callbacks);
        assert!(draw.callbacks.is_empty());
        assert_eq!(draw.layers[0].state.data(), original.data());
    }

    #[test]
    fn buffers_fences_cache_and_barriers_retain_owned_fd_objects() {
        use std::os::fd::AsFd;
        let os_file = std::fs::File::open("/dev/null").unwrap();
        let file = aim_binder_host::server::file_from_fd(os_file.as_fd()).unwrap();
        let original = original_layer();
        let buffer_offset = original.position() - 92;
        assert_eq!(
            &original.data()[buffer_offset..buffer_offset + 4],
            &0i32.to_le_bytes()
        );
        let mut layer = Parcel::new();
        range(&mut layer, &original, 0, buffer_offset);
        layer.write_bool(true); // buffer data
        layer.write_bool(true); // Parcelable<BufferData>
        layer.write_i32(0x07); // all BufferData changes
        for length in [16, 4] {
            // flattened GraphicBuffer and Fence
            layer.write_bool(true);
            layer.write_i32(length);
            layer.write_i32(1); // fd count
            for _ in 0..length / 4 {
                layer.write_i32(0);
            }
            layer.write_file(file.clone());
        }
        layer.write_i64(17); // frame number
        for token in [18, 19, 20] {
            layer.write_binder(Some(Binder::Local(token)));
        }
        layer.write_i64(21); // cache id
        layer.write_bool(true);
        layer.write_i64(22); // barrier frame
        layer.write_i32(23); // producer
        layer.write_i64(24); // dequeue time
        range(
            &mut layer,
            &original,
            buffer_offset + 4,
            original.position(),
        );
        layer.set_i32_at(32, 0x0020_0000); // eBufferChanged
        let p = native(&layer, &no_map());
        let draw = decode(&p);
        assert_eq!(draw.layers[0].state.data(), layer.data());
        let applied = draw.transaction(25, Binder::Local(26));
        assert_eq!(applied.files().len(), 2);
        for (at, _) in applied.files() {
            let mut r = applied.reader();
            r.set_position(*at as usize);
            assert_eq!(r.read_fd(), Ok(0));
        }
        // Failed FD resolution is a hard parcel error, never a dropped fence.
        assert!(DrawTransaction::read_with_files(&mut p.reader(), &|_| None).is_err());
    }

    #[test]
    fn null_closed_truncated_and_unsupported_parcels_are_distinct() {
        let p = native(&original_layer(), &no_map());
        for end in (0..p.position()).step_by(4) {
            let objects: Vec<_> = p
                .objects()
                .iter()
                .copied()
                .filter(|at| *at < end as u64)
                .collect();
            assert!(
                DrawTransaction::read_with_files(
                    &mut Reader::new(&p.data()[..end], &objects),
                    &|_| None
                )
                .is_err(),
                "truncated at {end}"
            );
        }
        let mut closed = Parcel::new();
        closed.write_i32(0);
        assert!(
            DrawTransaction::read_with_files(&mut closed.reader(), &|_| None)
                .unwrap()
                .is_none()
        );
        let mut layer = original_layer();
        // WindowInfo follows binder, the 68-byte geometry, nullable SCs, color.
        layer.set_i32_at(28 + 68 + 8 + 16, 1);
        let unsupported = native(&layer, &no_map());
        assert!(matches!(
            DrawTransaction::read_with_files(&mut unsupported.reader(), &|_| None),
            Err(Error::Unsupported("WindowInfo"))
        ));
        let mut layer = original_layer();
        layer.set_i32_at(layer.position() - 4, 1);
        let unsupported = native(&layer, &no_map());
        assert!(matches!(
            DrawTransaction::read_with_files(&mut unsupported.reader(), &|_| None),
            Err(Error::Unsupported("DisplayLuts"))
        ));
        let mut layer = Parcel::new();
        write_layer_state(&mut layer, Binder::Local(2), 7, &Change::default());
        assert!(super::layer(&mut layer.reader(), &|_| None).is_ok());
    }
}
