# Chromium Android acceptance

This gate runs an official, unmodified arm64 `chrome_public_apk` through the
AIM compatibility runtime. It does not inject JavaScript into Chromium
or replace its Android Activity, Service, Binder, renderer, GPU, file, media,
input, or TLS paths.

The fixture is served from a local HTTPS origin whose certificate chains to
the current macOS user's trusted mkcert root. Chromium must navigate via real
Android pointer and physical-key events. A second run restores that tab and
drives pointer, keyboard, download, Android document picker, WebM video/Opus
audio, `window.open`, and WebGL. The HTTPS origin records page-side results;
the gate also checks downloaded bytes, renderer/GPU Android Services, Binder
FD transport, CoreAudio output, absence of `--single-process`, and captures the
final screen.

Run after the normal graphics/runtime build:

```sh
./tools/chromium-android-acceptance/run.sh /path/to/chrome_public_apk.apk
```

Artifacts are written beneath `_build/chromium-android-acceptance/`.
