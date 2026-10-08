//! Case-sensitive APFS disk images for the guest (docs/storage.md).
//!
//! Android's filesystems are case-sensitive, the Mac's usually are not: an
//! original image holds names that differ only in case, and apps create
//! them in `/data`. The guest's files therefore live in disk images the
//! user attaches without admin rights, hidden from the Finder:
//!
//! - [`system`]: the pinned original image as a compressed read-only image
//!   with its translation cache, and the derived image as a shadow over it;
//! - [`data`]: the writable data of one data directory, a sparse image;
//! - [`copy`]: copies between them that keep what the guest sees.

pub mod copy;
pub mod data;
pub mod disk;
pub mod guest_inode;
pub mod system;

pub mod process_namespace;
pub mod mount_namespace;

pub mod socket_inode;

pub mod inode_lease;

pub mod fsverity;
