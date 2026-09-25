#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android35-bionic-pthread-provider.lock"
module_root="$project_root/tools/android-bionic-pthread-provider"
runner_root="$project_root/tools/android-bionic-pthread-provider-runner"
source_root="$project_root/_aosp/bionic-pthread-provider"
build_dir="$project_root/_build/bionic-pthread-provider"
ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64"
libcxx="$toolchain/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
pthread_types_h="$toolchain/sysroot/usr/include/bits/pthread_types.h"
pthread_h="$toolchain/sysroot/usr/include/pthread.h"
readelf="$toolchain/bin/llvm-readelf"
elf_nm="$toolchain/bin/llvm-nm"
android_clang="$toolchain/bin/aarch64-linux-android${ANDROID_API}-clang"

fail() { echo "bionic-pthread-provider: $*" >&2; exit 3; }
missing() { echo "bionic-pthread-provider: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

for input in "$libcxx" "$pthread_types_h" "$pthread_h"; do
  [[ -f "$input" ]] || missing "$input"
done
for tool in "$readelf" "$elf_nm" "$android_clang"; do
  [[ -x "$tool" ]] || missing "$tool"
done
[[ "$(stat -f '%z' "$libcxx")" == "$NDK_LIBCXX_SHARED_SIZE" ]] ||
  fail "libc++_shared size mismatch"
[[ "$(sha "$libcxx")" == "$NDK_LIBCXX_SHARED_SHA256" ]] ||
  fail "libc++_shared SHA mismatch"
[[ "$(sha "$pthread_types_h")" == "$NDK_PTHREAD_TYPES_H_SHA256" ]] ||
  fail "NDK pthread_types.h SHA mismatch"
[[ "$(sha "$pthread_h")" == "$NDK_PTHREAD_H_SHA256" ]] ||
  fail "NDK pthread.h SHA mismatch"

stage="$(mktemp -d "${TMPDIR:-/tmp}/bionic-pthread-provider.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$7=="UND" && $8 ~ /^pthread_.*@LIBC/ {
         name=$8; sub(/@.*/, "", name);
         version=$8; sub(/^[^@]*@/, "", version);
         print name "\t" $4 "\t" version
       }' | sort -u > "$stage/libcxx-pthread-imports.tsv"
tail -n +2 "$module_root/libcxx-pthread-imports.tsv" | cut -f1-3 > "$stage/locked-imports.tsv"
diff -u "$stage/locked-imports.tsv" "$stage/libcxx-pthread-imports.tsv" ||
  fail "libc++ pthread import manifest drift"
[[ "$(wc -l < "$stage/libcxx-pthread-imports.tsv" | tr -d ' ')" == "$LIBCXX_PTHREAD_IMPORT_COUNT" ]] ||
  fail "pthread import count mismatch"
[[ "$(awk -F '\t' '$4=="supported"{n++}END{print n+0}' "$module_root/libcxx-pthread-imports.tsv")" == "$SUPPORTED_PTHREAD_IMPORT_COUNT" ]] ||
  fail "supported import count mismatch"
[[ "$(awk -F '\t' '$4=="unsupported"{n++}END{print n+0}' "$module_root/libcxx-pthread-imports.tsv")" == "$UNSUPPORTED_PTHREAD_IMPORT_COUNT" ]] ||
  fail "unsupported import count mismatch"

files=(
  libc/bionic/pthread_once.cpp
  libc/bionic/pthread_mutex.cpp
  libc/bionic/pthread_key.cpp
  libc/bionic/pthread_self.cpp
  libc/bionic/pthread_cond.cpp
  libc/bionic/pthread_rwlock.cpp
  libc/bionic/pthread_create.cpp
  libc/bionic/pthread_join.cpp
  libc/bionic/pthread_detach.cpp
  libc/bionic/pthread_internal.h
)
hashes=(
  "$BIONIC_PTHREAD_ONCE_CPP_SHA256"
  "$BIONIC_PTHREAD_MUTEX_CPP_SHA256"
  "$BIONIC_PTHREAD_KEY_CPP_SHA256"
  "$BIONIC_PTHREAD_SELF_CPP_SHA256"
  "$BIONIC_PTHREAD_COND_CPP_SHA256"
  "$BIONIC_PTHREAD_RWLOCK_CPP_SHA256"
  "$BIONIC_PTHREAD_CREATE_CPP_SHA256"
  "$BIONIC_PTHREAD_JOIN_CPP_SHA256"
  "$BIONIC_PTHREAD_DETACH_CPP_SHA256"
  "$BIONIC_PTHREAD_INTERNAL_H_SHA256"
)
[[ "${#files[@]}" == "$LOCKED_BIONIC_SOURCE_COUNT" ]] ||
  fail "locked Bionic source count mismatch"
for index in "${!files[@]}"; do
  relative="${files[$index]}"
  destination="$source_root/$relative"
  expected="${hashes[$index]}"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-pthread-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(sha "$staged")" == "$expected" ]] ||
      fail "download SHA mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha "$destination")" == "$expected" ]] ||
    fail "Bionic source SHA mismatch: $relative"
