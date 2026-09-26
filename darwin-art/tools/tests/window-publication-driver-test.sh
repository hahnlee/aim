#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/window-publication-driver.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM

# Compile the actual WMS registry/delivery/endpoint/driver/controller.  The
# wm-publication tree supplies only deterministic Android/JNI-port seams and
# minimal package-local identity/ownership fixtures; it is never a runtime
# input.
javac --release 8 -encoding UTF-8 -d "$stage" \
  $(find "$root/tools/tests/wm-publication/src" -name '*.java' -print) \
  "$root/tools/tests/wm/desktop-root-stubs/android/os/Parcel.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/wm/DesktopRootFocusDecision.java" \
  "$root/runtime/framework/wm/DesktopRootFocusDecisionTransport.java" \
  "$root/runtime/framework/wm/WindowRootFocusDecisionDelivery.java" \
  "$root/runtime/framework/wm/WindowRootFocusDecisions.java" \
  "$root/runtime/framework/wm/WindowFocusRegistry.java" \
  "$root/runtime/framework/wm/DesktopRootWindowBindingOwner.java" \
  "$root/runtime/framework/wm/DesktopRootRegistry.java" \
  "$root/runtime/framework/wm/DesktopForegroundAuthority.java" \
  "$root/runtime/framework/wm/WindowFocusPublicationDelivery.java" \
  "$root/runtime/framework/wm/WindowInputEndpoint.java" \
  "$root/runtime/framework/wm/WindowPublicationDriver.java" \
  "$root/runtime/framework/wm/WindowRootActivationPolicy.java" \
  $(find "$root/tools/tests/wm/window-id-stubs" -name '*.java' -print) \
  "$root/runtime/framework/wm/WindowIdRegistry.java" \
  "$root/runtime/framework/wm/WindowPublicationController.java"
java -ea -cp "$stage" dev.darwinart.runtime.wm.WindowPublicationDriverTest
echo 'WMS publication driver/controller: real production classes with deterministic fixture seam PASS'
