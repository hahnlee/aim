//! The one display's state: its layers, what composition each asked for,
//! and the buffers the host has.
//!
//! In device mode composition is client (GPU) composition: validation
//! turns every layer into `CLIENT`, SurfaceFlinger renders them with
//! RenderEngine into the client target, and a present shows that buffer.
//!
//! In window mode (`docs/layers.md`) each Mac window shows its own task's
//! layers, so the layers stay `DEVICE` (and `SOLID_COLOR`) where the
//! server can draw them, and a present sends the server every layer, in z
//! order, with its buffer and geometry; the client target shows where the
//! `CLIENT` layers are, which validation makes contiguous in z.
//!
//! The display server waits for the acquire fences, so a present does
//! not; it returns the present fence, which signals once the frame is on
//! screen (`docs/composer.md`), and on which each buffer a present
//! replaced is released.
//!
//! The one exception is the cursor: the display's hardware cursor is the
//! Mac's own, so a `CURSOR` layer (the pointer's sprite) stays one and is
//! not drawn; its buffer is the Mac's cursor ([`Cursor`]).

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

use aim_gralloc::Handle;
use aim_hostcall::display::{Layer as Sent, layer};
use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_graphics_composer3::aidl::android::hardware::graphics::composer3::{
    ChangedCompositionLayer::ChangedCompositionLayer,
    Composition::Composition,
    IComposerClient::{EX_BAD_PARAMETER, EX_NO_RESOURCES},
    LayerCommand::LayerCommand,
};

use crate::cursor::Cursor;
use crate::host::Host;

/// The `PixelFormat`s the server maps as textures: RGBA_8888, RGBX_8888,
/// RGB_565, BGRA_8888, RGBA_FP16, RGBA_1010102. A layer of another format
/// is composed by the client.
const DEVICE_FORMATS: [i32; 6] = [0x1, 0x2, 0x4, 0x5, 0x16, 0x2b];

/// A layer's state, kept across frames: SurfaceFlinger sends what changed.
struct Layer {
    composition: Composition,
    /// Buffer id and format per slot (window mode).
    slots: HashMap<i32, (u64, i32)>,
    /// The buffer shown and its format.
    buffer: Option<(u64, i32)>,
    /// Its acquire fence, when it is new since the last present.
    fence: Option<OwnedFd>,
    /// A new buffer since the last present.
    replaced: bool,
    frame: [i32; 4],
    crop: [f32; 4],
    transform: u32,
    alpha: f32,
    blend: u32,
    color: [f32; 4],
    z: i32,
    visible: Vec<[i32; 4]>,
    /// It has a color transform other than the identity.
    color_transform: bool,
}

impl Layer {
    fn new() -> Layer {
        Layer {
            composition: Composition::INVALID,
            slots: HashMap::new(),
            buffer: None,
            fence: None,
            replaced: false,
            frame: [0; 4],
            crop: [0.0; 4],
            transform: 0,
            alpha: 1.0,
            blend: 2,
            color: [0.0, 0.0, 0.0, 1.0],
            z: 0,
            visible: Vec::new(),
            color_transform: false,
        }
    }
}

pub struct Display {
    /// Whether each window shows its task's layers (window mode).
    windows: bool,
    layers: HashMap<i64, Layer>,
    next_layer: i64,
    /// Buffer id per client target slot.
    slots: Vec<Option<u64>>,
    /// Slots (the client target's and the layers') referring to each
    /// buffer the host has.
    imported: HashMap<u64, usize>,
    target: Option<u64>,
    target_fence: Option<OwnedFd>,
    /// The display has a color transform other than the identity.
    color_transform: bool,
    /// The compositions the last validation asked for, applied when
    /// SurfaceFlinger accepts the changes.
    changes: Vec<(i64, Composition)>,
    pub cursor: Cursor,
}

/// A failed command, as a composer3 `EX_*` code.
pub type Error = i32;

/// Whether a color matrix (row-major 4x4) is the identity.
fn is_identity(m: &[f32]) -> bool {
    m.len() == 16
        && m.iter()
            .enumerate()
            .all(|(i, &v)| v == if i % 5 == 0 { 1.0 } else { 0.0 })
}

impl Display {
    pub fn new(windows: bool) -> Display {
        Display {
            windows,
            layers: HashMap::new(),
            next_layer: 1,
            slots: vec![None; 1],
            imported: HashMap::new(),
            target: None,
            target_fence: None,
            color_transform: false,
            changes: Vec::new(),
            cursor: Cursor::default(),
        }
    }

    pub fn create_layer(&mut self) -> i64 {
        let id = self.next_layer;
        self.next_layer += 1;
        self.layers.insert(id, Layer::new());
        id
    }

    pub fn destroy_layer(&mut self, host: &Host, layer: i64) -> bool {
        let Some(l) = self.layers.remove(&layer) else {
            return false;
        };
        for (id, _) in l.slots.into_values() {
            self.unref(host, id);
        }
        true
    }

    pub fn has_layer(&self, layer: i64) -> bool {
        self.layers.contains_key(&layer)
    }

