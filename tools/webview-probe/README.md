# Original Android WebView consumer probe

This ordinary SDK application creates the installed original Android WebView,
shows local HTML and SVG, and verifies its JavaScript DOM marker. The Activity
logs the actual provider package/version and renderer handle. It does not replace
a provider or modify original APKs, the image, or native libraries.

Use the Java tools already fetched by `cargo aim` (docs/build.md, "Java") and
an existing Android SDK 36 platform jar:

```sh
python3 tools/webview-probe/build.py \
  --android-jar ~/Library/Android/sdk/platforms/android-36/android.jar
```

The script reads versions from `upstream/java-toolchain.lock`; it does not fetch
inputs. APK, classes, SHA256SUMS and a separate disposable signing key are generated
under `target/aim/webview-probe`. Generated files are not repository sources.

Copy the APK to a disposable guest and use its original package commands:

```sh
pm install /data/local/tmp/webview-probe.apk
am start -W -n dev.aim.webviewprobe/.WebViewProbeActivity
logcat -d -s AIMWebViewProbe
```

Expected records are START, PROVIDER, PAGE_FINISHED, RENDERER, and DOM_PASS with
`AIM WEBVIEW ORIGINAL PROVIDER`. Check the visible green HTML and SVG on an actual
screenshot as well; a DOM callback alone does not prove rendering.
PROVIDER_INIT_FAIL, PAGE_FAIL, DOM_FAIL and RENDERER_GONE indicate failure.

The original image provides compressed WebViewGoogle and TrichromeLibrary APKs
alongside their factory stubs. Its genuine compressed-package owner and original
provider loading path must supply them; this probe does not waive relro or change
provider selection or signatures.
