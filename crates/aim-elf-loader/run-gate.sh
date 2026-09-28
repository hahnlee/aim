#!/bin/bash
set -euo pipefail

crate_root="$(cd "$(dirname "$0")" && pwd)"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
if [[ -z "$sdk_root" ]]; then
  sdk_root="/Users/${USER:?USER is required}/Library/Android/sdk"
fi

ndk_root="${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-}}"
if [[ -z "$ndk_root" ]]; then
  ndk_root="$(find "$sdk_root/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)"
fi
[[ -d "$ndk_root" ]] || { echo "elf-loader-gate: Android NDK not found" >&2; exit 1; }
[[ "$(basename "$ndk_root")" == 28.2.13676358 ]] || {
  echo "elf-loader-gate: Android NDK revision is not pinned r28c: $ndk_root" >&2
  exit 1
}

clang="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang"
[[ -x "$clang" ]] || { echo "elf-loader-gate: NDK AArch64 clang missing: $clang" >&2; exit 1; }

fixture_dir="$crate_root/target/fixtures"
mkdir -p "$fixture_dir"

common_flags=(
  -shared -fPIC -O2 -nostdlib -fuse-ld=lld
  -Wl,--hash-style=sysv
  -Wl,--build-id=none
  -Wl,-z,max-page-size=16384
  -Wl,-z,common-page-size=16384
)

"$clang" "${common_flags[@]}" -Wl,-soname,libaim_provider.so \
  -Wl,--version-script="$crate_root/fixtures/provider.map" \
  "$crate_root/fixtures/provider.c" -o "$fixture_dir/libaim_provider.so"
"$clang" "${common_flags[@]}" -Wl,-soname,liblifecycle_sink.so \
  "$crate_root/fixtures/lifecycle_sink.c" -o "$fixture_dir/liblifecycle_sink.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,norelro \
  -Wl,-soname,libaim_positive.so \
  "$crate_root/fixtures/positive.c" -o "$fixture_dir/positive.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,now -Wl,-z,norelro \
  -Wl,-soname,libaim_import.so "$crate_root/fixtures/import.c" \
  "$fixture_dir/libaim_provider.so" -o "$fixture_dir/import.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,now -Wl,-z,norelro \
  -Wl,-soname,libaim_weak.so \
  "$crate_root/fixtures/weak.c" -o "$fixture_dir/weak.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,lazy -Wl,-z,norelro \
  -Wl,-soname,libaim_lazy.so "$crate_root/fixtures/import.c" \
  "$fixture_dir/libaim_provider.so" -o "$fixture_dir/lazy.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,now -Wl,-z,relro \
  -Wl,-soname,libaim_relro.so "$crate_root/fixtures/import.c" \
  "$fixture_dir/libaim_provider.so" -o "$fixture_dir/relro.so"
"$clang" "${common_flags[@]}" -Wl,-z,norelro -Wl,-soname,libaim_tls.so \
  "$crate_root/fixtures/tls.c" -o "$fixture_dir/tls.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,now -Wl,-z,norelro -Wl,-Bsymbolic \
  -Wl,-soname,libaim_ifunc.so \
  "$crate_root/fixtures/ifunc.c" -o "$fixture_dir/ifunc.so"
"$clang" "${common_flags[@]}" -O0 -Wl,-z,now -Wl,-z,norelro \
  -Wl,-soname,libaim_finalizer.so -Wl,-fini,finalizer_dt_fini \
  "$crate_root/fixtures/finalizer.c" "$fixture_dir/liblifecycle_sink.so" \
  -o "$fixture_dir/finalizer.so"

