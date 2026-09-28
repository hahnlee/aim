#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture_source="$root/tools/chromium-android-acceptance"
apk="${1:-}"
[[ -n "$apk" && -f "$apk" ]] || {
  echo "usage: $0 CHROME_PUBLIC_APK" >&2
  exit 64
}
apk="$(cd "$(dirname "$apk")" && pwd)/$(basename "$apk")"
"$root/tools/materialize-moltenvk.sh" >/dev/null
command -v mkcert >/dev/null || { echo "mkcert is required" >&2; exit 69; }
command -v ffmpeg >/dev/null || { echo "ffmpeg is required" >&2; exit 69; }

mkdir -p "$root/_build/chromium-android-acceptance"
output="$(mktemp -d "$root/_build/chromium-android-acceptance/run.XXXXXX")"
fixture="$output/origin"
app_data="$output/app-data"
mkdir -p "$fixture" "$app_data"
cp "$fixture_source/index.html" "$fixture_source/webgl.html" \
  "$fixture_source/upload-fixture.txt" "$fixture/"
ffmpeg -loglevel error -y -f lavfi -i testsrc2=size=160x90:rate=24 \
  -f lavfi -i sine=frequency=880:sample_rate=48000 -t 4 \
  -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 180k \
  -c:a libopus -b:a 64k "$fixture/media.webm"

port="${AIM_CHROMIUM_ACCEPTANCE_PORT:-$((20000 + RANDOM % 20000))}"
cert="$output/localhost.pem"
key="$output/localhost-key.pem"
trust_preflight="$output/trust-preflight.log"
{
  printf 'preflight=macos-ca-trust\n'
  printf 'user=%s\n' "$(id -un)"
  printf 'caroot=%s\n' "$(mkcert -CAROOT)"
  printf 'verify_command=security verify-cert -c %s -p ssl -s 127.0.0.1\n' "$cert"
} >"$trust_preflight"
mkcert -cert-file "$cert" -key-file "$key" 127.0.0.1 localhost ::1 >>"$trust_preflight" 2>&1
openssl x509 -in "$cert" -noout -fingerprint -sha256 >>"$trust_preflight"
if ! security verify-cert -c "$cert" -p ssl -s 127.0.0.1 >/dev/null 2>&1; then
  {
    printf 'status=BLOCKED_ENV reason=macos_ca_untrusted runtime_started=0\n'
    printf 'verify_rc=1\n'
  } >>"$trust_preflight"
  echo "Chromium acceptance BLOCKED_ENV: generated mkcert root is not trusted by macOS" >&2
  echo "trust-preflight=$trust_preflight" >&2
  exit 69
fi
printf 'status=pass verify_rc=0\n' >>"$trust_preflight"
python3 "$fixture_source/server.py" --directory "$fixture" \
  --report "$output/reports.log" --cert "$cert" --key "$key" --port "$port" \
  >"$output/server.log" 2>&1 &
server_pid=$!
capture_pid=""
cleanup() {
  kill "$server_pid" 2>/dev/null || true
  [[ -z "$capture_pid" ]] || wait "$capture_pid" 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 1 50); do
  kill -0 "$server_pid" 2>/dev/null || {
    echo "HTTPS acceptance origin exited during startup" >&2
    cat "$output/server.log" >&2
    exit 1
  }
  if curl --silent --fail --cacert "$(mkcert -CAROOT)/rootCA.pem" \
      "https://127.0.0.1:$port/index.html" | rg -F 'aim Chromium E2E' >/dev/null; then
    break
  fi
  sleep 0.1
done
curl --silent --fail --cacert "$(mkcert -CAROOT)/rootCA.pem" \
  "https://127.0.0.1:$port/index.html" | rg -F 'aim Chromium E2E' >/dev/null

common_command_line="--no-first-run --disable-fre --disable-background-networking"
if [[ -n "${AIM_CHROMIUM_ACCEPTANCE_EXTRA_SWITCHES:-}" ]]; then
  common_command_line="$common_command_line $AIM_CHROMIUM_ACCEPTANCE_EXTRA_SWITCHES"
fi

env AIM_APP_DATA_ROOT="$app_data" \
  AIM_APP_COMMAND_LINE="$common_command_line" \
  AIM_DEBUG_SECURITY=1 \
  AIM_APK_APP_INTENT_ACTION='android.intent.action.VIEW' \
  AIM_APK_APP_INTENT_URI="https://127.0.0.1:$port/index.html" \
  "$root/tools/run-android-apk-app.sh" "$apk" 16 >"$output/navigation.log" 2>&1

