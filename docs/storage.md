# Storage: case-sensitive disk images

Android's filesystems are case-sensitive; the Mac's system volume is not.
The pinned image holds names that differ only in case, and apps create
such names in `/data`. So the guest's files live in case-sensitive APFS
disk images (`crates/aim-storage`), which the user attaches without admin
rights and which stay hidden from the Finder (`nobrowse`):

| Image | What | Format | Where |
| --- | --- | --- | --- |
| Data image | the guest's writable data of one data directory | sparse (ASIF), read/write | `<data>.asif`, mounted at `<data>` |
| System image | the pinned original and its translation cache | compressed (lzfse), read-only | see "System image" |

## Data image

`guest-init --run --data DATA` (and so `cargo aim boot` and `cargo aim
bench`) keeps everything of `DATA` in `DATA.asif` beside it:

- **Created on demand**: `diskutil image create blank --format ASIF`
  with no filesystem, then `newfs_apfs -e` (case-sensitive APFS, volume
  `aim-data`). Its size is a ceiling of 64 GB (what `df /data` shows); the
  file holds only what the guest wrote. Nothing is preallocated.
- **Attached at start, detached at stop**: guest-init attaches it at
  `DATA` (`diskutil image attach --nobrowse --mountPoint DATA`) before it
  lays out the boot, and detaches it (`diskutil eject`) when the boot ends,
  on its timeout, SIGINT or SIGTERM. The path map, `DATA/run` (the
  runtime directory, logs, the path map file) and the persistent `/data`,
  `/metadata` and `/cache` are all on it, at the paths they had before.
- **One user**: guest-init holds an exclusive lock on `DATA.lock` while the
  image is attached. A second guest-init on the same data fails at once.
- **Crash recovery**: an image found attached at start belongs to a
  guest-init that died without detaching; it is detached and attached
  again, and APFS replays its journal. If the attach fails, `fsck_apfs -y`
  repairs the volume and the attach is retried.
- **Owners persist**: a guest's chown, chmod and file creator are an xattr
  on the file itself (`docs/guest-init-contract.md` section 6), so they
  are in the image with the data. They used to be in the runtime
  directory's table, which each boot wiped (#261).
- A data directory from before data images (a plain directory with
  content) is refused; move it away or remove it.
  `aim_storage::data::remove` removes a data directory with its image.

### Space: TRIM, and compaction at stop

Measured on macOS 27 (26A428), M2 Pro:

| Image | 1 GiB written, deleted, then stopped |
| --- | --- |
| Sparse bundle, 1 MiB or 8 MiB bands | keeps all of it: APFS's TRIM frees no band, and `hdiutil compact` returned 3 MB |
| ASIF | APFS TRIMs, and ASIF punches the TRIMmed chunks out of its file, but only once about 2 GiB are free: 1.75 GiB freed stay, 2 GiB are returned at unmount |
| ASIF with compaction at stop | 11 MB left (`tests/data.rs`, `deleted_data_returns_to_the_host_at_stop`) |

Hence ASIF rather than a sparse bundle, and at stop, when the image's file
holds more than 512 MiB beyond what its volume uses, guest-init allocates
2 GiB in the volume without writing it (`F_PREALLOCATE`) and frees it
before it unmounts: APFS then TRIMs all free space, and the file shrinks to
what the guest's files use. It needs that much free space on the host
for a moment.

## Security agent

The Mac's security agent (Exosphere) scans a volume's freshly written
files for a while and keeps them open, so a detach may find the volume
busy; detaching retries for 20 s (2 min for the system image's build
volume), then unmounts by force.
