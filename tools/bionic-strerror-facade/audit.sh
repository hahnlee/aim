#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
lock="$dir/sources.lock"
fail() { echo "bionic-strerror-facade: $1" >&2; exit 2; }
[[ -f "$lock" ]] || fail 'missing sources.lock'
# shellcheck disable=SC1090
source "$lock"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() {
  [[ "$(sha256 "$1")" == "$2" ]] || fail "hash mismatch: $1"
}

check_hash "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" \
  "$LIBC_IMPORT_MANIFEST_SHA256"
check_hash "$root/upstream/android16-os-constants-values.tsv" \
  "$OS_CONSTANTS_VALUES_SHA256"
check_hash "$root/tools/bionic-errno-tls/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check_hash "$root/tools/bionic-errno-tls/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check_hash "$root/tools/bionic-errno-tls/generated/darwin_to_android.inc" \
  "$BIONIC_ERRNO_MAPPING_SHA256"
check_hash "$dir/generated/android_errno_messages.inc" "$GENERATED_MESSAGES_SHA256"

awk -F '\t' '$1=="strerror_r" && $2=="FUNC" && $3=="B" &&
  $4=="errno-number-semantics" {found=1} END{exit !found}' \
  "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" ||
  fail 'strerror_r demand/classification drift'
for absent in strerror strsignal; do
  if awk -F '\t' -v symbol="$absent" '$1==symbol {found=1} END{exit !found}' \
    "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"; then
    fail "new coherent-family demand requires separate ownership review: $absent"
  fi
done

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
[[ -x "$android_cc" && -x "$readelf" ]] || fail 'missing pinned NDK toolchain'
check_hash "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check_hash "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/string.h" \
  "$NDK_STRING_HEADER_SHA256"
check_hash "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/errno.h" \
  "$NDK_ERRNO_HEADER_SHA256"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-strerror-facade.XXXXXX")"
cleanup() {
  if [[ -n "$tmp" && "$tmp" == "${TMPDIR:-/tmp}"/bionic-strerror-facade.* ]]; then
    find "$tmp" -depth -delete
  fi
}
trap cleanup EXIT

fetch_bionic() {
  local source_file="$1"
  local expected_hash="$2"
  local output_file="$tmp/$(basename "$source_file")"
  curl -fsSL "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$source_file?format=TEXT" |
    base64 --decode >"$output_file"
  check_hash "$output_file" "$expected_hash"
}
fetch_bionic libc/bionic/strerror.cpp "$BIONIC_STRERROR_CPP_SHA256"
fetch_bionic libc/private/bionic_errdefs.h "$BIONIC_ERRDEFS_SHA256"
fetch_bionic libc/include/string.h "$BIONIC_STRING_HEADER_SHA256"
grep -F 'int strerror_r(int error_number, char* buf, size_t buf_len)' \
  "$tmp/strerror.cpp" >/dev/null || fail 'pinned XSI implementation drift'
grep -F 'return ERANGE;' "$tmp/strerror.cpp" >/dev/null ||
  fail 'pinned positive-ERANGE semantics drift'
grep -F 'extern "C" char* __gnu_strerror_r' "$tmp/strerror.cpp" >/dev/null ||
  fail 'pinned GNU companion ABI drift'
grep -F 'ErrnoRestorer errno_restorer;' "$tmp/strerror.cpp" >/dev/null ||
  fail 'pinned errno preservation drift'

"$android_cc" -dM -E -include errno.h -x c /dev/null >"$tmp/android-macros"
python3 "$dir/tools/generate_messages.py" "$tmp/bionic_errdefs.h" \
  "$tmp/android-macros" >"$tmp/android_errno_messages.inc"
diff -u "$dir/generated/android_errno_messages.inc" \
  "$tmp/android_errno_messages.inc" || fail 'generated Bionic message table drift'
python3 - "$root/upstream/android16-os-constants-values.tsv" \
  "$dir/generated/android_errno_messages.inc" <<'PY'
import re
import sys
from pathlib import Path

values = []
for line in Path(sys.argv[1]).read_text().splitlines():
    fields = line.split("\t")
    if len(fields) == 2 and re.fullmatch(r"E[A-Z0-9]+", fields[0]):
        values.append((fields[0], int(fields[1], 0)))