graph_flags=("${common_flags[@]}" -O0 -Wl,-z,now -Wl,-z,norelro)
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_dep_a.so \
  -Wl,--version-script="$crate_root/fixtures/graph_dep_a.map" \
  "$crate_root/fixtures/graph_dep_a.c" -o "$fixture_dir/libgraph_dep_a.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_dep_a.so \
  -Wl,--version-script="$crate_root/fixtures/graph_dep_a.map" \
  "$crate_root/fixtures/graph_dep_a_alt.c" -o "$fixture_dir/libgraph_dep_a_alt.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_dep_a.so \
  -Wl,--version-script="$crate_root/fixtures/graph_dep_a_wrong.map" \
  "$crate_root/fixtures/graph_dep_a.c" -o "$fixture_dir/libgraph_dep_a_wrong.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_dep_b.so \
  "$crate_root/fixtures/graph_dep_b.c" "$fixture_dir/libgraph_dep_a.so" \
  -o "$fixture_dir/libgraph_dep_b.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_parent.so \
  "$crate_root/fixtures/graph_parent.c" "$fixture_dir/libgraph_dep_b.so" \
  "$fixture_dir/libgraph_dep_a.so" -o "$fixture_dir/libgraph_parent.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_absolute.so \
  -Wl,--defsym,absolute_graph_value=123 -Wl,--export-dynamic-symbol=absolute_graph_value \
  "$crate_root/fixtures/absolute.c" -o "$fixture_dir/libgraph_absolute.so"

libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
[[ -f "$libcxx" ]] || { echo "elf-loader-gate: pinned libc++_shared.so missing" >&2; exit 1; }
[[ "$(shasum -a 256 "$libcxx" | awk '{print $1}')" == \
   ab4e6c71b96b851de45a8a9bd86369e7dbc2130a44b3b4520564be94847910f2 ]] || {
  echo "elf-loader-gate: pinned r28c libc++_shared.so hash drift" >&2
  exit 1
}
cp "$libcxx" "$fixture_dir/libc++_shared.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,liblibcxx_discovery_root.so \
  "$crate_root/fixtures/libcxx_discovery_root.c" \
  -Wl,--no-as-needed "$fixture_dir/libc++_shared.so" -Wl,--as-needed \
  -o "$fixture_dir/liblibcxx_discovery_root.so"

"$clang" "${graph_flags[@]}" -Wl,-soname,liblifecycle_dep_a.so \
  -Wl,-fini,dep_a_dt_fini "$crate_root/fixtures/lifecycle_dep_a.c" \
  "$fixture_dir/liblifecycle_sink.so" -o "$fixture_dir/liblifecycle_dep_a.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,liblifecycle_dep_b.so \
  -Wl,-fini,dep_b_dt_fini "$crate_root/fixtures/lifecycle_dep_b.c" \
  "$fixture_dir/liblifecycle_dep_a.so" "$fixture_dir/liblifecycle_sink.so" \
  -o "$fixture_dir/liblifecycle_dep_b.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,liblifecycle_parent.so \
  -Wl,-fini,parent_dt_fini "$crate_root/fixtures/lifecycle_parent.c" \
  "$fixture_dir/liblifecycle_dep_b.so" "$fixture_dir/liblifecycle_sink.so" \
  -o "$fixture_dir/liblifecycle_parent.so"

"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_cycle_a.so \
  "$crate_root/fixtures/cycle_a_stub.c" -o "$fixture_dir/libgraph_cycle_a_stub.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_cycle_b.so \
  "$crate_root/fixtures/cycle_b.c" "$fixture_dir/libgraph_cycle_a_stub.so" \
  -o "$fixture_dir/libgraph_cycle_b.so"
"$clang" "${graph_flags[@]}" -Wl,-soname,libgraph_cycle_a.so \
  "$crate_root/fixtures/cycle_a.c" "$fixture_dir/libgraph_cycle_b.so" \
  -o "$fixture_dir/libgraph_cycle_a.so"

for fixture in positive import weak lazy relro tls ifunc finalizer; do
  file "$fixture_dir/$fixture.so" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null
done
for fixture in libgraph_parent libgraph_dep_a libgraph_dep_a_alt \
  libgraph_dep_a_wrong libgraph_dep_b libgraph_absolute libgraph_cycle_a libgraph_cycle_b; do
  file "$fixture_dir/$fixture.so" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null
