#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
out="$root/_build/thermal-service-endpoint-test"

# Keep the endpoint tied to the Android 16 framework actually shipped by this
# runtime. IThermalService is hidden and is not present in android.jar.
source "$root/upstream/android16-thermal-service.lock"
framework="$root/$FRAMEWORK_THERMAL_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_THERMAL_SHA256"
test -x "$apkanalyzer"
packages="$out/framework-packages.txt"
mkdir -p "$out"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_CURRENT_THERMAL_STATUS_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_THERMAL_HEADROOM_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $REGISTER_THERMAL_EVENT_LISTENER_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $REGISTER_THERMAL_EVENT_LISTENER_WITH_TYPE_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $UNREGISTER_THERMAL_EVENT_LISTENER_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_CURRENT_TEMPERATURES_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_CURRENT_TEMPERATURES_WITH_TYPE_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $REGISTER_THERMAL_STATUS_LISTENER_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $UNREGISTER_THERMAL_STATUS_LISTENER_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_CURRENT_COOLING_DEVICES_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_CURRENT_COOLING_DEVICES_WITH_TYPE_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $GET_THERMAL_HEADROOM_THRESHOLDS_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $REGISTER_THERMAL_HEADROOM_LISTENER_SIGNATURE" "$packages"
grep -Fq "$ITHERMAL_SERVICE_DESCRIPTOR $UNREGISTER_THERMAL_HEADROOM_LISTENER_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
"$apkanalyzer" dex code \
  --class 'android.os.IThermalService$Stub' \
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
test "$(transaction_for getCurrentThermalStatus)" = "$GET_CURRENT_THERMAL_STATUS_TRANSACTION"
test "$(transaction_for getThermalHeadroom)" = "$GET_THERMAL_HEADROOM_TRANSACTION"
test "$(transaction_for getThermalHeadroomThresholds)" = "$GET_THERMAL_HEADROOM_THRESHOLDS_TRANSACTION"
test "$(transaction_for registerThermalEventListener)" = "$REGISTER_THERMAL_EVENT_LISTENER_TRANSACTION"
test "$(transaction_for registerThermalEventListenerWithType)" = "$REGISTER_THERMAL_EVENT_LISTENER_WITH_TYPE_TRANSACTION"
test "$(transaction_for unregisterThermalEventListener)" = "$UNREGISTER_THERMAL_EVENT_LISTENER_TRANSACTION"
test "$(transaction_for getCurrentTemperatures)" = "$GET_CURRENT_TEMPERATURES_TRANSACTION"
test "$(transaction_for getCurrentTemperaturesWithType)" = "$GET_CURRENT_TEMPERATURES_WITH_TYPE_TRANSACTION"
test "$(transaction_for registerThermalStatusListener)" = "$REGISTER_THERMAL_STATUS_LISTENER_TRANSACTION"
test "$(transaction_for unregisterThermalStatusListener)" = "$UNREGISTER_THERMAL_STATUS_LISTENER_TRANSACTION"
test "$(transaction_for getCurrentCoolingDevices)" = "$GET_CURRENT_COOLING_DEVICES_TRANSACTION"
test "$(transaction_for getCurrentCoolingDevicesWithType)" = "$GET_CURRENT_COOLING_DEVICES_WITH_TYPE_TRANSACTION"
test "$(transaction_for registerThermalHeadroomListener)" = "$REGISTER_THERMAL_HEADROOM_LISTENER_TRANSACTION"
test "$(transaction_for unregisterThermalHeadroomListener)" = "$UNREGISTER_THERMAL_HEADROOM_LISTENER_TRANSACTION"

rm -rf "$out"
mkdir -p "$out"
javac --release 8 -encoding UTF-8 -d "$out" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Handler.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Looper.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcelable.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/power/ThermalServiceEndpoint.java" \
  "$root/tools/tests/power/ThermalServiceEndpointTest.java"
java -ea -cp "$out" dev.darwinart.runtime.power.ThermalServiceEndpointTest
