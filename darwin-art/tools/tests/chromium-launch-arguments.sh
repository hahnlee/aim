#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/chromium-launch-arguments.sh"

stage="$(mktemp -d "${TMPDIR:-/tmp}/chromium-launch-arguments.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

# The caller's switches and their spelling/order remain intact; only missing
# standalone-runtime defaults are appended.
input='--custom=$(touch '
input+="$stage/not-executed"
input+=') --no-first-run --disable-fre --disable-background-networking'
output="$(darwin_art_configure_chromium_launch_arguments "$input")"
[[ "$output" == "$input --enable-skia-graphite --skia-graphite-dawn-backend=vulkan" ]]
[[ ! -e "$stage/not-executed" ]]

# An empty caller payload receives the complete standalone-runtime policy.
output="$(darwin_art_configure_chromium_launch_arguments '')"
[[ "$output" == '--no-first-run --disable-fre --disable-background-networking --enable-skia-graphite --skia-graphite-dawn-backend=vulkan' ]]

# Existing first-run and Vulkan switches are not duplicated, and unrelated
# caller arguments are retained.
input='--user-data-dir=/data/chrome --skia-graphite-dawn-backend=vulkan --enable-skia-graphite --disable-fre --no-first-run --disable-background-networking'
output="$(darwin_art_configure_chromium_launch_arguments "$input")"
[[ "$output" == "$input" ]]

for invalid in \
  '--skia-graphite-dawn-backend=gl' \
  '--skia-graphite-dawn-backend=metal' \
  '--disable-skia-graphite' \
  '--disable-skia-graphite=true' \
  '--disable-gpu' \
  '--disable-gpu=true' \
  '--enable-skia-graphite=false' \
  '--enable-skia-graphite=0' \
  '--disable-features=SkiaGraphite' \
  '--disable-features=Foo,SkiaGraphite,Bar'; do
  if darwin_art_configure_chromium_launch_arguments "$invalid" >/dev/null; then
    echo "contradictory Chromium switch unexpectedly accepted: $invalid" >&2
    exit 1
  fi
done

if darwin_art_configure_chromium_launch_arguments '--skia-graphite-dawn-backend' >/dev/null; then
  echo 'incomplete Chromium Graphite backend switch unexpectedly accepted' >&2
  exit 1
fi

if darwin_art_configure_chromium_launch_arguments >/dev/null; then
  echo 'missing Chromium command-line argument unexpectedly accepted' >&2
  exit 1
fi

echo 'Chromium launch arguments: literal preservation, first-run defaults, Vulkan Graphite enforcement PASS'
