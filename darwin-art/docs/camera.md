# Camera (ADR 0012, P5)

The derived image's camera HAL is `android.hardware.camera.provider`
`ICameraProvider/internal/0` (AIDL V1, with camera.device, camera.common and
camera.metadata V1), a Rust service under `hal/camera`. It offers one
camera device per Mac camera and streams them through the host-call module
`camera` (id 9, `crates/darwin-host-camera`), which drives AVFoundation.
The original cameraserver and the apps' Camera2 stacks run unchanged above
it.

| Piece | Where |
| --- | --- |
| Provider, devices, sessions | `hal/camera` (`/vendor/bin/hw/android.hardware.camera.provider-service.darwin`) |
| Host module `camera`: AVFoundation, conversion, JPEG | `crates/darwin-host-camera` |
| ABI (`Devices`, `Open`, `Frame`, `Output`) | `darwin_hostcall::camera` |
| NDK Camera2 test client (not in the image) | `hal/tests/camera-client` |

## Devices

`FN_DEVICES` lists the cameras of an `AVCaptureDeviceDiscoverySession`
(built-in wide angle, external and Continuity cameras), built-in first,
then by `uniqueID`. Listing needs no camera permission and shows no
prompt. The provider reads the list once at start-up. Device `n` is
`device@1.1/internal/n` and Camera2 id `"n"`.

Each device is a **LIMITED** camera with the `BACKWARD_COMPATIBLE`
capability (`characteristics.rs`):

- **Facing:** the built-in camera is `FRONT`; a camera AVFoundation places
  on the back is `BACK`; others (USB, Continuity) are `EXTERNAL`. Sensor
  orientation 0: the Mac's image is upright in the window.
- **Sizes:** the camera's format sizes (landscape, 30 fps or more) that fit
  inside the widest one, which is the active array. A MacBook Pro's
  FaceTime HD camera has 1920x1080, 1760x1328, 1552x1552, 1280x720 and
  640x480; Android gets 1920x1080, 1280x720 and 640x480 on a 1920x1080
  array.
- **Streams:** every size as `IMPLEMENTATION_DEFINED`, `YCBCR_420_888` and
  `BLOB` (JPEG), 33.3 ms minimum frame duration (30 fps), a 100 ms JPEG
  stall. At most two processed streams and one JPEG stream.
- **Controls:** the camera runs its own exposure, white balance and focus,
  so only the AUTO modes are offered: AE on with auto antibanding, AWB auto,
  AF off, no flash, no effects or scene modes, target FPS ranges [15, 30]
  and [30, 30], digital zoom 1. The results report AE and AWB converged,
  AF inactive and the lens stationary.
- **Optics:** AVFoundation reports no focal length or sensor size on macOS.
  The static metadata uses nominal values for a laptop camera: 4 mm, f/2.0,
  and a sensor sized for a 70° horizontal field of view.
- **Timestamps:** `SENSOR_INFO_TIMESTAMP_SOURCE` is `REALTIME`. A frame's
  `CMSampleBuffer` presentation time (host clock) is converted to the host's
  CLOCK_MONOTONIC, which the syscall layer serves as the guest's
  CLOCK_MONOTONIC and CLOCK_BOOTTIME. Shutter and result carry the same
  timestamp (readout timestamp equal to it).

The static metadata, templates and results are `camera_metadata_t` buffers
written by `metadata.rs` (libcamera_metadata is a system library a vendor
process cannot link). All 103 typed insertions were checked against the
tag types of `camera_metadata_tag_info.c` at the image's tag.

## Sessions and frames

- **Configuration:** `configureStreams` checks the combination, then opens a
  host session at the largest stream size and 30 fps. The host picks the
  device format that covers that size (`activeFormat`, frame duration 1/30
  s) and captures BGRA frames with an `AVCaptureVideoDataOutput` on a
  dispatch queue, keeping the latest one.
- **Buffers:** stream buffers are the derived image's gralloc buffers
  ([graphics-buffers.md](graphics-buffers.md)). A buffer is mapped once per
  buffer id and dropped when cameraserver removes it from the cache. Its
  layout comes from the handle (`darwin-gralloc`).
- **Formats:** `IMPLEMENTATION_DEFINED` is overridden (`HalStream`) to
  `RGBX_8888` when the consumer samples or composes it (a TextureView,
  SurfaceView), since the host GPU has no YUV textures, and to
  `YCBCR_420_888` otherwise (a video encoder). A JPEG buffer is a `BLOB`
  whose width is its size, so the allocator now takes BLOB widths above the
  texture limit.
- **Capture:** requests queue in order (up to the pipeline depth of 4). A
  worker thread waits for the buffers' acquire fences, then makes one
  `FN_FRAME` call: the host waits for a frame newer than the last one
  delivered and writes it into every buffer of the request, scaled
  (bilinear) with a centered crop to each buffer's aspect ratio. RGBX is
  written as is, YUV as NV12 (BT.601 full range, the JFIF dataspace), JPEG
  by ImageIO with the requested quality and `android.jpeg.orientation` as
  the EXIF orientation; the HAL appends the `CameraBlob` trailer.
- **Results:** a second thread sends the shutter, then the result: the
  request's settings echoed, plus the timestamp, frame duration, rolling
  shutter skew, pipeline depth and 3A states. Separating it from the
  capture thread lets the next frame be captured while cameraserver is
  still returning the last one to its consumers.
- **Metadata travels in the parcelables.** Both metadata queue getters
  return an empty descriptor, which cameraserver takes as "no queue"; it
  logs `HAL returns empty result metadata fmq, not use it` at error level.