    /// The display's color transform.
    pub fn set_color_transform(&mut self, matrix: &[f32]) {
        self.color_transform = !is_identity(matrix);
    }

    /// A layer's commands. Buffers and geometry matter in window mode
    /// only.
    pub fn command(&mut self, host: &Host, cmd: &LayerCommand) -> Result<(), Error> {
        let windows = self.windows;
        let Some(l) = self.layers.get_mut(&cmd.layer) else {
            return Ok(());
        };
        if let Some(c) = &cmd.composition {
            l.composition = c.composition;
        }
        // The cursor's buffers are the Mac's cursor (`Cursor`).
        if !windows || l.composition == Composition::CURSOR {
            return Ok(());
        }
        if let Some(r) = &cmd.displayFrame {
            l.frame = [r.left, r.top, r.right, r.bottom];
        }
        if let Some(r) = &cmd.sourceCrop {
            l.crop = [r.left, r.top, r.right, r.bottom];
        }
        if let Some(t) = &cmd.transform {
            l.transform = t.transform.0 as u32;
        }
        if let Some(a) = &cmd.planeAlpha {
            l.alpha = a.alpha;
        }
        if let Some(b) = &cmd.blendMode {
            l.blend = b.blendMode.0 as u32;
        }
        if let Some(c) = &cmd.color {
            l.color = [c.r, c.g, c.b, c.a];
        }
        if let Some(z) = &cmd.z {
            l.z = z.z;
        }
        if let Some(rects) = &cmd.visibleRegion {
            l.visible = rects
                .iter()
                .flatten()
                .map(|r| [r.left, r.top, r.right, r.bottom])
                .collect();
        }
        if let Some(m) = &cmd.colorTransform {
            l.color_transform = !is_identity(m);
        }
        let mut dropped: Vec<u64> = cmd
            .bufferSlotsToClear
            .iter()
            .flatten()
            .filter_map(|slot| l.slots.remove(slot).map(|(id, _)| id))
            .collect();
        let mut new = None;
        if let Some(b) = &cmd.buffer {
            if let Some(native) = &b.handle {
                let (Some(fd), Some(h)) = (native.fds.first(), Handle::from_ints(&native.ints))
                else {
                    return Err(EX_BAD_PARAMETER);
                };
                if l.slots.get(&b.slot).map(|s| s.0) != Some(h.id) {
                    dropped.extend(l.slots.insert(b.slot, (h.id, h.format)).map(|(id, _)| id));
                    new = Some((fd.as_ref().as_fd(), h));
                }
            }
            let shown = l.slots.get(&b.slot).copied();
            if shown.map(|s| s.0) != l.buffer.map(|s| s.0) {
                l.replaced = true;
            }
            l.buffer = shown;
            l.fence = b.fence.as_ref().and_then(|f| f.as_ref().try_clone().ok());
        }
        if let Some((fd, h)) = new {
            self.import(host, fd, &h)?;
        }
        for id in dropped {
            self.unref(host, id);
        }
        Ok(())
    }

    /// Give the host buffer `h` unless it has it, and count one more slot
    /// referring to it.
    fn import(
        &mut self,
        host: &Host,
        fd: std::os::fd::BorrowedFd,
        h: &Handle,
    ) -> Result<(), Error> {
        if !self.imported.contains_key(&h.id) {
            host.import(fd, h).map_err(|e| {
                log::error!("buffer {:#x}: import failed: {e:?}", h.id);
                EX_NO_RESOURCES
            })?;
        }
        *self.imported.entry(h.id).or_default() += 1;
        Ok(())
    }

    /// One slot less refers to buffer `id`; the host forgets it with the
    /// last.
    fn unref(&mut self, host: &Host, id: u64) {
        if let Some(refs) = self.imported.get_mut(&id) {
            *refs -= 1;
            if *refs == 0 {
                self.imported.remove(&id);
                if self.target == Some(id) {
                    self.target = None;
                }
                let _ = host.release(id);
            }
        }
    }

    /// Resize the client target slot table, dropping buffers of removed
    /// slots.
    pub fn set_slot_count(&mut self, host: &Host, count: usize) {
        for slot in count..self.slots.len() {
            if let Some(id) = self.slots[slot].take() {
                self.unref(host, id);
            }
        }
        self.slots.resize(count.max(1), None);
    }

    /// `setClientTarget`: a new buffer for `slot`, or the one cached there.
    pub fn set_client_target(
        &mut self,
        host: &Host,
        slot: i32,
        handle: Option<&NativeHandle>,
        fence: Option<OwnedFd>,
    ) -> Result<(), Error> {
        let slot = usize::try_from(slot).map_err(|_| EX_BAD_PARAMETER)?;
        if slot >= self.slots.len() {
            return Err(EX_BAD_PARAMETER);
        }
        if let Some(native) = handle {
            let (Some(fd), Some(h)) = (native.fds.first(), Handle::from_ints(&native.ints)) else {
                return Err(EX_BAD_PARAMETER);
            };
            if self.slots[slot] != Some(h.id) {
                self.import(host, fd.as_ref().as_fd(), &h)?;
                if let Some(old) = self.slots[slot].replace(h.id) {
                    self.unref(host, old);
                }
            }
        }
        self.target = self.slots[slot];
        self.target_fence = fence;
        Ok(())
    }

