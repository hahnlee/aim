#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
runtime_dylib="${1:-$project_root/_build/runtime-link-probe/libaim_runtime.dylib}"
icu_runtime="$project_root/_build/icu-runtime-adapters/runtime"

inputs=(
  "$runtime_dylib"
  "$project_root/_prebuilt/android-16/bootclasspath/core-oj.jar"
  "$project_root/_prebuilt/android-16/bootclasspath/core-libart.jar"
  "$project_root/_prebuilt/android-16/bootclasspath/framework.jar"
  "$project_root/_build/bootclasspath/core-icu4j-apex-reconciled.jar"
  "$project_root/_build/dex-probe/dex/classes.dex"
)

for input in "${inputs[@]}"; do
  if [[ ! -f "$input" ]]; then
    echo "real-graphics-rust-host: missing input: $input" >&2
    exit 2
  fi
done

icu_data="$icu_runtime/i18n/etc/icu/icudt76l.dat"
if [[ ! -f "$icu_data" ]]; then
  echo "real-graphics-rust-host: missing input: $icu_data" >&2
  exit 2
fi

export ANDROID_I18N_ROOT="$icu_runtime/i18n"
export ANDROID_DATA="$icu_runtime/data"
export ANDROID_TZDATA_ROOT="$icu_runtime/tzdata"

cargo build \
  --manifest-path "$project_root/Cargo.toml" \
  -p aim-host \
  --bin real-graphics-acceptance

exec "$project_root/target/debug/real-graphics-acceptance" "${inputs[@]}"