# Restored HTTPS tab: focus the text field, type `hello`, click media, download,
# and file picker in the original tab.
( sleep 26; screencapture -x "$output/final.png" ) &
capture_pid=$!
env AIM_APP_DATA_ROOT="$app_data" \
  AIM_APP_COMMAND_LINE="$common_command_line" \
  AIM_TEST_OPEN_DOCUMENT="$fixture/upload-fixture.txt" \
  AIM_DEBUG_BINDER=1 AIM_DEBUG_SECURITY=1 \
  AIM_TEST_POINTER_SEQUENCE='0,0,0;180,140,10000;180,140,1500' \
  AIM_TEST_KEY_AFTER_POINTER_SEQUENCE='36,33,40,40,43' \
  AIM_TEST_KEY_AFTER_POINTER_DELAY_MS=600 \
  AIM_TEST_KEY_INTERVAL_MS=50 \
  AIM_TEST_POINTER_AFTER_SEQUENCE_DRAG='180,200;180,200' \
  AIM_TEST_POINTER_AFTER_DRAG_SEQUENCE='180,260,500;180,320,500' \
  AIM_TEST_POINTER_HOLD_MS=18 \
  "$root/tools/run-android-apk-app.sh" "$apk" 32 >"$output/interaction.log" 2>&1
wait "$capture_pid"
capture_pid=""

# Re-enter the same Activity through an Android VIEW intent, then follow the
# page's target=_blank link. This is a Chromium tab, never a second NSWindow.
( sleep 15; screencapture -x "$output/webgl.png" ) &
capture_pid=$!
env AIM_APP_DATA_ROOT="$app_data" \
  AIM_APP_COMMAND_LINE="$common_command_line" \
  AIM_APK_APP_INTENT_ACTION='android.intent.action.VIEW' \
  AIM_APK_APP_INTENT_URI="https://127.0.0.1:$port/index.html" \
  AIM_TEST_POINTER_SEQUENCE='0,0,0;180,380,10000;180,380,2000' \
  AIM_TEST_POINTER_HOLD_MS=18 \
  "$root/tools/run-android-apk-app.sh" "$apk" 24 >"$output/webgl.log" 2>&1
wait "$capture_pid"
capture_pid=""

reports="$output/reports.log"
rg -F 'case=e2e status=pass' "$reports" >/dev/null || {
  echo "Chromium page-side E2E result did not pass: $reports" >&2; exit 1;
}
rg -F 'case=webgl' "$reports" | rg -F 'status=pass' >/dev/null || {
  echo "Chromium WebGL result did not pass: $reports" >&2; exit 1;
}
tab_directory="$app_data/org.chromium.chrome/private-data/user/0/org.chromium.chrome/app_tabs/0"
tab_count="$(find "$tab_directory" -maxdepth 1 -type f -name 'flatbufferv1_tab*' | wc -l | tr -d ' ')"
[[ "$tab_count" -ge 2 ]] || {
  echo "Chromium target=_blank did not persist a second Android tab: $tab_directory" >&2
  exit 1
}
download="$app_data/org.chromium.chrome/private-data/user/0/org.chromium.chrome/external/files/downloaded-fixture.txt"
cmp "$fixture/upload-fixture.txt" "$download"
log="$output/interaction.log"
rg -a 'bind isolated Service .*SandboxedProcessService.*instance=0' "$log" >/dev/null
rg -a 'bind isolated Service .*SandboxedProcessService.*instance=1' "$log" >/dev/null
rg -a 'bind remote Service .*PrivilegedProcessService' "$log" >/dev/null
rg -a 'ART Binder parcel: import .* descriptors=[1-9]' "$log" >/dev/null
rg -a 'ART Android AudioTrack: first non-silent PCM write' "$log" >/dev/null
# Chromium's native network stack consumes the macOS trust anchors exported
# by DarwinAndroidCAStore; Java TrustManager.verifyServerChain is not involved.
rg -a 'DARWIN security: exported macOS roots=[1-9][0-9]*' \
  "$output/navigation.log" "$log" "$output/webgl.log" >/dev/null
if rg -a 'DARWIN security: macOS rejected chain' \
    "$output/navigation.log" "$log" "$output/webgl.log" >/dev/null; then
  echo "macOS trust evaluation rejected the local HTTPS certificate" >&2
  exit 1
fi
if rg -a -- '--single-process' "$output/navigation.log" "$log" "$output/webgl.log" >/dev/null; then
  echo "forbidden --single-process flag detected" >&2
  exit 1
fi
if rg -a -- '--disable-features=[^ ]*(SkiaGraphite|EnableDrDc)' \
    "$output/navigation.log" "$log" "$output/webgl.log" >/dev/null; then
  echo "forbidden Chromium graphics compatibility flag detected" >&2
  exit 1
fi
if rg -a 'Fatal signal|libc\+\+abi: terminating' \
    "$output/navigation.log" "$log" "$output/webgl.log" >/dev/null; then
  echo "Chromium or an Android service process crashed during acceptance" >&2
  exit 1
fi
if rg -a 'Could not find SharedImageBackingFactory|Failed to create Dawn context provider' \
    "$output/navigation.log" "$log" "$output/webgl.log" >/dev/null; then
  echo "Chromium GPU service could not satisfy a requested SharedImage/Graphite contract" >&2
  exit 1
fi

echo "chromium-android-acceptance: PASS official-apk=$(shasum -a 256 "$apk" | cut -d' ' -f1) HTTPS=macOS-trust pointer=MotionEvent keyboard=physical tabs=$tab_count renderer=service GPU=service Binder-FD=SCM_RIGHTS download=exact file-picker=content-URI WebGL=ANGLE-Metal media=WebM+Opus+CoreAudio"
echo "chromium-android-acceptance: artifacts=$output"