    /// The composition the server can give `l`.
    fn choose(&self, l: &Layer) -> Composition {
        let c = l.composition;
        if c == Composition::CURSOR {
            return c;
        }
        if !self.windows || self.color_transform || l.color_transform {
            return Composition::CLIENT;
        }
        let drawable = match c {
            Composition::DEVICE => l
                .buffer
                .is_some_and(|(_, format)| DEVICE_FORMATS.contains(&format)),
            Composition::SOLID_COLOR => true,
            _ => false,
        };
        if drawable { c } else { Composition::CLIENT }
    }

    /// The composition changes SurfaceFlinger must accept: in device mode
    /// every layer but the cursor is composed by the client; in window
    /// mode what the server cannot draw, and every layer between two
    /// `CLIENT` layers, so the client target has one place in z.
    pub fn validate(&mut self) -> Vec<ChangedCompositionLayer> {
        let mut chosen: Vec<(i64, i32, Composition)> = self
            .layers
            .iter()
            .map(|(&id, l)| (id, l.z, self.choose(l)))
            .collect();
        let client = chosen
            .iter()
            .filter(|(_, _, c)| *c == Composition::CLIENT)
            .map(|&(_, z, _)| z);
        if let (Some(lo), Some(hi)) = (client.clone().min(), client.max()) {
            for (_, z, c) in &mut chosen {
                if *c != Composition::CURSOR && (lo..=hi).contains(z) {
                    *c = Composition::CLIENT;
                }
            }
        }
        self.changes = chosen
            .into_iter()
            .filter(|(id, _, c)| self.layers[id].composition != *c)
            .map(|(id, _, c)| (id, c))
            .collect();
        self.changes.sort_by_key(|&(id, _)| id);
        self.changes
            .iter()
            .map(|&(layer, composition)| ChangedCompositionLayer { layer, composition })
            .collect()
    }

    pub fn accept_changes(&mut self) {
        for (id, c) in std::mem::take(&mut self.changes) {
            if let Some(l) = self.layers.get_mut(&id) {
                l.composition = c;
            }
        }
    }

    /// Show the frame once its rendering is done: the present fence, and
    /// the layers whose buffers it replaced (released on that fence).
    pub fn present(&mut self, host: &Host) -> Option<(OwnedFd, Vec<i64>)> {
        if self.windows {
            return self.present_layers(host);
        }
        let fence = self.target_fence.take();
        let id = self.target?;
        host.present(id, fence.as_ref().map(|f| f.as_fd()))
            .inspect_err(|e| log::error!("present {id:#x}: {e:?}"))
            .ok()
            .map(|f| (f, Vec::new()))
    }

    /// Window mode's present: every layer but the cursor, bottom first.
    fn present_layers(&mut self, host: &Host) -> Option<(OwnedFd, Vec<i64>)> {
        let mut order: Vec<(&i64, &mut Layer)> = self
            .layers
            .iter_mut()
            .filter(|(_, l)| l.composition != Composition::CURSOR)
            .collect();
        order.sort_by_key(|(_, l)| l.z);
        let mut sent = Vec::with_capacity(order.len());
        let mut rects = Vec::new();
        let mut fences = Vec::new();
        let mut replaced = Vec::new();
        let mut client = false;
        for (&id, l) in order {
            let kind = match l.composition {
                Composition::DEVICE => layer::BUFFER,
                Composition::SOLID_COLOR => layer::COLOR,
                _ => layer::CLIENT,
            };
            client |= kind == layer::CLIENT;
            let fence = l.fence.take();
            if std::mem::take(&mut l.replaced) {
                replaced.push(id);
            }
            sent.push(Sent {
                id: id as u64,
                buffer: if kind == layer::BUFFER {
                    l.buffer.map_or(0, |b| b.0)
                } else {
                    0
                },
                kind,
                transform: l.transform,
                frame: l.frame,
                crop: l.crop,
                color: l.color,
                alpha: l.alpha,
                blend: l.blend,
                visible_first: rects.len() as u32,
                visible_count: l.visible.len() as u32,
                acquire: fence.as_ref().map_or(-1, |f| f.as_raw_fd()),
                ..Default::default()
            });
            rects.extend_from_slice(&l.visible);
            fences.extend(fence);
        }
        let target = if client { self.target.unwrap_or(0) } else { 0 };
        let target_fence = self.target_fence.take();
        let r = host.layers(
            &sent,
            &rects,
            target,
            target_fence.as_ref().map(|f| f.as_fd()),
        );
        drop(fences);
        r.inspect_err(|e| log::error!("present of {} layers: {e:?}", sent.len()))
            .ok()
            .map(|f| (f, replaced))
    }

    /// Forget everything, as for a new client.
    pub fn reset(&mut self, host: &Host) {
        self.cursor.clear(host);
        for id in self.imported.keys() {
            let _ = host.release(*id);
        }
        *self = Display::new(self.windows);
    }
}
