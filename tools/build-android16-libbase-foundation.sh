#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
build="$project_root/_build/libbase-foundation"
objects="$build/objects"
archive="$build/libandroid-base-darwin.a"
lock_file="$project_root/upstream/android16-libbase-foundation.lock"
source "$lock_file"

"$script_dir/sync-android16-libbase-foundation.sh"
mkdir -p "$objects" "$(dirname "$archive")"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_hash() {
  local path="$1" expected="$2" actual
  [[ -f "$path" ]] || { echo "libbase-foundation: missing $path" >&2; exit 2; }
  actual="$(sha256 "$path")"
  [[ "$actual" == "$expected" ]] || {
    echo "libbase-foundation: identity mismatch: $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  }
}

base="$aosp/system/libbase"
fmt="$aosp/external/fmtlib"
verify_hash "$base/Android.bp" "$LIBBASE_ANDROID_BP_SHA256"
verify_hash "$fmt/Android.bp" "$FMTLIB_ANDROID_BP_SHA256"
verify_hash "$fmt/src/format.cc" "$FMTLIB_FORMAT_SOURCE_SHA256"
source_list="$build/libbase-darwin-sources.txt"
python3 - "$base/Android.bp" > "$source_list" <<'PY'
import re, sys
from pathlib import Path
bp = Path(sys.argv[1]).read_text()
d = bp.index('name: "libbase_defaults"')
s = bp.index('srcs: [', d); e = bp.index('],', s)
sources = re.findall(r'"([^"]+)"', bp[s:e])
d = bp.index('darwin: {', e)
s = bp.index('srcs: [', d); e = bp.index('],', s)
sources += re.findall(r'"([^"]+)"', bp[s:e])
print('\n'.join(sources))
PY
manifest="$build/libbase-darwin-source-manifest.txt"
python3 - "$base" "$source_list" > "$manifest" <<'PY'
import hashlib, sys
from pathlib import Path
root = Path(sys.argv[1])
for relative in Path(sys.argv[2]).read_text().splitlines():
    print(f"{hashlib.sha256((root / relative).read_bytes()).hexdigest()}  {relative}")
PY
source_count="$(wc -l < "$source_list" | tr -d ' ')"
manifest_sha="$(sha256 "$manifest")"
[[ "$source_count" == "$LIBBASE_DARWIN_SOURCE_COUNT" &&
   "$manifest_sha" == "$LIBBASE_DARWIN_SOURCE_MANIFEST_SHA256" ]] || {
  echo "libbase-foundation: Android.bp Darwin source selection changed" >&2
  exit 3
}

cxx="$(command -v clang++)"
ar="$(xcrun --find ar)"
flags=(
  -std=c++23 -arch arm64 -fPIC -Wall -Werror -Wextra
  -Wexit-time-destructors -Wno-vla-cxx-extension
  -I"$base/include" -I"$fmt/include" -I"$aosp/system/logging/liblog/include"
)
object_paths=()
while IFS= read -r relative; do
  object="$objects/libbase_${relative%.cpp}.o"
  echo "libbase-foundation: compile $relative"
  "$cxx" "${flags[@]}" -c "$base/$relative" -o "$object"
  object_paths+=("$object")
done < "$source_list"
fmt_object="$objects/fmt_format.o"
echo "libbase-foundation: compile whole-static fmtlib/src/format.cc"
"$cxx" -std=c++23 -arch arm64 -fPIC -fno-exceptions -UNDEBUG \
  -I"$fmt/include" -c "$fmt/src/format.cc" -o "$fmt_object"
object_paths+=("$fmt_object")

rm -f "$archive"
"$ar" rcs "$archive" "${object_paths[@]}"
members="$($ar -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$members" == 19 ]] || { echo "libbase-foundation: members=$members expected=19" >&2; exit 3; }

combined="$build/libbase-force-loaded.o"
"$cxx" -r -arch arm64 -Wl,-force_load,"$archive" -o "$combined"
nm -gU "$combined" | awk '$2 ~ /^[Tt]$/ {print $3}' | sort -u > "$build/defined-symbols.txt"
nm -u "$combined" | awk '$1 ~ /^_/ {print $1}' | sort -u > "$build/undefined-symbols.txt"
for symbol in _posix_strerror_r; do
  grep -Fx "$symbol" "$build/defined-symbols.txt" >/dev/null || {
    echo "libbase-foundation: required definition missing: $symbol" >&2
    exit 3
  }
done
grep -F '__ZN7android4base23SystemErrorCodeToStringEi' "$build/defined-symbols.txt" >/dev/null || {
  echo "libbase-foundation: SystemErrorCodeToString definition missing" >&2
  exit 3
}

liblog="$project_root/_build/graphics-foundations/liblog-darwin.a"
[[ -f "$liblog" ]] || {
  echo "libbase-foundation: missing module dependency $liblog" >&2
  echo "run tools/build-android16-graphics-foundations.sh" >&2
  exit 2
}
smoke="$build/android16-libbase-smoke"
"$cxx" -std=c++23 -arch arm64 "$project_root/probes/android16_libbase_smoke.cpp" \
  -I"$base/include" -I"$fmt/include" -Wl,-force_load,"$archive" "$liblog" -o "$smoke"
"$smoke" | tee "$build/smoke-output.txt"
echo "libbase-foundation: sources=$source_count whole-static-fmt=1 members=$members unresolved=$(wc -l < "$build/undefined-symbols.txt" | tr -d ' ') arm64=1"