numbers = {
    int(match.group(1))
    for match in re.finditer(r"^  \{(-?[0-9]+),", Path(sys.argv[2]).read_text(), re.M)
}
assert len(values) == 80, len(values)
missing = [(name, number) for name, number in values if number not in numbers]
assert not missing, missing
assert len(numbers) == 132, len(numbers)
print("bionic-strerror-facade: source-table=132 existing-errno-census=80")
PY

"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -U_GNU_SOURCE \
  -fsyntax-only "$dir/probes/abi.c"
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -c \
  "$dir/probes/gnu_abi.c" -o "$tmp/gnu_abi.o"
"$readelf" --symbols --wide "$tmp/gnu_abi.o" | \
  awk '$7=="UND" && $8=="__gnu_strerror_r" {found=1} END{exit !found}' ||
  fail 'GNU declaration did not rename to __gnu_strerror_r'

host_cc="$(xcrun --find clang)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-arch arm64 -isysroot "$sdk" -std=c17 -O2 -fno-builtin
            -fno-stack-protector -Wall -Wextra
            -Werror -Wpedantic -I"$dir/include" -I"$dir/generated")
"$host_cc" "${host_flags[@]}" -c "$dir/src/strerror.c" \
  -o "$tmp/strerror.o"
nm -u "$tmp/strerror.o" | sed 's/^[[:space:]]*//' | sort >"$tmp/undefined"
printf '%s\n' ___error __tlv_bootstrap >"$tmp/expected-undefined"
diff -u "$tmp/expected-undefined" "$tmp/undefined" ||
  fail 'provider dependency drift or host strerror forwarding'
definitions="$(nm -gU "$tmp/strerror.o")"
for symbol in __gnu_strerror_r strerror_r strerror_r_core strerror_resolve; do
  grep -F " _aim_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing prefixed definition: $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_aim_bionic_' >/dev/null; then
  fail 'unprefixed provider global escaped'
fi
if rg -n 'dlopen|dlsym|dyld|RTLD_|(^|[^[:alnum:]_])strerror(_r)?[[:space:]]*\(' \
  "$dir/src" "$dir/include" >/dev/null; then
  fail 'host strerror/global lookup entered provider'
fi

"$host_cc" "${host_flags[@]}" "$dir/src/strerror.c" \
  "$dir/probes/differential.c" -o "$tmp/differential"
"$tmp/differential"
for sanitizer in address undefined; do
  binary="$tmp/differential-$sanitizer"
  "$host_cc" "${host_flags[@]}" -O1 -g -fsanitize="$sanitizer" \
    -fno-omit-frame-pointer "$dir/src/strerror.c" \
    "$dir/probes/differential.c" -o "$binary"
  if [[ "$sanitizer" == address ]]; then
    ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 "$binary"
  else
    UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 "$binary"
  fi
done

fixture="$tmp/libbionic_strerror_fixture.so"
"$android_cc" -std=c17 -O2 -fPIC -fno-stack-protector -fno-builtin \
  -U_GNU_SOURCE -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror \
  -Wpedantic -shared -nostdlib -Wl,-soname,libbionic_strerror_fixture.so \
  -Wl,-z,now -Wl,-z,norelro -Wl,--hash-style=sysv \
  "$dir/probes/fixture.c" -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" >"$tmp/dynsyms"
awk '$7=="UND" && $8!="" {print $8}' "$tmp/dynsyms" | sort -u \
  >"$tmp/imports.actual"
cat >"$tmp/imports.expected" <<'EOF'
__errno
strerror_r
EOF
diff -u "$tmp/imports.expected" "$tmp/imports.actual" ||
  fail 'Android ELF exact import drift'
grep -E 'GLOBAL DEFAULT +[0-9]+ bionic_strerror_fixture_run$' \
  "$tmp/dynsyms" >/dev/null || fail 'fixture export missing'

CARGO_TARGET_DIR="$tmp/normal" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- \
  "$fixture"
CARGO_TARGET_DIR="$tmp/normal" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" -- -D warnings
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 BIONIC_STRERROR_C_SANITIZER=address \
  CARGO_TARGET_DIR="$tmp/asan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- \
  "$fixture"
UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
  BIONIC_STRERROR_C_SANITIZER=undefined CARGO_TARGET_DIR="$tmp/ubsan" \
  cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check

echo 'bionic-strerror-facade: PASS demand=1 XSI=int GNU=__gnu_strerror_r+caller-buffer messages=132 errno-census=80 unknown+NUL+ERANGE host-errno-preserved AndroidELF imports=2 AOSP-differential ASan+UBSan'
