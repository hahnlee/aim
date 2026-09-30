//! The cursor layer: the display's hardware cursor (`docs/composer.md`).
//!
//! A `CURSOR` layer (the mouse pointer's sprite) is not drawn into the
//! frame. Its buffers go to the display server like the client target's,
//! and each new buffer and position is sent on, which the server shows as
//! the Mac's cursor. SurfaceFlinger sends a buffer's handle once per slot
//! and later only the slot, as for any layer.

use std::collections::HashMap;
use std::os::fd::AsFd;

use aim_gralloc::Handle;
use android_hardware_graphics_composer3::aidl::android::hardware::graphics::composer3::{
    Composition::Composition, LayerCommand::LayerCommand,
};

use crate::host::Host;

#[derive(Default)]
pub struct Cursor {
    layer: Option<i64>,
    /// The layer's slots: the buffer ids the host has.
    slots: HashMap<i32, u64>,
    /// The buffer shown (0: none) and its top left corner.
    id: u64,
    at: (i32, i32),
}

impl Cursor {
    /// A layer's state: the cursor layer's buffer and position go to the
    /// host.
    pub fn command(&mut self, host: &Host, cmd: &LayerCommand) {
        match &cmd.composition {
            Some(c) if c.composition == Composition::CURSOR && self.layer != Some(cmd.layer) => {
                self.clear(host);
                self.layer = Some(cmd.layer);
            }
            Some(c) if c.composition != Composition::CURSOR && self.layer == Some(cmd.layer) => {
                self.clear(host);
            }
            _ => {}
        }
        if self.layer != Some(cmd.layer) {
            return;
        }
        for &slot in cmd.bufferSlotsToClear.iter().flatten() {
            self.drop_slot(host, slot);
        }
        let mut changed = false;
        if let Some(b) = &cmd.buffer {
            if let Some(native) = &b.handle {
                let (Some(fd), Some(h)) = (native.fds.first(), Handle::from_ints(&native.ints))
                else {
                    log::error!("cursor buffer without a gralloc handle");
                    return;
                };
                if self.slots.get(&b.slot) != Some(&h.id) {
                    self.drop_slot(host, b.slot);
                    if !self.slots.values().any(|&id| id == h.id)
                        && let Err(e) = host.import(fd.as_ref().as_fd(), &h)
                    {
                        log::error!("cursor buffer {:#x}: import failed: {e:?}", h.id);
                        return;
                    }
                    self.slots.insert(b.slot, h.id);
                }
            }
            if let Some(&id) = self.slots.get(&b.slot) {
                self.id = id;
                changed = true;
            }
        }
        let moved = cmd.displayFrame.as_ref().map(|r| (r.left, r.top));
        let at = cmd.cursorPosition.as_ref().map(|p| (p.x, p.y)).or(moved);
        if let Some(at) = at {
            self.at = at;
        }
        if self.id != 0 && (changed || at.is_some()) {
            let fence = cmd.buffer.as_ref().and_then(|b| b.fence.as_ref());
            let fence = fence.filter(|_| changed).map(|f| f.as_ref().as_fd());
            if let Err(e) = host.cursor(self.id, self.at, changed, fence) {
                log::error!("cursor: {e:?}");
            }
        }
    }

    /// Layer `layer` was destroyed.
    pub fn destroyed(&mut self, host: &Host, layer: i64) {
        if self.layer == Some(layer) {
            self.clear(host);
        }
    }

    /// No cursor layer: the host shows none and forgets its buffers.
    pub fn clear(&mut self, host: &Host) {
        if self.id != 0 {
            let _ = host.cursor(0, (0, 0), false, None);
        }
        let slots: Vec<i32> = self.slots.keys().copied().collect();
        for slot in slots {
            self.drop_slot(host, slot);
        }
        *self = Cursor::default();
    }

    fn drop_slot(&mut self, host: &Host, slot: i32) {
        if let Some(id) = self.slots.remove(&slot)
            && !self.slots.values().any(|&i| i == id)
        {
            let _ = host.release(id);
        }
    }
}