done
printf '%s\n' "$BIONIC_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail "Git metadata is forbidden in sparse source"

python3 - "$source_root" "$pthread_types_h" "$pthread_h" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
types = Path(sys.argv[2]).read_text()
header = Path(sys.argv[3]).read_text()
once = (root / "libc/bionic/pthread_once.cpp").read_text()
mutex = (root / "libc/bionic/pthread_mutex.cpp").read_text()
key = (root / "libc/bionic/pthread_key.cpp").read_text()
self_source = (root / "libc/bionic/pthread_self.cpp").read_text()
cond = (root / "libc/bionic/pthread_cond.cpp").read_text()
rwlock = (root / "libc/bionic/pthread_rwlock.cpp").read_text()
create = (root / "libc/bionic/pthread_create.cpp").read_text()
join = (root / "libc/bionic/pthread_join.cpp").read_text()
detach = (root / "libc/bionic/pthread_detach.cpp").read_text()
internal = (root / "libc/bionic/pthread_internal.h").read_text()

assert "typedef long pthread_t;" in types
assert "typedef int pthread_key_t;" in types
assert "typedef int pthread_once_t;" in types
assert "typedef long pthread_mutexattr_t;" in types
assert "int32_t __private[10]" in types
assert "int32_t __private[12]" in types
assert "int32_t __private[14]" in types
assert "#define PTHREAD_MUTEX_INITIALIZER { { ((PTHREAD_MUTEX_NORMAL & 3) << 14) } }" in header
assert "#define PTHREAD_COND_INITIALIZER  { { 0 } }" in header
assert "#define PTHREAD_COND_INITIALIZER_MONOTONIC_NP  { { 1 << 1 } }" in header
assert "#define PTHREAD_RWLOCK_INITIALIZER  { { 0 } }" in header
assert "#define PTHREAD_ONCE_INIT 0" in header
assert "ONCE_INITIALIZATION_NOT_YET_STARTED   0" in once
assert "ONCE_INITIALIZATION_UNDERWAY          1" in once
assert "ONCE_INITIALIZATION_COMPLETE          2" in once
assert "KEY_VALID_FLAG (1 << 31)" in key
assert "*key = i | KEY_VALID_FLAG;" in key
assert "pthread_key_clean_all" in key and "PTHREAD_DESTRUCTOR_ITERATIONS" in key
assert "key_data[i].data = nullptr" in key
assert "static inline __always_inline bool IsMutexDestroyed" in mutex
assert "mutex_state == 0xffff" in mutex
assert "memset(mutex, 0, sizeof(pthread_mutex_internal_t))" in mutex
assert "return reinterpret_cast<pthread_t>(__get_thread())" in self_source
assert "COND_SHARED_MASK 0x0001" in cond
assert "COND_CLOCK_MASK 0x0002" in cond
assert "COND_COUNTER_STEP 0x0004" in cond
assert "char __reserved[40]" in cond
assert "atomic_store_explicit(&cond->state, 0xdeadc04d" in cond
assert "pthread_mutex_unlock(mutex)" in cond and "pthread_mutex_lock(mutex)" in cond
assert "cond->use_realtime_clock()" in cond
assert "status == -ETIMEDOUT" in cond
assert "STATE_HAVE_PENDING_READERS_FLAG" in rwlock
assert "STATE_HAVE_PENDING_WRITERS_FLAG" in rwlock
assert "STATE_READER_COUNT_CHANGE_STEP  (1 << STATE_READER_COUNT_SHIFT)" in rwlock
assert "STATE_OWNED_BY_WRITER_FLAG      (1 << STATE_OWNED_BY_WRITER_SHIFT)" in rwlock
assert "char __reserved[20]" in rwlock
assert "rwlock->writer_tid" in rwlock and "return EPERM;" in rwlock
assert "pthread_rwlock_destroy" in rwlock and "return EBUSY;" in rwlock
assert "PTHREAD_RWLOCK_PREFER_READER_NP" in rwlock
assert "MUTEXATTR_TYPE_MASK   0x000f" in mutex
assert "MUTEXATTR_SHARED_MASK 0x0010" in mutex
assert "MUTEXATTR_PROTOCOL_MASK 0x0020" in mutex
assert "*attr = PTHREAD_MUTEX_DEFAULT" in mutex
assert "*attr = -1" in mutex
assert "type < PTHREAD_MUTEX_NORMAL || type > PTHREAD_MUTEX_ERRORCHECK" in mutex
assert "case PTHREAD_MUTEX_RECURSIVE" in mutex
assert "case PTHREAD_MUTEX_ERRORCHECK" in mutex
assert "*thread_out = __pthread_internal_add(thread)" in create
assert "thread->startup_handshake_lock.unlock()" in create
assert "t == pthread_self()" in join and "return EDEADLK;" in join
assert "return ESRCH;" in join and "return ESRCH;" in detach
assert "THREAD_EXITED_NOT_JOINED" in join and "THREAD_DETACHED" in detach
assert "return pthread_join(t, nullptr)" in detach
for state in ("THREAD_NOT_JOINED", "THREAD_EXITED_NOT_JOINED", "THREAD_JOINED", "THREAD_DETACHED"):
    assert state in internal