done
for fixture in liblifecycle_parent liblifecycle_dep_a liblifecycle_dep_b; do
  file "$fixture_dir/$fixture.so" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null
done

readelf="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"
nm="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-nm"
[[ -x "$readelf" && -x "$nm" ]] || { echo "elf-loader-gate: NDK ELF audit tools missing" >&2; exit 1; }

"$readelf" -l -d -r --dyn-syms "$fixture_dir/positive.so" > "$fixture_dir/positive.readelf.txt"
[[ "$(grep -c 'R_AARCH64_RELATIVE' "$fixture_dir/positive.readelf.txt")" == 2 ]]
grep -E 'INIT_ARRAYSZ.*16 \(bytes\)' "$fixture_dir/positive.readelf.txt" >/dev/null
! grep -E '\(NEEDED\)|R_AARCH64_(JUMP_SLOT|GLOB_DAT)| TLS ' "$fixture_dir/positive.readelf.txt" >/dev/null
"$nm" -D --defined-only "$fixture_dir/positive.so" | grep -E ' T fixture_value$' >/dev/null

"$readelf" -l -d -r -V --dyn-syms "$fixture_dir/import.so" > "$fixture_dir/import.readelf.txt"
grep -E '\(NEEDED\).*libaim_provider\.so' "$fixture_dir/import.readelf.txt" >/dev/null
grep -E 'R_AARCH64_ABS64.*provider_value' "$fixture_dir/import.readelf.txt" >/dev/null
grep -E 'R_AARCH64_GLOB_DAT.*provider_data' "$fixture_dir/import.readelf.txt" >/dev/null
grep -E 'R_AARCH64_JUMP_SLOT.*provider_value' "$fixture_dir/import.readelf.txt" >/dev/null
grep -E 'File: libaim_provider\.so' "$fixture_dir/import.readelf.txt" >/dev/null
grep -E 'Name: AIM_1' "$fixture_dir/import.readelf.txt" >/dev/null
grep -E '\(BIND_NOW\)|NOW' "$fixture_dir/import.readelf.txt" >/dev/null
! grep -E 'GNU_RELRO' "$fixture_dir/import.readelf.txt" >/dev/null
"$readelf" -d -r --dyn-syms "$fixture_dir/weak.so" > "$fixture_dir/weak.readelf.txt"
grep -E 'WEAK.*UND.*optional_provider' "$fixture_dir/weak.readelf.txt" >/dev/null
grep -E 'R_AARCH64_(GLOB_DAT|JUMP_SLOT).*optional_provider' "$fixture_dir/weak.readelf.txt" >/dev/null
"$readelf" -d -r "$fixture_dir/lazy.so" > "$fixture_dir/lazy.readelf.txt"
grep -E 'R_AARCH64_JUMP_SLOT.*provider_value' "$fixture_dir/lazy.readelf.txt" >/dev/null
! grep -E '\(BIND_NOW\)|FLAGS.*NOW' "$fixture_dir/lazy.readelf.txt" >/dev/null
"$readelf" -l "$fixture_dir/relro.so" > "$fixture_dir/relro.readelf.txt"
grep -E 'GNU_RELRO' "$fixture_dir/relro.readelf.txt" >/dev/null
"$readelf" -l -r --dyn-syms "$fixture_dir/tls.so" > "$fixture_dir/tls.readelf.txt"
grep -E '^  TLS ' "$fixture_dir/tls.readelf.txt" >/dev/null
[[ "$(grep -c 'R_AARCH64_TLSDESC' "$fixture_dir/tls.readelf.txt")" == 3 ]]
! grep -E 'R_AARCH64_TLS_(DTPMOD|DTPREL|TPREL)' "$fixture_dir/tls.readelf.txt" >/dev/null
grep -E 'TLS +GLOBAL +PROTECTED.*fixture_tls_state$' "$fixture_dir/tls.readelf.txt" >/dev/null
grep -E 'GLOBAL +DEFAULT.*fixture_tls_exchange$' "$fixture_dir/tls.readelf.txt" >/dev/null
"$readelf" -d -r --dyn-syms "$fixture_dir/ifunc.so" > "$fixture_dir/ifunc.readelf.txt"
grep -E 'R_AARCH64_IRELATIVE' "$fixture_dir/ifunc.readelf.txt" >/dev/null
grep -E 'GLOBAL +DEFAULT.*fixture_ifunc_value$' "$fixture_dir/ifunc.readelf.txt" >/dev/null
"$readelf" -d -r "$fixture_dir/finalizer.so" > "$fixture_dir/finalizer.readelf.txt"
grep -E '\(FINI\).*0x[0-9a-f]+' "$fixture_dir/finalizer.readelf.txt" >/dev/null
grep -E 'FINI_ARRAYSZ.*16 \(bytes\)' "$fixture_dir/finalizer.readelf.txt" >/dev/null
grep -E '\(NEEDED\).*liblifecycle_sink\.so' "$fixture_dir/finalizer.readelf.txt" >/dev/null
"$nm" -D --defined-only "$fixture_dir/finalizer.so" | \
  grep -E ' T finalizer_dt_fini$' >/dev/null
