#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
fail() { echo "bionic-dso-lifecycle-facade: $1" >&2; exit 2; }
# shellcheck disable=SC1090
source "$dir/sources.lock"
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash mismatch: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] ||
    fail 'module-local target directory exists'
  git -C "$root" diff --check -- tools/bionic-dso-lifecycle-facade ||
    fail 'tracked whitespace error'
  git -C "$root" diff --cached --check -- tools/bionic-dso-lifecycle-facade ||
    fail 'staged whitespace error'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
           tools/bionic-dso-lifecycle-facade)
}

clean
master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
[[ "$(wc -l < "$dir/manifests/imports.tsv" | tr -d ' ')" == 2 ]] ||
  fail 'provider import manifest count drift'
while IFS=$'\t' read -r symbol kind demand owner; do
  [[ "$kind" == FUNC && "$demand" == D && "$owner" == loader-lifecycle ]] ||
    fail "invalid provider import row: $symbol"
  awk -F '\t' -v wanted="$symbol" \
    '$1==wanted&&$2=="FUNC"&&$3=="D"&&$4=="loader-lifecycle"{found=1}END{exit !found}' \
    "$master" || fail "libc++ import classification drift: $symbol"
done < "$dir/manifests/imports.tsv"

source_root="$root/_aosp/bionic-dso-lifecycle-facade"
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-dso-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/$AOSP_BIONIC_COMMIT/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "AOSP source provenance mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "AOSP sparse source drift: $relative"
done < "$dir/upstream-sources.tsv"
check "$source_root/libc/bionic/atexit.cpp" "$AOSP_ATEXIT_CPP_SHA256"
check "$source_root/libc/bionic/atexit.h" "$AOSP_ATEXIT_H_SHA256"
check "$source_root/libc/arch-common/bionic/crtbegin_so.c" "$AOSP_CRTBEGIN_SO_SHA256"
python3 - "$source_root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
source = (root / 'libc/bionic/atexit.cpp').read_text()
crt = (root / 'libc/arch-common/bionic/crtbegin_so.c').read_text()
assert 'g_array.append_entry({.fn = func, .arg = arg, .dso = dso})' in source
assert 'const AtexitEntry entry = g_array.extract_entry(i);' in source
assert '(dso != nullptr && g_array[i].dso != dso)' in source
assert 'atexit_unlock();\n    entry.fn(entry.arg);\n    atexit_lock();' in source
assert 'if (g_array.total_appends() != total_appends) goto restart;' in source
assert 'if (dso != nullptr)' in source and '__libc_stdio_cleanup();' in source
assert '__unregister_atfork(dso);' in source
assert '__cxa_finalize(&__dso_handle);' in crt
print('bionic-dso-lifecycle-facade: AOSP source semantics PASS')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android35-clang"
readelf="$tc/llvm-readelf"
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-dso-lifecycle.XXXXXX")"
cleanup() {
  [[ "$tmp" == "${TMPDIR:-/tmp}"/bionic-dso-lifecycle.* ]] && find "$tmp" -depth -delete
}
trap cleanup EXIT

"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only \
  "$dir/probes/abi.c"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra \
  -Werror -Wpedantic -I"$dir/include" -c "$dir/src/shims.c" -o "$tmp/shims.o"
nm -u "$tmp/shims.o" | sed 's/^[[:space:]]*//' | sort > "$tmp/shims.actual"
cat > "$tmp/shims.expected" <<'EOF'
___error
_aim_bionic_android_dlopen_ext
_aim_bionic_dlclose
_aim_bionic_dlerror
_aim_bionic_dlopen
_aim_bionic_dlsym
_aim_bionic_dso_cxa_atexit_core
_aim_bionic_dso_cxa_finalize_core
_aim_bionic_dso_cxa_thread_atexit_core
EOF
diff -u "$tmp/shims.expected" "$tmp/shims.actual" || fail 'shim dependency drift'
definitions="$(nm -gU "$tmp/shims.o")"
for symbol in ___cxa_atexit ___cxa_finalize ___cxa_thread_atexit_impl \
              _dladdr _dso_install_dladdr _dso_lifecycle_resolve; do
  grep -F " _aim_bionic$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed provider definition: $symbol"
done
if nm -u "$tmp/shims.o" | grep -E ' (___cxa_atexit|___cxa_finalize|_dlopen|_dlsym|_dyld)' >/dev/null; then
  fail 'host lifecycle or dynamic-loader passthrough escaped provider'
fi
for sanitizer in address undefined; do
  boundary="$tmp/boundary-$sanitizer"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra \
    -Werror -Wpedantic -fsanitize="$sanitizer" -fno-omit-frame-pointer \
    -I"$dir/include" "$dir/src/shims.c" "$dir/probes/boundary_sanitizer.c" \
    -o "$boundary"
  if [[ "$sanitizer" == address ]]; then
    ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 "$boundary"
  else
    UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 "$boundary"
  fi
done

fixture="$tmp/libbionic_dso_lifecycle_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -fvisibility=hidden -mno-outline-atomics -U_FORTIFY_SOURCE \
  -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic -shared -nostdlib \
  -Wl,--build-id=none -Wl,-soname,libbionic_dso_lifecycle_fixture.so \
  -Wl,-z,now -Wl,-z,norelro -Wl,--hash-style=sysv \
  "$dir/probes/fixture.c" -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" > "$tmp/dynsyms"
awk '$7=="UND"&&$8!=""{name=$8;sub(/@.*/,"",name);print name}' "$tmp/dynsyms" |
  sort -u > "$tmp/imports.actual"
cut -f1 "$dir/manifests/imports.tsv" | sort -u > "$tmp/imports.expected"
diff -u "$tmp/imports.expected" "$tmp/imports.actual" ||
  fail 'Android fixture import namespace drift'
if "$readelf" -d "$fixture" | grep -F '(NEEDED)' >/dev/null; then
  fail 'standalone fixture acquired a DT_NEEDED dependency'
fi
for symbol in bionic_dso_fixture_main_handle bionic_dso_fixture_register_triples \
              bionic_dso_fixture_finalize_main bionic_dso_fixture_finalize_global \
              bionic_dso_fixture_register_concurrent bionic_dso_fixture_register_blocking; do
  awk -v wanted="$symbol" '$7!="UND"&&$8==wanted{found=1}END{exit !found}' \
    "$tmp/dynsyms" || fail "fixture export missing: $symbol"
done

CARGO_TARGET_DIR="$tmp/target" cargo run --quiet --manifest-path "$dir/Cargo.toml" \
  --features standalone-test-stubs -- "$fixture"
CARGO_TARGET_DIR="$tmp/target" cargo test --quiet --manifest-path "$dir/Cargo.toml" \
  --features standalone-test-stubs
CARGO_TARGET_DIR="$tmp/target" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" --features standalone-test-stubs -- -D warnings
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean
echo 'bionic-dso-lifecycle-facade: PASS imports=2 AndroidELF LIFO global-interowner-LIFO global-reentrant callbacks=64-exactly-once range-lazy-admit=exactly-once hooks=3-global-cleanups unpublish=busy-drain-success C-boundary=ASan+UBSan target-clean'