print("bionic-pthread-provider: upstream-layout=PASS sources=10 lifecycle=create+join+detach")
PY

[[ "$(sha "$module_root/fixture/pthread_fixture.c")" == "$FIXTURE_C_SHA256" ]] ||
  fail "fixture source SHA mismatch"
[[ "$(sha "$module_root/fixture/exports.map")" == "$FIXTURE_EXPORTS_SHA256" ]] ||
  fail "fixture exports SHA mismatch"

python3 - "$module_root/src/provider.cc" <<'PY'
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_text()
start = source.index('extern "C" int darwin_art_bionic_pthread_create(')
end = source.index('extern "C" int darwin_art_bionic_pthread_join(', start)
create = source[start:end]
host_create = create.index("pthread_create(&entry->host")
assert create.index("std::make_shared<ThreadEntry>()") < host_create
assert create.index("state.threads.emplace") < host_create
assert create.index("new (std::nothrow)") < host_create
assert "catch (const std::bad_alloc&)" in create
assert "RemoveThreadEntry(entry);" in create
find_start = source.index("std::shared_ptr<ThreadEntry> FindThreadEntry(")
find_end = source.index("std::atomic<uint64_t>& NextThreadToken()", find_start)
assert "entry->published ? entry : nullptr" in source[find_start:find_end]
print("bionic-pthread-provider: create-failure-atomicity=PASS allocation-before-host publish-gated rollback=owned")
PY

cxx="$(xcrun --find clang++)"
cc="$(xcrun --find clang)"
ar="$(xcrun --find ar)"
host_nm="$(xcrun --find nm)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-std=c++20 -arch arm64 -isysroot "$macos_sdk" -O2 -Wall -Wextra -Werror
            -fvisibility=hidden -fvisibility-inlines-hidden -I"$module_root/include")
"$cxx" "${host_flags[@]}" -c "$module_root/src/provider.cc" -o "$stage/provider.o"
"$cc" -std=c17 -arch arm64 -isysroot "$macos_sdk" -O2 -Wall -Wextra -Werror \
  -I"$project_root/tools/bionic-errno-tls/include" \
  -I"$project_root/tools/bionic-errno-tls/generated" \
  -c "$project_root/tools/bionic-errno-tls/src/errno_tls.c" \
  -o "$stage/errno_tls.o"
if grep -E 'reinterpret_cast<[^>]*pthread_(t|key_t|once_t|mutex_t|mutexattr_t|cond_t|rwlock_t)' \
    "$module_root/src/provider.cc" >/dev/null; then
  fail "Android opaque object is reinterpreted as a Darwin pthread type"
fi
"$ar" rcs "$stage/libdarwin-art-bionic-pthread.a" \
  "$stage/provider.o" "$stage/errno_tls.o"
file "$stage/provider.o" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
  fail "provider object is not Darwin arm64"
if "$host_nm" -gU "$stage/provider.o" | awk '{print $3}' |
    grep -E '^_pthread_' >/dev/null; then
  fail "unprefixed pthread definition escaped provider"
fi
for symbol in self key_create key_delete getspecific setspecific once \
              create join detach \
              mutexattr_init mutexattr_destroy mutexattr_settype \
              mutex_init mutex_lock mutex_trylock mutex_unlock mutex_destroy \
              cond_wait cond_timedwait cond_signal cond_broadcast cond_destroy \
              rwlock_rdlock rwlock_wrlock rwlock_unlock; do
  "$host_nm" -gU "$stage/provider.o" | awk '{print $3}' |
    grep -Fx "_darwin_art_bionic_pthread_$symbol" >/dev/null ||
    fail "missing provider definition: pthread_$symbol"
