#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
out="$root/_build/trust-manager-endpoint-test"

# Keep the endpoint tied to the Android 16 framework actually shipped by this
# runtime. The hidden ITrustManager interface is not present in android.jar.
source "$root/upstream/android16-trust-manager.lock"
framework="$root/$FRAMEWORK_TRUST_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_TRUST_SHA256"
test -x "$apkanalyzer"
packages="$out/framework-packages.txt"
mkdir -p "$out"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "$ITRUST_MANAGER_DESCRIPTOR $IS_DEVICE_LOCKED_SIGNATURE" "$packages"
grep -Fq "$ITRUST_MANAGER_DESCRIPTOR $IS_DEVICE_SECURE_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
"$apkanalyzer" dex code \
  --class 'android.app.trust.ITrustManager$Stub' \
  --method 'getDefaultTransactionName(I)Ljava/lang/String;' "$framework" \
  > "$transaction_names"
transaction_for() {
  local method="$1" target
  target="$(awk -v wanted="\"$method\"" '
    /^[[:space:]]*:pswitch_/ { label=$1 }
    ($1 == "const-string" || $1 == "const-string/jumbo") && $3 == wanted {
      print label; exit
    }
  ' "$transaction_names")"
  awk -v target="$target" '
    /^[[:space:]]*\.packed-switch 0x1/ { inside=1; number=1; next }
    inside && /^[[:space:]]*:pswitch_/ {
      if ($1 == target) { print number; exit }
      number++
    }
  ' "$transaction_names"
}
test "$(transaction_for isDeviceLocked)" = "$IS_DEVICE_LOCKED_TRANSACTION"
test "$(transaction_for isDeviceSecure)" = "$IS_DEVICE_SECURE_TRANSACTION"

rm -rf "$out"
mkdir -p "$out"
javac --release 8 -encoding UTF-8 -d "$out" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/runtime/framework/trust/TrustManagerEndpoint.java" \
  "$root/tools/tests/trust/TrustManagerEndpointTest.java"
java -ea -cp "$out" dev.darwinart.runtime.trust.TrustManagerEndpointTest