for fixture in liblifecycle_parent liblifecycle_dep_a liblifecycle_dep_b; do
  "$readelf" -d -r "$fixture_dir/$fixture.so" > "$fixture_dir/$fixture.readelf.txt"
  grep -E '\(FINI\).*0x[0-9a-f]+' "$fixture_dir/$fixture.readelf.txt" >/dev/null
  grep -E 'FINI_ARRAYSZ.*16 \(bytes\)' "$fixture_dir/$fixture.readelf.txt" >/dev/null
  grep -E '\(NEEDED\).*liblifecycle_sink\.so' "$fixture_dir/$fixture.readelf.txt" >/dev/null
done

"$readelf" -d -r -V --dyn-syms "$fixture_dir/libgraph_parent.so" \
  > "$fixture_dir/graph-parent.readelf.txt"
[[ "$(grep -c '(NEEDED)' "$fixture_dir/graph-parent.readelf.txt")" == 2 ]]
grep -E '\(NEEDED\).*libgraph_dep_b\.so' "$fixture_dir/graph-parent.readelf.txt" >/dev/null
grep -E '\(NEEDED\).*libgraph_dep_a\.so' "$fixture_dir/graph-parent.readelf.txt" >/dev/null
grep -E 'WEAK.*UND.*optional_graph_value' "$fixture_dir/graph-parent.readelf.txt" >/dev/null
grep -E 'File: libgraph_dep_a\.so' "$fixture_dir/graph-parent.readelf.txt" >/dev/null
grep -E 'Name: GRAPH_1' "$fixture_dir/graph-parent.readelf.txt" >/dev/null
"$readelf" --dyn-syms "$fixture_dir/libgraph_dep_b.so" > "$fixture_dir/graph-dep-b.readelf.txt"
grep -E 'GLOBAL +PROTECTED.*dep_b_value' "$fixture_dir/graph-dep-b.readelf.txt" >/dev/null
"$readelf" --dyn-syms "$fixture_dir/libgraph_absolute.so" > "$fixture_dir/graph-absolute.readelf.txt"
grep -E 'GLOBAL +DEFAULT +ABS absolute_graph_value' "$fixture_dir/graph-absolute.readelf.txt" >/dev/null
"$readelf" -d "$fixture_dir/libgraph_cycle_a.so" > "$fixture_dir/cycle-a.readelf.txt"
"$readelf" -d "$fixture_dir/libgraph_cycle_b.so" > "$fixture_dir/cycle-b.readelf.txt"
grep -E '\(NEEDED\).*libgraph_cycle_b\.so' "$fixture_dir/cycle-a.readelf.txt" >/dev/null
grep -E '\(NEEDED\).*libgraph_cycle_a\.so' "$fixture_dir/cycle-b.readelf.txt" >/dev/null

