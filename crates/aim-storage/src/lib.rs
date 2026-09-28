//! Case-sensitive APFS disk images for the guest (docs/storage.md).
//!
//! Android's filesystems are case-sensitive, the Mac's usually are not: an
//! original image holds names that differ only in case, and apps create
//! them in `/data`. The guest's files therefore live in disk images the
//! user attaches without admin rights, hidden from the Finder:
//!
//! - [`data`]: the writable data of one data directory, a sparse image.

pub mod data;
pub mod disk;
