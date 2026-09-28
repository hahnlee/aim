#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"; data="$(mktemp -d "${TMPDIR:-/tmp}/aim-keystore-cross.XXXXXX")"
classes="$root/_build/android16-framework-compat/classes"; harness="$root/tools/android-framework-compat/tests/AndroidKeyStoreCrossProcess.java"
trap 'rm -rf "$data" "$classes/AndroidKeyStoreCrossProcess.class"' EXIT
[[ -d "$classes" ]] || bash "$root/tools/build-android16-framework-compat.sh" >/dev/null
javac --release 8 -encoding UTF-8 -cp "$classes" -d "$classes" "$harness"
AIM_APK_APP_DATA_DIR="$data" java -cp "$classes" AndroidKeyStoreCrossProcess writer >"$data/writer.log"
AIM_APK_APP_DATA_DIR="$data" java -cp "$classes" AndroidKeyStoreCrossProcess reader >"$data/reader.log"
grep -q 'writer alias=true' "$data/writer.log"; grep -q '^reader mac=[0-9a-f]\{64\}$' "$data/reader.log"; [[ ! -s "$data/keystore/android-keystore-hmac-v1" ]]
echo "android-keystore-cross-process-acceptance: PASS"; cat "$data/writer.log" "$data/reader.log"
