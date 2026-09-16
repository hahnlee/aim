//! Bionic directory-stream ownership over the macOS directory transport.
//!
//! This module alone owns stream tokens, translated entries and close/drop.
//! Path admission and descriptor capabilities remain with the filesystem facade;
//! no host DIR pointer is exposed through the Android ABI.

use super::{
    ANDROID_EBADF, ANDROID_ENOTDIR, AndroidDirent, Descriptor, Facade, HostDirent, O_CLOEXEC,
    O_DIRECTORY, O_RDONLY, android_directory_type, darwin_art_bionic_fs_host_closedir,
    darwin_art_bionic_fs_host_fdopendir, darwin_art_bionic_fs_host_readdir,
    darwin_art_bionic_fs_host_rewinddir, host_close,
};
use std::collections::BTreeMap;
use std::ffi::{c_int, c_void};
use std::os::fd::IntoRawFd;
use std::ptr;

#[repr(C)]
struct DirectoryToken {
    opaque_id: u64,
}

struct DirectoryRecord {
    // Token and translated entry are separate allocations containing no host
    // descriptor or DIR pointer. Only their addresses cross the guest ABI.
    _token: Box<DirectoryToken>,
    host_directory: usize,
    guest_fd: c_int,
    offset: i64,
    entry: Box<AndroidDirent>,
}

impl Drop for DirectoryRecord {
    fn drop(&mut self) {
        if self.host_directory != 0 {
            let mut ignored_errno = 0;
            // SAFETY: a nonzero stream is owned by this state until this call.
            unsafe {
                darwin_art_bionic_fs_host_closedir(
                    self.host_directory as *mut c_void,
                    &mut ignored_errno,
                )
            };
            self.host_directory = 0;
        }
    }
}

pub(super) struct DirectoryTable {
    // Only live streams are retained. POSIX makes DIR* use after closedir
    // undefined, so close can reclaim both facade-owned guest allocations.
    streams: BTreeMap<usize, DirectoryRecord>,
    next_id: u64,
}

impl Default for DirectoryTable {
    fn default() -> Self {
        Self {
            streams: BTreeMap::new(),
            next_id: 1,
        }
    }
}

impl DirectoryTable {
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    fn insert(&mut self, host_directory: *mut c_void, guest_fd: c_int) -> *mut c_void {
        let opaque_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let token = Box::new(DirectoryToken { opaque_id });
        let token_pointer = (&*token as *const DirectoryToken).cast_mut().cast();
        let state = DirectoryRecord {
            _token: token,
            host_directory: host_directory as usize,
            guest_fd,
            offset: 0,
            entry: Box::new(AndroidDirent::default()),
        };
        self.streams.insert(token_pointer as usize, state);
        token_pointer
    }
}

impl Facade {
    pub(super) fn opendir(&self, path: &[u8]) -> *mut c_void {
        // Use the same guest path admission as open, including GuestRoot.
        // AOSP opendir opens O_RDONLY|O_DIRECTORY|O_CLOEXEC. Keep that
        // descriptor-local flag in the guest table; the host descriptor is
        // already protected by the descriptor-table owner.
        let fd = self.open_with_mode(path, O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0);
        if fd < 0 {
            return ptr::null_mut();
        }
        let directory = self.fdopendir(fd);
        if directory.is_null() {
            self.close(fd);
        }
        directory
    }