done
"$host_nm" -gU "$stage/provider.o" | awk '{print $3}' |
  grep -Fx '_darwin_art_bionic_pthread_rwlock_destroy' >/dev/null ||
  fail "missing prefixed rwlock lifecycle destroy API"

fixture="$stage/libdarwin-art-pthread-fixture.so"
"$android_clang" -std=c17 -O2 -mno-outline-atomics -fPIC \
  -fvisibility=hidden -Wall -Wextra -Werror -shared -nostdlib -fuse-ld=lld \
  -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now -Wl,-z,norelro \
  -Wl,-z,max-page-size=16384 -Wl,-soname,libdarwin-art-pthread-fixture.so \
  -Wl,--version-script,"$module_root/fixture/exports.map" \
  "$module_root/fixture/pthread_fixture.c" -lc -o "$fixture"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail "fixture is not Android arm64 ELF"
[[ "$(sha "$fixture")" == "$FIXTURE_ELF_SHA256" ]] || fail "fixture ELF SHA mismatch"
[[ "$($readelf -d "$fixture" | awk '/\(NEEDED\)/{gsub(/[][]/,"",$NF); print $NF}')" == "libc.so" ]] ||
  fail "fixture namespace is not exactly libc.so"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND" && $8 ~ /^pthread_.*@LIBC/ {name=$8; sub(/@.*/,"",name); print name}' |
  sort -u > "$stage/fixture-imports.txt"
awk -F '\t' '$4=="supported"{print $1}' "$module_root/libcxx-pthread-imports.tsv" |
  sort -u > "$stage/supported-imports.txt"
cp "$stage/supported-imports.txt" "$stage/fixture-expected-imports.txt"
printf '%s\n' pthread_create >> "$stage/fixture-expected-imports.txt"
sort -u "$stage/fixture-expected-imports.txt" -o "$stage/fixture-expected-imports.txt"
diff -u "$stage/fixture-expected-imports.txt" "$stage/fixture-imports.txt" ||
  fail "fixture does not execute exact supported import set"
[[ "$($elf_nm -D --defined-only "$fixture" | awk '$2=="T"{n++}END{print n+0}')" == 31 ]] ||
  fail "fixture export count mismatch"

export DARWIN_ART_PTHREAD_PROVIDER_LIBDIR="$stage"
export CARGO_TARGET_DIR="$stage/cargo-target"
cargo build --quiet --manifest-path "$runner_root/Cargo.toml" --features provider-link
runner="$CARGO_TARGET_DIR/debug/android-bionic-pthread-provider-runner"
output="$("$runner" "$fixture")"
grep -F 'ELF=executed resolver=libc.so@LIBC imports=24/24 extra-owner=pthread_create' <<< "$output" >/dev/null ||
  fail "closed resolver execution failed"
grep -F 'tls-destructor=2-pass once=1 mutex=8x2000-contention' <<< "$output" >/dev/null ||
  fail "thread/TLS/once/mutex E2E failed"
grep -F 'destroy-lookup=race-safe' <<< "$output" >/dev/null ||
  fail "destroy/lookup lifecycle synchronization failed"
grep -F 'mutex-attr=normal+recursive+errorcheck recursive-depth=2 errorcheck-self=EDEADLK wrong-unlock=EPERM destroyed-attr=EINVAL pshared+PI=ENOTSUP' <<< "$output" >/dev/null ||
  fail "mutex attribute Android ELF E2E failed"
grep -F 'cond=4-waiters signal=1 broadcast=3 predicate-loop=held monotonic-timeout=110 destroy-wait=EBUSY pshared=ENOTSUP' <<< "$output" >/dev/null ||
  fail "condition variable Android ELF E2E failed"
grep -F 'rwlock=4-concurrent-readers writer=blocked-then-progress wrong-unlock=EPERM recursive-read=2 lazy-zero-init reset=idle' <<< "$output" >/dev/null ||
  fail "rwlock Android ELF E2E failed"
grep -F 'thread-lifecycle=create+join+detach token=provider-owned result=roundtrip foreign=ESRCH self-join=EDEADLK race=one-winner target-clean=reset' <<< "$output" >/dev/null ||
  fail "thread lifecycle Android ELF E2E failed"
grep -F 'no-Darwin-reinterpret stale-key=reuse-alias(Bionic-undefined) unsupported=fork+robust+pshared+PI+thread-attrs' <<< "$output" >/dev/null ||
  fail "capability matrix failed"

"$cxx" "${host_flags[@]}" -fsanitize=address,undefined \
  "$module_root/src/provider.cc" "$stage/errno_tls.o" \
  "$module_root/tls_delete_stress.cc" \
  -o "$stage/tls-delete-stress"
