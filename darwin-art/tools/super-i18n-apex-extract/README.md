# API 36 super-image i18n APEX extractor

This standalone, std-only Rust tool reads the known Android API 36 ARM64
`system.img` layout without mounting or modifying it:

1. validate the primary GPT and select the `super` partition;
2. validate Android LP geometry, metadata SHA-256 checksums, and the `system`
   logical-partition extents;
3. expose those physical extents as a bounded read-only logical view;
4. walk the EROFS path `/system/apex/com.android.i18n.apex`;
5. decode only that inode's compact LZ4 big-pcluster extents into a new output.

The system logical partition is never copied. Unsupported GPT, LP target types,
EROFS layouts, compression algorithms, holes, overlaps, or malformed bounds are
hard errors. The input is opened read-only and the output uses `create_new`.

Use `INPUT - --path /system/apex` to list an image directory without extracting
its files. Directory decoding is isolated in `src/directory.rs`. Use
`INPUT OUTPUT --path /system/etc/classpaths/bootclasspath.pb` to extract the
original platform classpath metadata; APEX modules carry their own metadata.

```sh
cargo run --release --manifest-path tools/super-i18n-apex-extract/Cargo.toml -- \
  "$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore/arm64-v8a/system.img" \
  /tmp/com.android.i18n.apex
```

The parser supports both locked API 36 Play Store ARM64 images currently used
by this project: the ordinary 4 KiB-page image and the `_ps16k` image. Their
compressed APEX bytes differ, but both contain the same locked `core-icu4j.jar`.
The acceptance script hashes the complete input image before selecting either
expected APEX tuple.

The report includes GPT, LP and EROFS versions, logical/physical read counts,
the selected inode and a built-in SHA-256 of the extracted APEX.

Use `INPUT - --stat /system/bin/app_process64` to print the original inode's
octal mode, numeric Android UID/GID and byte size without extracting file data.
`--stat` requires stdout (`-`) and never changes host ownership. The decoder in
`src/inode.rs` preserves compact 16-bit and extended 32-bit IDs according to the
[Linux EROFS disk format](https://github.com/torvalds/linux/blob/v6.12/fs/erofs/erofs_fs.h).
This is image metadata, not the invoking process's credentials. Runtime `stat`
projection and writable inode ownership must explicitly consume/persist their
own metadata; this query alone does not implement guest `chown`.

Newly extracted regular files retain that original metadata in the descriptor-
bound `com.darwin-art.android-inode` attribute before final sync. Its versioned
encoding is owned by `darwin-art-fs-broker::inode_metadata`, and includes the
original mode without applying Android set-ID/executable bits or UID/GID to
the macOS file. Existing attributes cannot be overwritten by this producer.
Symlink and APEX-internal filesystem metadata require their own producers.
