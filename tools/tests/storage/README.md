# StorageManager endpoint test

Run `./tools/tests/storage/run.sh` to compile the production
`StorageManagerEndpoint` with the Android 36 API jar and verify that the
Android 16 primary emulated volume is non-removable, mounted, and rooted at
`/storage/emulated/0`. The test does not launch an APK or create external
storage; host-side directory preparation remains an independent filesystem
boundary.
