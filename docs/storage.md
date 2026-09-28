# Storage: case-sensitive disk images

Android's filesystems are case-sensitive; the Mac's system volume is not.
The pinned image holds names that differ only in case, and apps create
such names in `/data`. So the guest's files live in case-sensitive APFS
disk images (`crates/aim-storage`), which the user attaches without admin
rights and which stay hidden from the Finder (`nobrowse`):

| Image | What | Format | Where |
| --- | --- | --- | --- |
| Data image | the guest's writable data of one data directory | sparse (ASIF), read/write | `<data>.asif`, mounted at `<data>` |
| System image | the pinned original and its translation cache | compressed (ULFO, lzfse), read-only | `_build/android16-image.dmg`, mounted at `_build/android16-image` |
| Derived image | the system image plus `image/overlay.toml` and the overlay's translations | a shadow file over the system image | `target/aim/derived.shadow`, mounted read-only at `target/aim/derived` |

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
- After a stop, the boot's logs (`DATA/run/logs`) are in the detached
  image; `diskutil image attach --nobrowse --mountPoint DATA DATA.asif`
  shows them again (`diskutil eject` it before the next boot).
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

## System image

The `image` node of `cargo aim` (docs/build.md) builds it once per pin:

1. The archive of `image/original.lock` is checked against its sha256.
2. A blank case-sensitive ASIF volume (`aim-system`, 32 GB ceiling) is
   attached writable, and `android-image-extract` extracts the archive
   into its `root/`, recording the archive's sha256 in `root.identity`.
   The extraction fails if a name collides with another, which on this
   volume cannot happen: every path of the archive's partitions and APEX
   payloads is there.
3. `linux-translate --image` translates every AArch64 ELF of `root/` into
   `translated/`: the translation cache, as in `~/Library/Caches/aim`
   (entries keyed by the original's sha256 and the translator version),
   with its index keyed by the path relative to `root/` instead of the
   host path, since the mount point and device change on every attach.
4. The volume is converted to a compressed read-only image
   (`diskutil image create from --format ULFO`) and published with a
   rename, then attached read-only and hidden at
   `_build/android16-image`.

Volume layout:

```
root/               the guest's root: the original files, byte-identical,
                    with their SELinux labels (dev.aim.xattr.security.selinux)
                    and original owners and modes (dev.aim.android-inode)
root.identity       the archive's sha256
translated/         <sha256>-v<VERSION>/{meta,elf}: translated copies
translated/paths/   <sha256 of the path relative to root/> -> "<ino>:<size>:<mtime> <sha256>"
```

An existing image is never rewritten: other checkouts may link and use
it. The node checks its identity and attaches it; a Mac restart detaches
it, and the next `cargo aim build` (or `test`, `boot`, `bench`) attaches it
again. `aim_paths::original_image()` is `_build/android16-image/root`
(beside the real image file, so a worktree that links the image finds the
same mount).

**It is a local build artefact.** The image holds the pinned Google
system image's content; it is never committed or distributed, like the
archive it is built from and the extracted tree it replaces.
Distribution of the base image is covered by [gsi-base.md](gsi-base.md).

### Derived image

The `derived-image` node attaches the system image with a shadow file,
`target/aim/derived.shadow`, which takes every write, applies the overlay
to its `root/` (`aim_android_image::assemble::apply`) and writes the
derived identity last. The `translation-cache` node then attaches it
writable again and runs `linux-translate --image` for what the system
image's cache lacks: the overlay's files, or every file after a
translator version change. It stays attached read-only at
`target/aim/derived`; `aim_paths::derived_image()` is
`target/aim/derived/root`. The shadow holds only the changes (tens of
MB), where the derived image used to be an APFS clone of the whole
extracted tree. A shadow that does not attach or holds another identity
is replaced.

### Lookup order in linux-run

For a file under its root `R`, linux-run looks in the image's cache
`R/../translated` first, then in the user cache (`--cache`, default
`~/Library/Caches/aim/translated`, indexed by host path), and rewrites at
load time only what neither has: code made at run time, libraries inside
APKs, and files in `/data`. With an empty user cache, the image's
programs run with no load-time rewriting
(`crates/aim-linux-abi/tests/image_cache.rs`).

### Measurements

The pinned archive (`arm64-v8a-playstore-ps16k-36_r07`) on an M2 Pro,
macOS 27, with other builds loading the machine:

| | Size |
| --- | --- |
| The extracted tree `_build/android16-image-full` (case-insensitive, 7 files lost) | 3.8 GB |
| Its translation cache | 520 MB |
| The volume (tree and cache) | 4.6 GB used |
| ULFO (lzfse), chosen | 2.12 GB |
| UDZO (zlib) | 2.12 GB |
| ULMO (lzma) | 1.69 GB |

Read cost, all 4,453 files (3,920 MiB) of `root/` read once with one
thread right after attaching (cold: every block decompressed), then again
(warm: the page cache):

| | Cold | Warm | Build (convert) |
| --- | --- | --- | --- |
| ULFO | 7.2 s, 546 MiB/s | 0.51 s | 4 s |
| UDZO | 10.7 s, 368 MiB/s | 0.44 s | 10 s |
| ULMO | 71.8 s, 55 MiB/s | 69.7 s, 56 MiB/s | 83 s |

lzma is 20 % smaller, but its reads are never cached; lzfse builds and
reads fastest. The whole `image` node (extraction, translation,
compression) takes about 140 s.

## Security agent

The Mac's security agent (Exosphere) scans a volume's freshly written
files for a while and keeps them open, so a detach may find the volume
busy; detaching retries for 20 s (2 min for the system image's build
volume), then unmounts by force.
