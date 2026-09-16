//! Guest descriptor numbers and their per-descriptor lifetime state.
//!
//! The host `File` remains the resource owner, while this table owns the
//! Android descriptor number and its descriptor-local flags.  In particular,
//! FD_CLOEXEC is not inferred from the private host descriptor: Android
//! `dup`/`fcntl` semantics are maintained here and host descriptors remain
//! close-on-exec as a safety invariant.

use super::{CENTRAL_BROKER_TOKEN_MARKER, Descriptor, host_fcntl};
use darwin_art_fs_broker::guest_path::MountOrigin;
use std::collections::BTreeMap;
use std::ffi::c_int;

pub(super) const FD_CLOEXEC: c_int = 1;
const F_SETFD: c_int = 2;

pub(super) struct DescriptorTable {
    pub(super) next: c_int,
    pub(super) free: Vec<c_int>,
    pub(super) entries: BTreeMap<c_int, Descriptor>,
    pub(super) fd_flags: BTreeMap<c_int, c_int>,
    /// Resolver provenance retained separately from the host `File`.
    ///
    /// `None` is deliberately the conservative default: a host-adopted or
    /// otherwise unclassified descriptor is never treated as an image node.
    /// The sidecar is keyed by the guest descriptor number so ownership moves
    /// (`take`/`restore`) can preserve it without changing `Descriptor`.
    pub(super) fd_origins: BTreeMap<c_int, Option<MountOrigin>>,
}

impl Default for DescriptorTable {
    fn default() -> Self {
        Self {
            next: 10_000,
            free: Vec::new(),
            entries: BTreeMap::new(),
            fd_flags: BTreeMap::new(),
            fd_origins: BTreeMap::new(),
        }
    }
}

impl DescriptorTable {
    pub(super) fn insert(&mut self, descriptor: Descriptor) -> Result<c_int, ()> {
        self.insert_with_flags(descriptor, false)
    }

    pub(super) fn insert_with_flags(
        &mut self,
        descriptor: Descriptor,
        close_on_exec: bool,
    ) -> Result<c_int, ()> {
        self.insert_with_origin(descriptor, close_on_exec, None)
    }

    pub(super) fn insert_with_origin(
        &mut self,
        descriptor: Descriptor,
        close_on_exec: bool,
        origin: Option<MountOrigin>,
    ) -> Result<c_int, ()> {
        // The host fd is private implementation state. Keep it protected even
        // when Android clears the guest descriptor's FD_CLOEXEC bit.
        if !protect_host_descriptor(&descriptor) {
            return Err(());
        }
        let flags = if close_on_exec { FD_CLOEXEC } else { 0 };
        if let Some(fd) = self.free.pop() {
            assert!(self.entries.insert(fd, descriptor).is_none());
            // Reuse must never inherit the previous descriptor's flags.
            self.fd_flags.insert(fd, flags);
            // A closed slot has no retained provenance. Store the new
            // descriptor's origin explicitly, including `None`.
            assert!(self.fd_origins.insert(fd, origin).is_none());
            return Ok(fd);
        }
        for _ in 0..100_000 {
            let candidate = self.next;
            self.next = if self.next >= CENTRAL_BROKER_TOKEN_MARKER - 1 {
                10_000
            } else {
                self.next + 1
            };
            if candidate & CENTRAL_BROKER_TOKEN_MARKER != 0
                || self.fd_flags.contains_key(&candidate)
                || self.fd_origins.contains_key(&candidate)
            {
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = self.entries.entry(candidate)
            {
                entry.insert(descriptor);
                self.fd_flags.insert(candidate, flags);
                self.fd_origins.insert(candidate, origin);
                return Ok(candidate);
            }
        }
        Err(())
    }

    pub(super) fn close_entry(&mut self, fd: c_int) -> Option<Descriptor> {
        let descriptor = self.entries.remove(&fd)?;
        self.fd_flags.remove(&fd);
        self.fd_origins.remove(&fd);
        self.free.push(fd);
        Some(descriptor)
    }

    pub(super) fn take(&mut self, fd: c_int) -> Option<Descriptor> {
        // Keep fd_flags while a descriptor is temporarily leased. `restore`
        // then preserves the exact descriptor-local state across sendfile.
        self.entries.remove(&fd)
    }

    pub(super) fn restore(&mut self, fd: c_int, descriptor: Descriptor) {
        assert!(self.entries.insert(fd, descriptor).is_none());
        assert!(self.fd_flags.contains_key(&fd));
        assert!(self.fd_origins.contains_key(&fd));
    }

    pub(super) fn origin(&self, fd: c_int) -> Option<&MountOrigin> {
        self.fd_origins.get(&fd).and_then(Option::as_ref)
    }

    pub(super) fn fd_flags(&self, fd: c_int) -> Option<c_int> {
        self.fd_flags.get(&fd).copied()
    }

    pub(super) fn set_fd_flags(&mut self, fd: c_int, flags: c_int) -> bool {
        if !self.entries.contains_key(&fd) {
            return false;
        }
        self.fd_flags.insert(fd, flags & FD_CLOEXEC);
        true
    }
}

fn protect_host_descriptor(descriptor: &Descriptor) -> bool {
    let fd = match descriptor {
        Descriptor::File(file) | Descriptor::PrivateFile(file) => {
            use std::os::fd::AsRawFd;
            file.as_raw_fd()
        }
        Descriptor::Random(_) | Descriptor::Overlay(_) => return true,
    };
    // SAFETY: fd is borrowed from the live File owned by `descriptor`; the
    // variadic call only changes the host descriptor's private close flag.
    unsafe { host_fcntl(fd, F_SETFD, FD_CLOEXEC) >= 0 }
}

#[cfg(test)]
#[path = "descriptor_origin_tests.rs"]
mod descriptor_origin_tests;
