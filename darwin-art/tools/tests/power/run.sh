#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
out="$root/_build/power-manager-endpoint-test"

# Keep the endpoint tied to the Android 16 framework actually shipped by this
# runtime. IPowerManager is hidden and is not present in android.jar.
source "$root/upstream/android16-power-manager.lock"
framework="$root/$FRAMEWORK_POWER_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_POWER_SHA256"
test -x "$apkanalyzer"
packages="$out/framework-packages.txt"
mkdir -p "$out"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "$IPOWER_MANAGER_DESCRIPTOR $IS_INTERACTIVE_SIGNATURE" "$packages"
grep -Fq "$IPOWER_MANAGER_DESCRIPTOR $IS_DISPLAY_INTERACTIVE_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
"$apkanalyzer" dex code \
  --class 'android.os.IPowerManager$Stub' \
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
test "$(transaction_for isInteractive)" = "$IS_INTERACTIVE_TRANSACTION"
test "$(transaction_for isDisplayInteractive)" = "$IS_DISPLAY_INTERACTIVE_TRANSACTION"

rm -rf "$out"
mkdir -p "$out"
javac --release 8 -encoding UTF-8 -d "$out" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/runtime/framework/power/PowerStateProvider.java" \
  "$root/runtime/framework/power/DarwinPowerStateProvider.java" \
  "$root/runtime/framework/power/PowerManagerEndpoint.java" \
  "$root/tools/tests/power/PowerManagerEndpointTest.java"
java -ea -cp "$out" dev.darwinart.runtime.power.PowerManagerEndpointTest