tls_stress_output="$("$stage/tls-delete-stress")"
grep -F 'delete-vs-get+set ASan=clean' <<< "$tls_stress_output" >/dev/null ||
  fail "TLS delete/get/set sanitizer stress failed"
grep -F 'repeated-delete=10000 peak-cells=1 reset-cells=0' <<< "$tls_stress_output" >/dev/null ||
  fail "TLS repeated-delete bounded-state/reset gate failed"

[[ "$(sha "$module_root/cond_stress.cc")" == "$COND_STRESS_SHA256" ]] ||
  fail "condition stress source SHA mismatch"
"$cxx" "${host_flags[@]}" -fsanitize=address,undefined \
  "$module_root/src/provider.cc" "$stage/errno_tls.o" \
  "$module_root/cond_stress.cc" \
  -o "$stage/cond-stress"
cond_stress_output="$("$stage/cond-stress")"
grep -F 'rounds=100 waiters=8 destroy-wait=EBUSY ASan=clean monotonic-timeout=110 monotonic-attr-timeout=110 relock=owned' <<< "$cond_stress_output" >/dev/null ||
  fail "condition sanitizer stress failed"

[[ "$(sha "$module_root/rwlock_stress.cc")" == "$RWLOCK_STRESS_SHA256" ]] ||
  fail "rwlock stress source SHA mismatch"
"$cxx" "${host_flags[@]}" -fsanitize=address,undefined \
  "$module_root/src/provider.cc" "$stage/errno_tls.o" \
  "$module_root/rwlock_stress.cc" \
  -o "$stage/rwlock-stress"
rwlock_stress_output="$("$stage/rwlock-stress")"
grep -F 'rounds=20 readers=4 concurrent>=2 writer=10000-progress wrong-unlock=EPERM destroy-held=EBUSY double-destroy=Bionic-0 reinit-live=Bionic-0 lazy-reset=clean ASan=clean' <<< "$rwlock_stress_output" >/dev/null ||
  fail "rwlock sanitizer stress failed"

[[ "$(sha "$module_root/mutex_attr_stress.cc")" == "$MUTEX_ATTR_STRESS_SHA256" ]] ||
  fail "mutex attribute stress source SHA mismatch"
"$cxx" "${host_flags[@]}" -fsanitize=address,undefined \
  "$module_root/src/provider.cc" "$stage/errno_tls.o" \
  "$module_root/mutex_attr_stress.cc" \
  -o "$stage/mutex-attr-stress"
mutex_attr_stress_output="$("$stage/mutex-attr-stress")"
grep -F 'rounds=100 normal+recursive+errorcheck recursive-depth=2 self=EDEADLK wrong-owner=EPERM held-destroy=EBUSY address-reuse=fresh-generation reinit-live=Bionic-0 destroyed-attr=EINVAL pshared+PI=ENOTSUP ASan=clean' <<< "$mutex_attr_stress_output" >/dev/null ||
  fail "mutex attribute sanitizer stress failed"

[[ "$(sha "$module_root/thread_lifecycle_stress.cc")" == "$THREAD_LIFECYCLE_STRESS_SHA256" ]] ||
  fail "thread lifecycle stress source SHA mismatch"
"$cxx" "${host_flags[@]}" -fsanitize=address,undefined \
  "$module_root/src/provider.cc" "$stage/errno_tls.o" \
  "$module_root/thread_lifecycle_stress.cc" \
  -o "$stage/thread-lifecycle-stress"
thread_lifecycle_stress_output="$("$stage/thread-lifecycle-stress")"
grep -F 'rounds=100 create+join+detach result=roundtrip self=EDEADLK foreign=ESRCH join-vs-detach=one-winner detached-clean=quiescent target-clean=reset ASan=clean' <<< "$thread_lifecycle_stress_output" >/dev/null ||
  fail "thread lifecycle sanitizer stress failed"

mkdir -p "$build_dir"
cp "$stage/libdarwin-art-bionic-pthread.a" "$build_dir/"
cp "$fixture" "$build_dir/"
cp "$runner" "$build_dir/"
printf '%s\n' "$output"
printf '%s\n' "$tls_stress_output"
printf '%s\n' "$cond_stress_output"
printf '%s\n' "$rwlock_stress_output"
printf '%s\n' "$mutex_attr_stress_output"
printf '%s\n' "$thread_lifecycle_stress_output"
echo "bionic-pthread-provider: PASS imports=24/24 extra-owner=create ELF=Android-arm64 host=Darwin-arm64 runtime-files-modified=0"