    pub(super) fn fdopendir(&self, fd: c_int) -> *mut c_void {
        // All operations needing both tables acquire streams before descriptors.
        // Preserve the guest descriptor number until closedir, as Bionic does.
        let mut directories = match self.directories.lock() {
            Ok(table) => table,
            Err(_) => {
                self.fail_capability();
                return ptr::null_mut();
            }
        };
        let descriptors = match self.descriptors.lock() {
            Ok(table) => table,
            Err(_) => {
                self.fail_capability();
                return ptr::null_mut();
            }
        };
        let file = match descriptors.entries.get(&fd) {
            Some(Descriptor::File(file) | Descriptor::PrivateFile(file)) => file,
            Some(_) => {
                self.fail(ANDROID_ENOTDIR);
                return ptr::null_mut();
            }
            None => {
                self.fail(ANDROID_EBADF);
                return ptr::null_mut();
            }
        };
        match file.metadata() {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                self.fail(ANDROID_ENOTDIR);
                return ptr::null_mut();
            }
            Err(error) => {
                self.fail_io(&error);
                return ptr::null_mut();
            }
        }
        // dup shares the open file description with the guest descriptor.
        // The stream owns the duplicate; the table retains the guest capability.
        let stream_file = match file.try_clone() {
            Ok(file) => file,
            Err(error) => {
                self.fail_io(&error);
                return ptr::null_mut();
            }
        };
        let raw_fd = stream_file.into_raw_fd();
        let mut host_errno = 0;
        // SAFETY: fdopendir consumes raw_fd only on success.
        let stream = unsafe { darwin_art_bionic_fs_host_fdopendir(raw_fd, &mut host_errno) };
        if stream.is_null() {
            unsafe { host_close(raw_fd) };
            self.fail_host_errno(host_errno);
            return ptr::null_mut();
        }
        directories.insert(stream, fd)
    }

    pub(super) fn dirfd(&self, directory: *mut c_void) -> c_int {
        let directories = match self.directories.lock() {
            Ok(table) => table,
            Err(_) => return self.fail_capability(),
        };
        match directories.streams.get(&(directory as usize)) {
            Some(record) => record.guest_fd,
            None => self.fail(ANDROID_EBADF),
        }
    }

    pub(super) fn readdir(&self, directory: *mut c_void) -> *mut AndroidDirent {
        if directory.is_null() {
            self.fail(ANDROID_EBADF);
            return ptr::null_mut();
        }
        // A single lock serializes readdir/closedir on every facade stream.
        // Tokens are keys only and are never dereferenced before membership.
        let mut directories = match self.directories.lock() {
            Ok(directories) => directories,
            Err(_) => {
                self.fail_capability();
                return ptr::null_mut();
            }
        };
        let Some(state) = directories.streams.get_mut(&(directory as usize)) else {
            self.fail(ANDROID_EBADF);
            return ptr::null_mut();
        };
        if state.host_directory == 0 {
            self.fail(ANDROID_EBADF);
            return ptr::null_mut();
        }
        let mut host_entry = HostDirent::default();
        let mut host_errno = 0;
        // SAFETY: the table exclusively owns and serializes this live stream.
        let result = unsafe {
            darwin_art_bionic_fs_host_readdir(
                state.host_directory as *mut c_void,
                &mut host_entry,
                &mut host_errno,
            )
        };
        if result == 0 {
            // Bionic readdir leaves errno unchanged at end-of-directory.
            return ptr::null_mut();
        }
        if result < 0 {
            self.fail_host_errno(host_errno);
            return ptr::null_mut();
        }
        let name_length = usize::from(host_entry.d_name_length);
        if name_length >= host_entry.d_name.len() {
            self.fail_capability();
            return ptr::null_mut();
        }
        state.offset = match state.offset.checked_add(1) {
            Some(offset) => offset,
            None => {
                self.fail_capability();
                return ptr::null_mut();
            }
        };
        let record_length = (19usize + name_length + 1 + 7) & !7;
        *state.entry = AndroidDirent::default();
        state.entry.d_ino = host_entry.d_ino;
        state.entry.d_off = state.offset;
        state.entry.d_reclen = record_length as u16;
        state.entry.d_type = android_directory_type(host_entry.d_type);
        state.entry.d_name[..=name_length].copy_from_slice(&host_entry.d_name[..=name_length]);
        &raw mut *state.entry
    }

    pub(super) fn closedir(&self, directory: *mut c_void) -> c_int {
        if directory.is_null() {
            return self.fail(ANDROID_EBADF);
        }
        let mut directories = match self.directories.lock() {
            Ok(directories) => directories,
            Err(_) => return self.fail_capability(),
        };
        let Some(mut state) = directories.streams.remove(&(directory as usize)) else {
            return self.fail(ANDROID_EBADF);
        };
        let host_directory = std::mem::replace(&mut state.host_directory, 0);
        // Closing the stream invalidates its guest descriptor as well.
        self.close(state.guest_fd);
        let mut host_errno = 0;
        // SAFETY: table ownership is transferred exactly once to closedir.
        if unsafe {
            darwin_art_bionic_fs_host_closedir(host_directory as *mut c_void, &mut host_errno)
        } == 0
        {
            0
        } else {
            self.fail_host_errno(host_errno)
        }
    }

    pub(super) fn rewinddir(&self, directory: *mut c_void) {
        if directory.is_null() {
            self.fail(ANDROID_EBADF);
            return;
        }
        let mut directories = match self.directories.lock() {
            Ok(directories) => directories,
            Err(_) => {
                self.fail_capability();
                return;
            }
        };
        let Some(state) = directories.streams.get_mut(&(directory as usize)) else {
            self.fail(ANDROID_EBADF);
            return;
        };
        unsafe { darwin_art_bionic_fs_host_rewinddir(state.host_directory as *mut c_void) };
        state.offset = 0;
    }
}
