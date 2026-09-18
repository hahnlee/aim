#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/window-session-identity.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root/tools/tests/am/stubs/android/os/IBinder.java" \
  "$root/tools/tests/am/stubs/android/os/RemoteException.java" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/wm/WindowSessionIdentity.java" \
  "$root/tools/tests/wm/WindowSessionIdentityTest.java"
java -cp "$stage" dev.darwinart.runtime.wm.WindowSessionIdentityTest
