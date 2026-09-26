#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
out="$root/_build/usage-stats-endpoint-test"

source "$root/upstream/android16-usage-stats.lock"
framework="$root/$FRAMEWORK_USAGE_STATS_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_USAGE_STATS_SHA256"
test -x "$apkanalyzer"
grep -Fq "services.put(\"$USAGE_STATS_SERVICE_NAME\", new UsageStatsManagerEndpoint());" \
  "$root/runtime/framework/system/SystemServiceFactory.java"
packages="$out/framework-packages.txt"
mkdir -p "$out"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "$IUSAGE_STATS_MANAGER_DESCRIPTOR $GET_APP_STANDBY_BUCKET_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
"$apkanalyzer" dex code \
  --class 'android.app.usage.IUsageStatsManager$Stub' \
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
test "$(transaction_for getAppStandbyBucket)" = "$GET_APP_STANDBY_BUCKET_TRANSACTION"

rm -rf "$out/classes"
mkdir -p "$out/classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$out/classes" \
  "$root/tools/tests/connectivity/stubs/android/os/Binder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/IBinder.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcel.java" \
  "$root/tools/tests/connectivity/stubs/android/os/Parcelable.java" \
  "$root/tools/tests/connectivity/stubs/android/os/RemoteException.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/usage/UsageStatsManagerEndpoint.java" \
  "$root/tools/tests/usagestats/UsageStatsManagerEndpointTest.java"
"$java_home/bin/java" -ea -cp "$out/classes" \
  dev.darwinart.runtime.usage.UsageStatsManagerEndpointTest
echo "usage-stats-contract: PASS"
