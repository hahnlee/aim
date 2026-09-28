# Android 16 codec foundation on Darwin arm64

This gate builds the complete Android.bp host/arm64 source selection for
`libimage_io`, `libjpeg`, `libultrahdr`, `libjpegencoder`, and
`libjpegdecoder`. Because `libimage_io` declares `libmodpb64` as a static
dependency, its one-source archive is included as part of the module closure.
No unresolved symbol is replaced with a compatibility stub.

`tools/sync-android16-codec-foundation.sh` uses immutable Android 16 Gitiles
revisions without Git history. `image_io` materializes its exact `src` and
`includes` subtrees. JPEG extracts the 67 Blueprint-selected common/arm64
sources plus their root and arm64 SIMD support headers. UltraHDR extracts its
ten host-arm64 sources, two helper module sources, public include subtree, and
root API header. It does not materialize Android-only GLES sources.
The resulting ignored source closure is 2,955,384 logical bytes: ImageIO 98
files/346,353 bytes, modp_b64 4/43,912, libjpeg-turbo 110/1,928,974, and
UltraHDR 26/636,145. No repository checkout or `.git` directory is created.

Run:

```sh
tools/build-android16-codec-foundation.sh
```

Acceptance requires six arm64 archives with 116 total members, an `ld -r`
force-load audit with defined/undefined manifests, and a native executable
that compresses and decodes a 4x4 RGB JPEG. The smoke also passes that JPEG to
UltraHDR's `is_uhdr_image`; this exercises UltraHDR's real ImageIO scanner and
must correctly reject the ordinary JPEG before constructing and releasing an
UltraHDR decoder.

The verified clean Darwin arm64 build produces these archive member counts:
`libmodpb64` 1, `libimage_io` 36, `libjpeg` 67, `libultrahdr` 10,
`libjpegencoder` 1, and `libjpegdecoder` 1. Its global-definition manifest has
1,924 entries and SHA-256
`ead2379e4fa06ff004892b1b1559f1cb6a54d49360764ee8d54f75303e0ef9c9`.
The relocatable force-load object has 172 unresolved Darwin/libc++ runtime
symbols, recorded with SHA-256
`d9bce4b94470d99b0281ec7e94af05328e59c17416c0ff11f367ea088a6263d4`;
the final executable resolves them without extra compatibility code. The
deterministic smoke result is `jpeg=668 fnv1a=79f77f992abae25f` and its output
manifest SHA-256 is
`f084e4b9eadce3d70581dc48a5f9ac5784574d79f30d97b721e51a6f5b2454c0`.
