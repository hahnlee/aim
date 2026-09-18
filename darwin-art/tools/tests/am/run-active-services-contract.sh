#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
out="$root/_build/active-services-contract-test"

source "$root/upstream/android16-active-services.lock"
framework="$root/$FRAMEWORK_ACTIVE_SERVICES_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = \
  "$FRAMEWORK_ACTIVE_SERVICES_SHA256"
mkdir -p "$out"
packages="$out/framework-packages.txt"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "android.app.IActivityManager $PUBLISH_SERVICE_SIGNATURE" "$packages"
grep -Fq "android.app.IActivityManager $SERVICE_DONE_EXECUTING_SIGNATURE" "$packages"
grep -Fq "android.app.IActivityManager $UNBIND_FINISHED_SIGNATURE" "$packages"

names="$out/transaction-names.smali"
"$apkanalyzer" dex code --class 'android.app.IActivityManager$Stub' \
  --method 'getDefaultTransactionName(I)Ljava/lang/String;' "$framework" > "$names"
transaction_for() {
  local method="$1" target
  target="$(awk -v wanted="\"$method\"" '
    /^[[:space:]]*:pswitch_/ { label=$1 }
    ($1 == "const-string" || $1 == "const-string/jumbo") && $3 == wanted {
      print label; exit
    }
  ' "$names")"
  awk -v target="$target" '
    /^[[:space:]]*\.packed-switch 0x1/ { inside=1; number=1; next }
    inside && /^[[:space:]]*:pswitch_/ {
      if ($1 == target) { print number; exit }
      number++
    }
  ' "$names"
}
test "$(transaction_for publishService)" = "$PUBLISH_SERVICE_TRANSACTION"
test "$(transaction_for serviceDoneExecuting)" = "$SERVICE_DONE_EXECUTING_TRANSACTION"
test "$(transaction_for unbindFinished)" = "$UNBIND_FINISHED_TRANSACTION"

# Assert the pinned wire contract, not just a positive-PID Java fixture.
# The framework sends no reply Parcel and FLAG_ONEWAY; Binder reports PID 0.
done_proxy="$out/service-done-executing-proxy.smali"
"$apkanalyzer" dex code --class 'android.app.IActivityManager$Stub$Proxy' \
  --method 'serviceDoneExecuting(Landroid/os/IBinder;IIILandroid/content/Intent;)V' \
  "$framework" > "$done_proxy"
rg -Fq 'const/4 v2, 0x0' "$done_proxy"
rg -Fq 'const/4 v3, 0x1' "$done_proxy"
rg -Fq 'const/16 v4, 0x3f' "$done_proxy"
rg -Fq 'invoke-interface {v1, v4, v0, v2, v3}, Landroid/os/IBinder;->transact(ILandroid/os/Parcel;Landroid/os/Parcel;I)Z' "$done_proxy"

endpoint="$root/runtime/framework/am/ActivityManagerEndpoint.java"
grep -Fq 'transaction("serviceDoneExecuting")' "$endpoint"
grep -Fq 'transaction("unbindFinished")' "$endpoint"
grep -Fq 'code != serviceDoneExecutingCode' "$endpoint"
echo "active-services-contract: PASS"
