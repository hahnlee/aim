#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/desktop-root-endpoint.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root/tools/tests/wm/desktop-root-stubs/android/os/IBinder.java" \
  "$root/tools/tests/wm/desktop-root-stubs/android/os/RemoteException.java" \
  "$root/tools/tests/wm/desktop-root-stubs/android/os/Binder.java" \
  "$root/tools/tests/wm/desktop-root-stubs/android/os/Parcel.java" \
  "$root/tools/tests/wm/desktop-root-stubs/android/os/Process.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/wm/WindowSessionIdentity.java" \
  "$root/runtime/framework/wm/DesktopRootWindowBindingOwner.java" \
  "$root/runtime/framework/wm/DesktopRootRegistry.java" \
  "$root/runtime/framework/wm/DesktopForegroundAuthority.java" \
  "$root/runtime/framework/wm/DesktopRootEndpoint.java" \
  "$root/tools/tests/wm/DesktopRootEndpointTest.java"
java -cp "$stage" dev.darwinart.runtime.wm.DesktopRootEndpointTest
