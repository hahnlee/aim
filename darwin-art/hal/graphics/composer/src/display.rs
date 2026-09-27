//! The one display's state: its layers, what composition each asked for,
//! and the client target buffers the host has.
//!
//! Composition is all client (GPU) composition: validation turns every
//! layer into `CLIENT`, SurfaceFlinger renders them with RenderEngine into
//! the client target, and a present shows that buffer. There are no sync
//! fences yet, so a present fence is never returned and SurfaceFlinger
//! treats the frame as shown when present returns (`docs/composer.md`).

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_graphics_composer3::aidl::android::hardware::graphics::composer3::{
    ChangedCompositionLayer::ChangedCompositionLayer,
    Composition::Composition,
    IComposerClient::{EX_BAD_PARAMETER, EX_NO_RESOURCES},
};
use darwin_gralloc::Handle;

use crate::host::Host;

/// How long a present waits for the client target's acquire fence.
const FENCE_TIMEOUT_MS: i32 = 3000;

pub struct Display {
    /// Requested composition per layer.
    layers: HashMap<i64, Composition>,
    next_layer: i64,
    /// Buffer id per client target slot.
    slots: Vec<Option<u64>>,
    /// Slots referring to each buffer the host has.
    imported: HashMap<u64, usize>,
    target: Option<u64>,
    target_fence: Option<OwnedFd>,
    /// Layers the last validation made `CLIENT`, applied when
    /// SurfaceFlinger accepts the changes.
    changes: Vec<i64>,
}

/// A failed command, as a composer3 `EX_*` code.
pub type Error = i32;

impl Display {
    pub fn new() -> Display {
        Display {
            layers: HashMap::new(),
            next_layer: 1,
            slots: vec![None; 1],
            imported: HashMap::new(),
            target: None,
            target_fence: None,
            changes: Vec::new(),
        }
    }

    pub fn create_layer(&mut self) -> i64 {
        let id = self.next_layer;
        self.next_layer += 1;
        self.layers.insert(id, Composition::INVALID);
        id
    }

    pub fn destroy_layer(&mut self, layer: i64) -> bool {
        self.layers.remove(&layer).is_some()
    }

    pub fn has_layer(&self, layer: i64) -> bool {
        self.layers.contains_key(&layer)
    }

    pub fn set_composition(&mut self, layer: i64, c: Composition) {
        self.layers.insert(layer, c);
    }

    /// Resize the client target slot table, dropping buffers of removed
    /// slots.
    pub fn set_slot_count(&mut self, host: &Host, count: usize) {
        for slot in count..self.slots.len() {
            self.clear_slot(host, slot);
        }
        self.slots.resize(count.max(1), None);
    }

    fn clear_slot(&mut self, host: &Host, slot: usize) {
        let Some(id) = self.slots.get_mut(slot).and_then(Option::take) else {
            return;
        };
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
                self.clear_slot(host, slot);
                if !self.imported.contains_key(&h.id) {
                    host.import(fd.as_ref().as_fd(), &h).map_err(|e| {
                        log::error!("client target {:#x}: import failed: {e:?}", h.id);
                        EX_NO_RESOURCES
                    })?;
                }
                *self.imported.entry(h.id).or_default() += 1;
                self.slots[slot] = Some(h.id);
            }
        }
        self.target = self.slots[slot];
        self.target_fence = fence;
        Ok(())
    }

    /// Every layer is composed by the client; the changes SurfaceFlinger
    /// must accept.
    pub fn validate(&mut self) -> Vec<ChangedCompositionLayer> {
        self.changes = self
            .layers
            .iter()
            .filter(|&(_, &c)| c != Composition::CLIENT)
            .map(|(&layer, _)| layer)
            .collect();
        self.changes.sort();
        self.changes
            .iter()
            .map(|&layer| ChangedCompositionLayer {
                layer,
                composition: Composition::CLIENT,
            })
            .collect()
    }

    pub fn accept_changes(&mut self) {
        for layer in std::mem::take(&mut self.changes) {
            self.layers.insert(layer, Composition::CLIENT);
        }
    }

    /// Show the client target, once its rendering is done.
    pub fn present(&mut self, host: &Host) {
        if let Some(fence) = self.target_fence.take() {
            let mut p = libc::pollfd {
                fd: fence.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll on one fd we hold.
            unsafe { libc::poll(&mut p, 1, FENCE_TIMEOUT_MS) };
        }
        if let Some(id) = self.target
            && let Err(e) = host.present(id)
        {
            log::error!("present {id:#x}: {e:?}");
        }
    }

    /// Forget everything, as for a new client.
    pub fn reset(&mut self, host: &Host) {
        for id in self.imported.keys() {
            let _ = host.release(*id);
        }
        *self = Display::new();
    }
}
