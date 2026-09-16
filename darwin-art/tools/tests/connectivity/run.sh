#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-connectivity-endpoint.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

# shellcheck source=/dev/null
source "$root/upstream/android16-connectivity-manager.lock"
connectivity_jar="$root/$FRAMEWORK_CONNECTIVITY_PATH"

actual_sha="$(shasum -a 256 "$connectivity_jar" | awk '{print $1}')"
test "$actual_sha" = "$FRAMEWORK_CONNECTIVITY_SHA256"

"$apkanalyzer" dex packages --defined-only "$connectivity_jar" \
  > "$tmp/packages.txt"
for signature in \
  "$GET_ACTIVE_NETWORK_SIGNATURE" \
  "$GET_ACTIVE_NETWORK_INFO_SIGNATURE" \
  "$GET_ALL_NETWORKS_SIGNATURE" \
  "$GET_LINK_PROPERTIES_SIGNATURE" \
  "$GET_NETWORK_CAPABILITIES_SIGNATURE" \
  "$IS_ACTIVE_NETWORK_METERED_SIGNATURE" \
  "$REQUEST_NETWORK_SIGNATURE" \
  "$LISTEN_FOR_NETWORK_SIGNATURE" \
  "$RELEASE_NETWORK_REQUEST_SIGNATURE"; do
  grep -Fq "android.net.IConnectivityManager $signature" "$tmp/packages.txt"
done

"$apkanalyzer" dex code \
  --class 'android.net.IConnectivityManager$Stub' \
  --method 'getDefaultTransactionName(I)Ljava/lang/String;' \
  "$connectivity_jar" > "$tmp/transaction-names.smali"
transaction_for() {
  local method="$1" target
  target="$(awk -v wanted="\"$method\"" '
    /^[[:space:]]*:pswitch_/ { label=$1 }
    ($1 == "const-string" || $1 == "const-string/jumbo") && $3 == wanted {
      print label; exit
    }
  ' "$tmp/transaction-names.smali")"
  test -n "$target"
  awk -v target="$target" '
    /^[[:space:]]*\.packed-switch 0x1/ { inside=1; number=1; next }
    inside && /^[[:space:]]*:pswitch_/ {
      if ($1 == target) { print number; exit }
      number++
    }
  ' "$tmp/transaction-names.smali"
}
test "$(transaction_for getActiveNetwork)" = "$GET_ACTIVE_NETWORK_TRANSACTION"
test "$(transaction_for getActiveNetworkInfo)" = "$GET_ACTIVE_NETWORK_INFO_TRANSACTION"
test "$(transaction_for getAllNetworks)" = "$GET_ALL_NETWORKS_TRANSACTION"
test "$(transaction_for getLinkProperties)" = "$GET_LINK_PROPERTIES_TRANSACTION"
test "$(transaction_for getNetworkCapabilities)" = "$GET_NETWORK_CAPABILITIES_TRANSACTION"
test "$(transaction_for isActiveNetworkMetered)" = "$IS_ACTIVE_NETWORK_METERED_TRANSACTION"
test "$(transaction_for requestNetwork)" = "$REQUEST_NETWORK_TRANSACTION"
test "$(transaction_for listenForNetwork)" = "$LISTEN_FOR_NETWORK_TRANSACTION"
test "$(transaction_for releaseNetworkRequest)" = "$RELEASE_NETWORK_REQUEST_TRANSACTION"

"$apkanalyzer" dex code \
  --class 'android.net.IConnectivityManager$Stub' \
  --method 'onTransact(ILandroid/os/Parcel;Landroid/os/Parcel;I)Z' \
  "$connectivity_jar" > "$tmp/on-transact.smali"
grep -Fq "const-string v15, \"$ICONNECTIVITY_MANAGER_DESCRIPTOR\"" \
  "$tmp/on-transact.smali"
for invocation in \
  'getActiveNetwork()Landroid/net/Network;' \
  'getActiveNetworkInfo()Landroid/net/NetworkInfo;' \
  'getAllNetworks()[Landroid/net/Network;' \
  'getLinkProperties(Landroid/net/Network;)Landroid/net/LinkProperties;' \
  'getNetworkCapabilities(Landroid/net/Network;Ljava/lang/String;Ljava/lang/String;)Landroid/net/NetworkCapabilities;' \
  'isActiveNetworkMetered()Z'; do
  grep -Fq "Landroid/net/IConnectivityManager\$Stub;->$invocation" "$tmp/on-transact.smali"
done
for invocation in \
  'requestNetwork(ILandroid/net/NetworkCapabilities;ILandroid/os/Messenger;ILandroid/os/IBinder;IILjava/lang/String;Ljava/lang/String;I)Landroid/net/NetworkRequest;' \
  'listenForNetwork(Landroid/net/NetworkCapabilities;Landroid/os/Messenger;Landroid/os/IBinder;ILjava/lang/String;Ljava/lang/String;I)Landroid/net/NetworkRequest;' \
  'releaseNetworkRequest(Landroid/net/NetworkRequest;)V'; do
  grep -Fq "Landroid/net/IConnectivityManager\$Stub;->$invocation" "$tmp/on-transact.smali"
done

mkdir -p "$tmp/classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$tmp/classes" \
  $(find "$root/tools/tests/connectivity/stubs" -name '*.java' -print) \
  "$root/runtime/framework/connectivity/ConnectivityState.java" \
  "$root/runtime/framework/connectivity/ConnectivitySnapshot.java" \
  "$root/runtime/framework/connectivity/ConnectivityStateOwner.java" \
  "$root/runtime/framework/connectivity/NetworkProbeTransport.java" \
  "$root/runtime/framework/connectivity/HttpNetworkProbeTransport.java" \
  "$root/runtime/framework/connectivity/NetworkValidationMonitor.java" \
  "$root/runtime/framework/connectivity/ConnectivityServiceState.java" \
  "$root/runtime/framework/connectivity/ConnectivityCallbackRegistry.java" \
  "$root/runtime/framework/connectivity/ConnectivityProjection.java" \
  "$root/runtime/framework/connectivity/ConnectivityPermissionEnforcer.java" \
  "$root/runtime/framework/connectivity/ConnectivityManagerEndpoint.java" \
  "$root/tools/tests/connectivity/ConnectivityManagerEndpointTest.java"
"$java_home/bin/java" -ea -cp "$tmp/classes" \
  dev.darwinart.runtime.connectivity.ConnectivityManagerEndpointTest
printf 'connectivity-aosp-lock: PASS (%s transactions 1,3,9,14,16,20,42,45,47)\n' \
  "$ICONNECTIVITY_MANAGER_DESCRIPTOR"