cargo test --manifest-path "$crate_root/Cargo.toml"
cargo run --quiet --release --manifest-path "$crate_root/Cargo.toml" --bin elf-loader-gate -- \
  "$fixture_dir/positive.so" "$fixture_dir/import.so" "$fixture_dir/weak.so" \
  "$fixture_dir/lazy.so" "$fixture_dir/relro.so" "$fixture_dir/tls.so" \
  "$fixture_dir/ifunc.so" "$fixture_dir/finalizer.so" "$fixture_dir/libc++_shared.so"
cargo run --quiet --release --manifest-path "$crate_root/Cargo.toml" --bin elf-namespace-gate -- \
  "$fixture_dir/libgraph_parent.so" "$fixture_dir/libgraph_dep_a.so" \
  "$fixture_dir/libgraph_dep_a_alt.so" "$fixture_dir/libgraph_dep_a_wrong.so" \
  "$fixture_dir/libgraph_dep_b.so" "$fixture_dir/libgraph_cycle_a.so" \
  "$fixture_dir/libgraph_cycle_b.so" "$fixture_dir/libgraph_parent.so" \
  "$fixture_dir/libgraph_absolute.so" "$fixture_dir/liblifecycle_parent.so" \
  "$fixture_dir/liblifecycle_dep_a.so" "$fixture_dir/liblifecycle_dep_b.so"

cargo build --quiet --release --manifest-path "$crate_root/Cargo.toml" --lib
staticlib="$crate_root/target/release/libaim_elf_loader.a"
[[ -f "$staticlib" ]] || { echo "elf-loader-gate: staticlib missing" >&2; exit 1; }
xcrun clang -std=c11 -arch arm64 -Wall -Wextra -Werror -x c -fsyntax-only \
  -include "$crate_root/include/aim_elf_loader.h" /dev/null
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -I "$crate_root/include" "$crate_root/ffi-smoke.mm" "$staticlib" \
  -framework Security -framework CoreFoundation -liconv \
  -o "$fixture_dir/ffi-smoke"
file "$fixture_dir/ffi-smoke" | grep -F 'Mach-O 64-bit executable arm64' >/dev/null
for symbol in aim_elf_load_bytes aim_elf_load_path \
  aim_elf_inspect_bytes aim_elf_inspection_soname \
  aim_elf_inspection_needed_count aim_elf_inspection_needed_at \
  aim_elf_inspection_destroy aim_elf_discover_sibling_graph \
  aim_elf_discovered_graph_root_soname \
  aim_elf_discovered_graph_sources aim_elf_discovered_graph_destroy \
  aim_elf_run_initializers aim_elf_lookup aim_elf_unload \
  aim_elf_graph_load aim_elf_graph_load_with_lifecycle \
  aim_elf_graph_lookup_root \
  aim_elf_graph_unload; do
  nm -gU "$fixture_dir/ffi-smoke" | grep -E " _${symbol}$" >/dev/null
done
"$fixture_dir/ffi-smoke" "$fixture_dir/positive.so" "$fixture_dir/import.so" \
  "$fixture_dir/libgraph_parent.so" "$fixture_dir/libgraph_dep_a.so" \
  "$fixture_dir/libgraph_dep_b.so" "$fixture_dir/liblibcxx_discovery_root.so" \
  "$fixture_dir/libc++_shared.so"
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I "$crate_root/include" "$crate_root/ffi-smoke.mm" "$staticlib" \
  -framework Security -framework CoreFoundation -liconv \
  -o "$fixture_dir/ffi-smoke-sanitized"
ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$fixture_dir/ffi-smoke-sanitized" \
  "$fixture_dir/positive.so" "$fixture_dir/import.so" \
  "$fixture_dir/libgraph_parent.so" "$fixture_dir/libgraph_dep_a.so" \
  "$fixture_dir/libgraph_dep_b.so" "$fixture_dir/liblibcxx_discovery_root.so" \
  "$fixture_dir/libc++_shared.so" >/dev/null
