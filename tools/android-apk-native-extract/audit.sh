#!/bin/bash
set -euo pipefail
export LC_ALL=C

tool_root="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$tool_root/../.." && pwd)"
fixture_build="$project_root/_build/android-elf-jni-fixture"
loader_root="$project_root/crates/aim-elf-loader"

fail() { echo "apk-native-extract: $*" >&2; exit 3; }
for command in cargo zip xcrun stat; do
  command -v "$command" >/dev/null || fail "missing command: $command"
done

"$project_root/tools/audit-android16-native-library-control-flow.sh" >/dev/null
"$project_root/tools/build-android-elf-jni-fixture.sh" >/dev/null

stage="$(mktemp -d "${TMPDIR:-/tmp}/apk-native-extract.XXXXXX")"
cleanup() {
  chmod -R u+w "$stage" 2>/dev/null || true
  rm -rf "$stage"
}
trap cleanup EXIT

mkdir -p "$stage/apk/lib/arm64-v8a"
for soname in libaim-generic-root.so libaim-generic-child.so \
  libaim-generic-grandchild.so; do
  cp "$fixture_build/$soname" "$stage/apk/lib/arm64-v8a/$soname"
done
printf '\000\377not-text\200\n' > "$stage/apk/arbitrary.bin"

(cd "$stage/apk" && zip -q -9 -X -r "$stage/fixture.apk" .)
cargo test --quiet --manifest-path "$tool_root/Cargo.toml"
extract_output="$(cargo run --quiet --release --manifest-path "$tool_root/Cargo.toml" -- \
  "$stage/fixture.apk" "$stage/extracted" libaim-generic-root.so)"
grep -E '^apk-native-extract: PASS files=3 stored=0 deflated=3 .*crc=verified mode=dir0500\+file0400 publish=atomic' \
  <<< "$extract_output" >/dev/null || fail "unexpected extraction report: $extract_output"
[[ "$(stat -f '%Lp' "$stage/extracted")" == 500 ]] || fail "directory mode is not 0500"
for soname in libaim-generic-root.so libaim-generic-child.so \
  libaim-generic-grandchild.so; do
  [[ "$(stat -f '%Lp' "$stage/extracted/$soname")" == 400 ]] || \
    fail "$soname mode is not 0400"
  cmp "$fixture_build/$soname" "$stage/extracted/$soname" || \
    fail "$soname bytes changed during extraction"
done

cargo build --quiet --release --manifest-path "$loader_root/Cargo.toml" --lib
staticlib="$project_root/target/release/libaim_elf_loader.a"
[[ -f "$staticlib" ]] || fail "existing ELF loader static library is missing"
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -I "$loader_root/include" "$tool_root/discovery_smoke.cc" "$staticlib" \
  -framework Security -framework CoreFoundation -liconv \
  -o "$stage/discovery-smoke"
smoke_output="$("$stage/discovery-smoke" "$stage/extracted" \
  libaim-generic-root.so)"
grep -F 'apk-native-discovery-smoke: PASS' <<< "$smoke_output" >/dev/null || \
  fail "existing loader rejected extracted graph: $smoke_output"

# A bad CRC must fail before publication and leave no staging directory. Use a
# stored copy so the first ELF payload byte has a stable searchable identity.
(cd "$stage/apk" && zip -q -0 -X -r "$stage/bad-crc.apk" .)
elf_offset="$(grep -abo $'\177ELF' "$stage/bad-crc.apk" | head -1 | cut -d: -f1)"
[[ -n "$elf_offset" ]] || fail "could not locate stored ELF payload for CRC test"
printf '\000' | dd of="$stage/bad-crc.apk" bs=1 seek="$elf_offset" conv=notrunc status=none
if cargo run --quiet --release --manifest-path "$tool_root/Cargo.toml" -- \
    "$stage/bad-crc.apk" "$stage/rejected" libaim-generic-root.so \
    >"$stage/rejected.stdout" 2>"$stage/rejected.stderr"; then
  fail "corrupted APK unexpectedly extracted"
fi
[[ ! -e "$stage/rejected" ]] || fail "failed extraction published a directory"
! find "$stage" -maxdepth 1 -name '.aim-apk-extract.*' -print -quit | grep . >/dev/null || \
  fail "failed extraction left a staging directory"

printf '%s\n' "$extract_output"
printf '%s\n' "$smoke_output"
echo "apk-native-extract: PASS source=android-16.0.0_r1 zip=central+local+bounds+crc+stored+deflate policy=arm64-direct-only+no-traversal+no-duplicate+caps+no-symlink ownership=private-read-only cleanup=atomic runtime-files-modified=0"
