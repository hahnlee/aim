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
grep -Fq "$IBATTERY_PROPERTIES_REGISTRAR_DESCRIPTOR $GET_PROPERTY_SIGNATURE" "$packages"
grep -Fq "$IBATTERY_PROPERTIES_REGISTRAR_DESCRIPTOR $SCHEDULE_UPDATE_SIGNATURE" "$packages"
grep -Fq "$IBATTERY_STATS_DESCRIPTOR $IS_CHARGING_SIGNATURE" "$packages"
grep -Fq "$IBATTERY_STATS_DESCRIPTOR $COMPUTE_BATTERY_TIME_REMAINING_SIGNATURE" "$packages"
grep -Fq "$IBATTERY_STATS_DESCRIPTOR $COMPUTE_CHARGE_TIME_REMAINING_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
stub_transactions() {
  "$apkanalyzer" dex code --class "$1" \
    --method 'getDefaultTransactionName(I)Ljava/lang/String;' "$framework" \
    > "$transaction_names"
}
stub_transactions 'android.os.IPowerManager$Stub'
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
stub_transactions 'android.os.IBatteryPropertiesRegistrar$Stub'
test "$(transaction_for getProperty)" = "$GET_PROPERTY_TRANSACTION"
test "$(transaction_for scheduleUpdate)" = "$SCHEDULE_UPDATE_TRANSACTION"
stub_transactions 'com.android.internal.app.IBatteryStats$Stub'
test "$(transaction_for isCharging)" = "$IS_CHARGING_TRANSACTION"
test "$(transaction_for computeBatteryTimeRemaining)" = "$COMPUTE_BATTERY_TIME_REMAINING_TRANSACTION"
test "$(transaction_for computeChargeTimeRemaining)" = "$COMPUTE_CHARGE_TIME_REMAINING_TRANSACTION"

rm -rf "$out"
mkdir -p "$out"
javac --release 8 -encoding UTF-8 -d "$out" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcelable.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/runtime/framework/power/PowerStateProvider.java" \
  "$root/runtime/framework/power/DarwinPowerStateProvider.java" \
  "$root/runtime/framework/power/PowerManagerEndpoint.java" \
  "$root/runtime/framework/power/BatteryHealth.java" \
  "$root/runtime/framework/power/BatteryPropertiesRegistrarEndpoint.java" \
  "$root/runtime/framework/power/BatteryStatsEndpoint.java" \
  "$root/tools/tests/power/PowerManagerEndpointTest.java" \
  "$root/tools/tests/power/BatteryEndpointsTest.java"
java -ea -cp "$out" dev.darwinart.runtime.power.PowerManagerEndpointTest
java -ea -cp "$out" dev.darwinart.runtime.power.BatteryEndpointsTest
