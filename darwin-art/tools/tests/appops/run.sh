#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
out="$root/_build/appops-endpoint-test"

source "$root/upstream/android16-appops-service.lock"
framework="$root/$FRAMEWORK_APPOPS_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_APPOPS_SHA256"
test -x "$apkanalyzer"
grep -Fq "services.put(\"$APPOPS_SERVICE_NAME\", new AppOpsServiceEndpoint());" \
  "$root/runtime/framework/system/SystemServiceFactory.java"
mkdir -p "$out"
packages="$out/framework-packages.txt"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "$IAPPOPS_SERVICE_DESCRIPTOR $CHECK_OPERATION_FOR_DEVICE_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
"$apkanalyzer" dex code \
  --class 'com.android.internal.app.IAppOpsService$Stub' \
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
test "$(transaction_for checkOperationForDevice)" = "$CHECK_OPERATION_FOR_DEVICE_TRANSACTION"

rm -rf "$out/classes"
mkdir -p "$out/classes"
javac --release 8 -encoding UTF-8 -d "$out/classes" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/runtime/framework/appops/AppOpsServiceEndpoint.java" \
  "$root/tools/tests/appops/AppOpsServiceEndpointTest.java"
java -ea -cp "$out/classes" dev.darwinart.runtime.appops.AppOpsServiceEndpointTest
