//! The display server, through the host-call module `display`.

use std::io::Read;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use aim_gralloc::Handle;
use aim_hostcall::display::{Connect, Cursor, Event, Import, Layer, Layers, Present, event};
use aim_hostcall::guest::{self, Errno};

/// The connected display: its mode, and the fd its events arrive on.
pub struct Host {
    pub info: Connect,
    events: OwnedFd,
}

impl Host {
    pub fn connect() -> Result<Host, Errno> {
        let mut info = Connect::default();
        let fd = guest::display_connect(&mut info)?;
        // SAFETY: the host call returned a new fd that we now own.
        let events = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Host { info, events })
    }

    /// Give the server the buffer behind `fd`, named by its buffer id.
    pub fn import(&self, fd: BorrowedFd, h: &Handle) -> Result<(), Errno> {
        let layout = h.layout().ok_or(Errno(libc::EINVAL))?;
        let mut args = Import {
            fd: fd.as_raw_fd(),
            format: h.format,
            width: h.width,
            height: h.height,
            stride_bytes: layout.stride_bytes() as u32,
            _reserved: 0,
            // Page aligned and past every plane (docs/graphics-buffers.md).
            length: h.metadata_offset,
            id: h.id,
        };
        guest::display_import(&mut args)
    }

    /// Show buffer `id` once `acquire` signals; returns the present fence.
    pub fn present(&self, id: u64, acquire: Option<BorrowedFd>) -> Result<OwnedFd, Errno> {
        let mut args = Present {
            id,
            acquire: acquire.map_or(-1, |f| f.as_raw_fd()),
            present: -1,
        };
        guest::display_present(&mut args)?;
        // SAFETY: the host call returned a new fd that we now own.
        Ok(unsafe { OwnedFd::from_raw_fd(args.present) })
    }

    /// Show a frame of `layers` (window mode) once their buffers and the
    /// client target `target` (0: none) are ready; returns the present
    /// fence.
    pub fn layers(
        &self,
        layers: &[Layer],
        rects: &[[i32; 4]],
        target: u64,
        acquire: Option<BorrowedFd>,
    ) -> Result<OwnedFd, Errno> {
        let mut args = Layers {
            layers: layers.as_ptr() as u64,
            count: layers.len() as u32,
            rect_count: rects.len() as u32,
            rects: rects.as_ptr() as u64,
            client_target: target,
            client_acquire: acquire.map_or(-1, |f| f.as_raw_fd()),
            present: -1,
        };
        guest::display_layers(&mut args)?;
        // SAFETY: the host call returned a new fd that we now own.
        Ok(unsafe { OwnedFd::from_raw_fd(args.present) })
    }

    /// Show buffer `id` (0: none) as the hardware cursor, its top left
    /// corner at `at`; `changed`: new content, ready once `acquire`
    /// signals.
    pub fn cursor(
        &self,
        id: u64,
        at: (i32, i32),
        changed: bool,
        acquire: Option<BorrowedFd>,
    ) -> Result<(), Errno> {
        let mut args = Cursor {
            id,
            x: at.0,
            y: at.1,
            changed: changed as u32,
            acquire: acquire.map_or(-1, |f| f.as_raw_fd()),
        };
        guest::display_cursor(&mut args)
    }

    pub fn release(&self, id: u64) -> Result<(), Errno> {
        guest::display_release(id)
    }

    pub fn set_vsync(&self, enabled: bool) -> Result<(), Errno> {
        guest::display_set_vsync(enabled)
    }

    /// Read vsync events until the server goes away.
    pub fn run_events(&self, mut on_vsync: impl FnMut(&Event)) {
        // SAFETY: a borrowed view of our fd, closed only with `self`.
        let mut file = std::mem::ManuallyDrop::new(unsafe {
            std::fs::File::from_raw_fd(self.events.as_raw_fd())
        });
        let mut e = Event::default();
        loop {
            // SAFETY: `Event` is plain old data.
            let buf = unsafe {
                std::slice::from_raw_parts_mut((&mut e as *mut Event).cast(), size_of::<Event>())
            };
            if file.read_exact(buf).is_err() {
                return;
            }
            if e.kind == event::VSYNC {
                on_vsync(&e);
            }
        }
    }
}
