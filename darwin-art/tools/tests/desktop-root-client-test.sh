#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/desktop-root-client.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/IBinder.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/RemoteException.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/Binder.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/Parcel.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/Looper.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/Handler.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/ServiceManager.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/os/SystemClock.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/util/Log.java" \
  "$root/tools/tests/wm/desktop-root-client-stubs/android/view/InputChannel.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/wm/DesktopRootProtocol.java" \
  "$root/runtime/framework/wm/DesktopRootFocusDecision.java" \
  "$root/runtime/framework/wm/DesktopRootFocusDecisionTransport.java" \
  "$root/runtime/framework/wm/DesktopRootKeyDecisionNative.java" \
  "$root/runtime/framework/wm/DesktopRootFocusDecisionClient.java" \
  "$root/runtime/framework/wm/DesktopRootClient.java" \
  "$root/tools/tests/wm/DesktopRootClientTest.java"
java -cp "$stage" dev.darwinart.runtime.wm.DesktopRootClientTest
