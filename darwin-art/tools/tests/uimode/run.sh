#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$root/_build/uimode-endpoint-test"

rm -rf "$out"
mkdir -p "$out"
javac --release 8 -encoding UTF-8 -d "$out" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcelable.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/uimode/UiModeManagerEndpoint.java" \
  "$root/tools/tests/uimode/UiModeManagerEndpointTest.java"
java -ea -cp "$out" dev.darwinart.runtime.uimode.UiModeManagerEndpointTest