- **Errors:** a request whose frame does not come within 1 s (3 s for the
  first) is returned with `ERROR_REQUEST`; a buffer that cannot be written
  (a JPEG larger than its buffer) with `ERROR_BUFFER`. `flush` and `close`
  return the queued requests with `ERROR_REQUEST` and wait for every result
  to be sent.
- Not offered: reprocessing (input streams), offline sessions, the torch,
  concurrent cameras, injection sessions and vendor tags.

## Camera permission and the test pattern

The first open of a session asks macOS for camera access
(`requestAccessForMediaType:`), so TCC shows its prompt, attributed to the
app responsible for the process: the terminal `guest-init` was started
from. Nothing changes system settings. Until access is granted (and while
it is denied or restricted), and whenever AVFoundation reports the camera
suspended (a MacBook with its lid closed), the session streams a **test
pattern** instead: eight vertical color bars (white, yellow, cyan, green,
magenta, red, blue, black) with a white band that moves down each frame, at
the requested size and rate. The host log says which source a session
uses, for example:

```text
camera: asking macOS for camera access (the TCC prompt, attributed to the terminal)
camera: FaceTime HD 카메라: camera access is not granted yet; streaming a test pattern (1920x1080 at 30 fps)
```

and the HAL logs `capturing 1920x1080 from a test pattern` to logcat. The
devices are listed either way. A session opened after access is granted
captures from the camera.

## Image

`image/overlay.toml` adds the service, its `.rc` (user `cameraserver`) and
its vintf fragment (`ICameraProvider/internal/0` V1, accepted by the target
FCM level 8, `1-2`). The emulator's ranchu and Google camera providers were
removed with #187. The emulator's feature files that claim what a Mac camera
cannot do (`camera.full`, `camera.raw`, `camera.flash-autofocus`,
`camera.concurrent`) are removed too; `camera.front` stays.

## Verified (2026-09-28, MacBook Pro M2 Pro, macOS 27, lid closed)

- **Unit tests.** `cargo test -p darwin-host-camera` (13): cropping,
  bilinear scaling, RGBA and NV12/NV21 packing of known colors, the test
  pattern, JPEG encoding with the EXIF orientation, a pattern session into
  RGBA, NV12 and JPEG outputs, argument checks, and the camera list (the
  FaceTime HD camera, suspended, with the sizes above; access not
  determined). The HAL's tests (10), built for `aarch64-linux-android` and
  run under `linux-run`: `camera_metadata_t` packing and parsing,
  characteristics, templates and results, the RGBX override, gralloc
  outputs and the JPEG trailer on a memfd buffer. `darwin-gralloc` (9)
  with the BLOB width.
- **Full boot** as in [boot-status.md](boot-status.md) (release build, first
  boot, `--exclude bootanim`, darwin-display 1080x1920): `sys.boot_completed`
  about 50 s after guest-init started; `init.svc.vendor.camera-provider-darwin`
  and `init.svc.cameraserver` running.
- **`dumpsys media.camera`:** one device, `Camera Provider HAL
  android.hardware.camera.provider.ICameraProvider/internal/0-0`, `Camera
  HAL device device@1.1/internal/0`, facing front, 67 static entries,
  hardware level LIMITED.
- **Capture through the original cameraserver:** `camera-client` (NDK
  `ACameraManager`, run from `/data/local/tmp` as a native client) sees
  camera `"0"` (front, LIMITED, JPEG 1920x1080, 1280x720, 640x480), streams
  30 preview frames into a 640x480 YUV reader and a GPU-sampled
  (`IMPLEMENTATION_DEFINED`) reader, and takes a 1920x1080 JPEG with a YUV
  frame of the same timestamp. On the test pattern (no camera access
  granted; the lid was closed):
  - once the first boot's apps had settled (about 4 minutes), the preview
    ran at 29.7 fps with the sensor timestamps 6.5–7.5 ms behind
    CLOCK_BOOTTIME (three runs); in the first minutes after boot, with the
    host busy, 15–27 fps and 40–90 ms; timestamps increase strictly;
  - the GPU reader's buffers are `RGBX_8888` with the pattern's colors
    (yellow `255,255,0` at the left edge, blue at the right, the white band
    where it was);
  - the JPEG (about 49 KB) decodes to the pattern with correct colors; the
    YUV frame's luma at the edges is 226 (yellow) and 29 (blue).
- **TCC:** tccd logged `AUTHREQ_PROMPTING ... service=kTCCServiceCamera,
  subject=Sub:{com.mitchellh.ghostty}` for each boot's first session: the
  prompt, attributed to the terminal. It was not answered during the runs.

## Gaps

- The image's Camera2 app opens the camera (LIMITED_YUV, 1920x1080 preview
  and picture streams) and cameraserver counts 30 preview frames a second
  into its SurfaceTexture, but its preview stays black: the TextureView
  never shows the camera buffers. The buffers are correct (the NDK client's
  GPU reader), so the fault is in consuming them (HWUI's layer update over
  the GLES driver).
- Capture from the real camera is untested here: access was not granted and
  the lid was closed. The capture path is the same after `FN_FRAME` (a
  camera frame replaces the pattern frame); only the delegate and the
  format selection are unexercised.
- The frame rate depends on the system's load: every frame is a host call
  plus two binder calls to cameraserver, and cameraserver's result handling
  returns the buffers to the consumer's process before the call returns.
- The host module has no hot-plug: cameras connected after start-up are not
  listed.
