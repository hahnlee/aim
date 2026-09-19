//! Synchronous namespace admission at the filesystem descriptor owner.
//!
//! Other owners commit only after every file and sidecar is staged under the
//! FS table mutex. Their callback must not wait, perform RPC or reenter FS.
//! No guard or reservation escapes this boundary.

use super::{Descriptor, DescriptorTable, File, FromRawFd, acquire_active};
use std::ffi::{c_int, c_void};

const MAX_GROUP: usize = 16;

#[repr(C)]
pub struct OwnedDescriptor {
    pub host_fd: c_int,
    pub descriptor_flags: u32,
}

pub type CommitGroup = unsafe extern "C" fn(*mut c_void, *const c_int, usize) -> c_int;

fn stage_and_commit(
    table: &mut DescriptorTable,
    files: &mut [Option<File>; MAX_GROUP],
    flags: &[bool; MAX_GROUP],
    count: usize,
    commit: impl FnOnce(&[c_int]) -> c_int,
) -> Result<[c_int; MAX_GROUP], c_int> {
    let next_before = table.next;
    let reused = count.min(table.free.len());
    let mut staged = [-1; MAX_GROUP];
    let mut installed = 0;
    let mut error = 24;
    for index in 0..count {
        let file = files[index].take().expect("complete owned input group");
        match table.insert_with_flags(Descriptor::File(file), flags[index]) {
            Ok(fd) => { staged[index] = fd; installed += 1; }
            Err(()) => break,
        }
    }
    if installed == count {
        error = commit(&staged[..count]);
        if error == 0 { return Ok(staged); }
    }
    // Restore allocator state and all descriptor sidecars without allocating.
    // Vec pushes replace slots popped from the same existing allocation.
    for index in (0..installed).rev() {
        let fd = staged[index];
        table.entries.remove(&fd);
        table.fd_flags.remove(&fd);
        table.fd_origins.remove(&fd);
        if index < reused { table.free.push(fd); }
    }
    table.next = next_before;
    Err(if error > 0 { error } else { 5 })
}

#[unsafe(no_mangle)]
/// Consumes every distinct input host FD on success and failure. Outputs are
/// untouched on failure. Entries/output point to `count` records (<=16).
/// A successful callback is the namespace linearization point; no fallible
/// work follows it. A null callback commits a file-only group.
pub unsafe extern "C" fn darwin_art_bionic_fs_adopt_group(
    entries: *const OwnedDescriptor,
    count: usize,
    commit: Option<CommitGroup>,
    context: *mut c_void,
    output: *mut c_int,
) -> c_int {
    if count > MAX_GROUP || (count != 0 && entries.is_null()) {
        return 22;
    }
    let mut files: [Option<File>; MAX_GROUP] = std::array::from_fn(|_| None);
    let mut raw = [-1; MAX_GROUP];
    let mut flags = [false; MAX_GROUP];
    let mut error = if count != 0 && output.is_null() { 22 } else { 0 };
    for index in 0..count {
        // SAFETY: trusted caller's complete synchronous input array.
        let entry = unsafe { &*entries.add(index) };
        if entry.host_fd < 0 || raw[..index].contains(&entry.host_fd) {
            error = 9;
            continue;
        }
        raw[index] = entry.host_fd;
        // SAFETY: unique descriptor ownership transfers at this boundary.
        files[index] = Some(unsafe { File::from_raw_fd(entry.host_fd) });
        if entry.descriptor_flags & !1 != 0 { error = 22; }
        flags[index] = entry.descriptor_flags & 1 != 0;
    }
    if error != 0 { return error; }
    let Some(active) = acquire_active() else { return 9; };
    let Ok(mut table) = active.facade.descriptors.lock() else { return 5; };
    let result = stage_and_commit(&mut table, &mut files, &flags, count, |fds| {
        match commit {
            // SAFETY: callback borrows only this operation's staged numbers.
            Some(commit) => unsafe { commit(context, fds.as_ptr(), fds.len()) },
            None => 0,
        }
    });
    match result {
        Ok(staged) => {
            if count != 0 {
                // SAFETY: trusted output array is writable and nonnull.
                unsafe { std::ptr::copy_nonoverlapping(staged.as_ptr(), output, count) };
            }
            0
        }
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn files(count: usize) -> [Option<File>; MAX_GROUP] {
        std::array::from_fn(|index| {
            if index < count { Some(File::open("/dev/null").unwrap()) } else { None }
        })
    }

    #[test]
    fn failed_other_owner_commit_restores_entries_flags_provenance_and_allocator() {
        let mut table = DescriptorTable::default();
        let old = table.insert(Descriptor::File(File::open("/dev/null").unwrap())).unwrap();
        drop(table.close_entry(old));
        let free = table.free.clone();
        let next = table.next;
        let mut input = files(3);
        let raw: Vec<_> = input.iter().flatten().map(AsRawFd::as_raw_fd).collect();
        assert_eq!(stage_and_commit(&mut table, &mut input, &[true; MAX_GROUP], 3,
            |staged| { assert_eq!(staged.len(), 3); 12 }), Err(12));
        assert!(table.entries.is_empty());
        assert!(table.fd_flags.is_empty());
        assert!(table.fd_origins.is_empty());
        assert_eq!(table.free, free);
        assert_eq!(table.next, next);
        for fd in raw { assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1); }
    }

    #[test]
    fn committed_group_preserves_guest_flags_and_private_host_cloexec() {
        let mut table = DescriptorTable::default();
        let mut input = files(2);
        let mut flags = [false; MAX_GROUP]; flags[1] = true;
        let result = stage_and_commit(&mut table, &mut input, &flags, 2, |_| 0).unwrap();
        assert_eq!(table.fd_flags[&result[0]], 0);
        assert_eq!(table.fd_flags[&result[1]], 1);
        for fd in &result[..2] {
            assert_eq!(table.fd_origins[fd], None);
            let Descriptor::File(file) = &table.entries[fd] else { panic!("file owner"); };
            assert_ne!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
        }
    }
}
