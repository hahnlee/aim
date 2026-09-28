#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
lock_file="$project_root/upstream/android16-libbase-foundation.lock"
source "$lock_file"

fetch_file() {
  local project="$1" revision="$2" relative="$3" destination="$4"
  [[ -f "$destination" ]] && return
  mkdir -p "$(dirname "$destination")"
  local temporary
  temporary="$(mktemp "${destination}.tmp.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" \
    | python3 -c 'import base64,sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.buffer.read()))' \
    > "$temporary"
  mv "$temporary" "$destination"
}
fetch_subtree() {
  local project="$1" revision="$2" subtree="$3" destination="$4" marker="$5"
  [[ -f "$destination/$marker" ]] && return
  mkdir -p "$destination"
  curl -fsSL "https://android.googlesource.com/$project/+archive/$revision/$subtree.tar.gz" \
    | tar -xzf - -C "$destination"
}

base="$aosp/system/libbase"
fetch_file "$LIBBASE_PROJECT" "$LIBBASE_REVISION" Android.bp "$base/Android.bp"
fetch_subtree "$LIBBASE_PROJECT" "$LIBBASE_REVISION" include "$base/include" android-base/errors.h
base_sources=(
  abi_compatibility.cpp chrono_utils.cpp cmsg.cpp file.cpp hex.cpp logging.cpp
  mapped_file.cpp parsebool.cpp parsenetaddress.cpp posix_strerror_r.cpp
  process.cpp properties.cpp result.cpp stringprintf.cpp strings.cpp threads.cpp
  test_utils.cpp errors_unix.cpp logging_splitters.h
)
for path in "${base_sources[@]}"; do
  fetch_file "$LIBBASE_PROJECT" "$LIBBASE_REVISION" "$path" "$base/$path"
done

fmt="$aosp/external/fmtlib"
fetch_file "$FMTLIB_PROJECT" "$FMTLIB_REVISION" Android.bp "$fmt/Android.bp"
fetch_file "$FMTLIB_PROJECT" "$FMTLIB_REVISION" src/format.cc "$fmt/src/format.cc"
fetch_subtree "$FMTLIB_PROJECT" "$FMTLIB_REVISION" include "$fmt/include" fmt/format.h

echo "libbase-foundation-sync: Android 16 sparse Gitless sources ready"
