#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ninja="$root/_aosp/external/skia/third_party/ninja/ninja"
graph_dir="$root/_build/native-graph-audit"
graph="$graph_dir/build.ninja"
mkdir -p "$graph_dir"

audit_start=$SECONDS

if [[ ! -x "$ninja" ]]; then
  echo "native-graph: pinned Ninja is missing: $ninja" >&2
  exit 2
fi

(cd "$root" && cargo run -q -p darwin-art-xtask -- native-graph --out "$graph")

cached_cpp="$(grep -Ec '^build [^:]+[.]o: native_cached_cpp(_(legacy|promoted))? ' "$graph" || true)"
cached_archives="$(grep -c ': native_cached_archive ' "$graph" || true)"
icu_cpp="$(grep -Ec '^build .*/_build/icu-foundation/objects/[^:]+[.]o: native_cached_cpp(_(legacy|promoted))? ' "$graph" || true)"
runtime_cpp="$(grep -Ec '^build .*/_build/runtime-(common|bootstrap)/objects/[^:]+[.]o: native_cached_cpp(_(legacy|promoted))? ' "$graph" || true)"
graphics_cpp="$(grep -Ec '^build .*/_build/android-graphics-jni/objects/[^:]+[.]o: native_cached_cpp(_(legacy|promoted))? ' "$graph" || true)"

(( icu_cpp >= 458 )) || {
  echo "native-graph: ICU cache incomplete ($icu_cpp/458 TUs)" >&2
  exit 1
}
(( runtime_cpp >= 128 )) || {
  echo "native-graph: ART runtime cache not promotable ($runtime_cpp TUs)" >&2
  exit 1
}
(( graphics_cpp >= 63 )) || {
  echo "native-graph: GraphicsJNI cache incomplete ($graphics_cpp/63 TUs)" >&2
  exit 1
}
grep -q 'depfile = \$out.d' "$graph"
grep -q 'deps = gcc' "$graph"
# Rust ownership is a real final-link prerequisite. A RuntimeSession/provider
# edit must rebuild only this archive and relink the dylib, never invalidate
# the C++ ART/HWUI translation-unit set.
grep -q '^rule runtime_owner_archive$' "$graph"
grep -q 'target/release/libdarwin_art_runtime.a' "$graph"
grep -q 'runtime_owner_archive' "$graph"
# Once the complete object cache is promotable, neither bootstrap archive may
# fall back to a single edge over the entire repository input closure. Such a
# rule is allowed only during first cache population; keeping it in a warm
# graph would make an unrelated source edit widen the native invalidation set.
if grep -q '^rule graphics_bootstrap$' "$graph"; then
  echo "native-graph: warm graph still exposes broad graphics bootstrap fallback" >&2
  exit 1
fi
if grep -q '^rule runtime_bootstrap$' "$graph"; then
  echo "native-graph: warm graph still exposes broad runtime bootstrap fallback" >&2
  exit 1
fi
runtime_library_target="$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"
runtime_query="$($ninja -f "$graph" -t query "$runtime_library_target" 2>&1)"
grep -q 'target/release/libdarwin_art_runtime.a' <<<"$runtime_query"
headless_library_target="$root/_build/runtime-link-probe/libdarwin_art_runtime.dylib"
headless_query="$($ninja -f "$graph" -t query "$headless_library_target" 2>&1)"
grep -q 'headless_runtime_audit' <<<"$headless_query"
grep -q 'runtime-bootstrap' <<<"$headless_query"
bash "$root/tools/tests/production-runtime-graph-boundary-test.sh"
bash "$root/tools/tests/production-runtime-graph-boundary.sh" \
  "$graph" "$runtime_library_target" "$headless_library_target"

# Phase edges must remain independently addressable. This is the build-time
# contract that keeps a graphics-input/framework edit from recompiling the
# HWUI implementation object or the ART bootstrap archive.
phase_rules=(
  runtime_filesystem_probe
  runtime_network_probe
  runtime_graphics_phase_probe
  runtime_graphics_input_probe
  runtime_graphics_state_probe
  runtime_graphics_session
  runtime_graphics_gpu_probe
  runtime_jni_acceptance_probe
  runtime_hwui_probe
  runtime_app_resources_probe
  runtime_app_activity_probe
  runtime_app_presentation_probe
)
phase_sources=(
  probes/runtime_filesystem_probe.cc
  probes/runtime_network_probe.cc
  probes/runtime_graphics_phase.cc
  probes/runtime_graphics_input.cc
  runtime/embedding/graphics_state.cc
  runtime/embedding/graphics_session.cc
  probes/runtime_graphics_gpu.cc
  probes/runtime_jni_acceptance_probe.cc
  probes/runtime_hwui_probe.cc
  probes/runtime_app_resources.cc
  probes/runtime_app_activity.cc
  probes/runtime_app_presentation.cc
)
for index in "${!phase_rules[@]}"; do
  grep -q "^rule ${phase_rules[$index]}$" "$graph" || {
    echo "native-graph: missing phase rule ${phase_rules[$index]}" >&2
    exit 1
  }
  grep -q "${phase_sources[$index]}" "$graph" || {
    echo "native-graph: missing narrow source edge ${phase_sources[$index]}" >&2
    exit 1
  }
done

phase_objects=(
  "$root/_build/runtime-probes/darwin_art_runtime_filesystem_probe.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_network_probe.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_graphics_phase.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_graphics_input.cc.o"
  "$root/_build/runtime-embedding/darwin_art_graphics_state.cc.o"
  "$root/_build/runtime-embedding/darwin_art_graphics_session.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_graphics_gpu.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_jni_acceptance_probe.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_hwui_probe.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_app_resources.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_app_activity.cc.o"
  "$root/_build/runtime-probes/darwin_art_runtime_app_presentation.cc.o"
)
# A graph generated by an older Ninja invocation may have stale entries in
# `.ninja_deps` even when the compiler fingerprint is current. Let Ninja
# reconcile those entries once; the following dry-run is the actual warm
# no-op assertion.
phase_start=$SECONDS
"$ninja" -f "$graph" "${phase_objects[@]}" >/dev/null
phase_seconds=$((SECONDS - phase_start))
warm_start=$SECONDS
for index in "${!phase_objects[@]}"; do
  object="${phase_objects[$index]}"
  phase_output="$($ninja -f "$graph" -n "$object" 2>&1)"
  grep -q 'no work to do' <<<"$phase_output" || {
    echo "native-graph: warm phase is not a no-op: $object" >&2
    echo "$phase_output" >&2
    exit 1
  }
  # The phase object may feed the final dylib, but it must not inherit a
  # bootstrap archive as a direct input.  Otherwise a framework/probe edit
  # silently widens the invalidation boundary back to the monolithic archive.
  phase_query="$($ninja -f "$graph" -t query "$object" 2>&1)"
  if grep -Eq 'runtime-bootstrap|runtime-graphics-bootstrap|hwui-static-foundation' <<<"$phase_query"; then
    echo "native-graph: phase edge widened by bootstrap input: $object" >&2
    echo "$phase_query" >&2
    exit 1
  fi
  # A phase rule must own exactly its narrow probe source.  The rule-name
  # checks above only prove that an edge exists; this query proves that a
  # later edit to another probe cannot silently widen the phase's direct
  # invalidation set.  Keep this structural check on the generated graph so
  # it does not rely on mtimes or a particular cache state.
  expected_source="${phase_sources[$index]}"
  grep -Fq -- "$expected_source" <<<"$phase_query" || {
    echo "native-graph: phase query lost owner source ${phase_sources[$index]}: $object" >&2
    echo "$phase_query" >&2
    exit 1
  }
  for other_index in "${!phase_sources[@]}"; do
    if [[ "$other_index" == "$index" ]]; then
      continue
    fi
    other_source="${phase_sources[$other_index]}"
    if grep -Fq -- "$other_source" <<<"$phase_query"; then
      echo "native-graph: phase query widened to ${phase_sources[$other_index]}: $object" >&2
      echo "$phase_query" >&2
      exit 1
    fi
  done
done
warm_seconds=$((SECONDS - warm_start))

# Legacy promoted objects may need one dependency-scan edge before the warm
# assertion. Materialize that metadata once, then require the actual second
# query to be a no-op rather than mistaking the bootstrap scan for a rebuild.
"$ninja" -f "$graph" icu-foundation >/dev/null
warm_output="$($ninja -f "$graph" -n icu-foundation 2>&1)"
grep -q 'no work to do' <<<"$warm_output" || {
  echo "native-graph: ICU warm target is not a no-op" >&2
  echo "$warm_output" >&2
  exit 1
}

audit_seconds=$((SECONDS - audit_start))
echo "native-graph: PASS runtime=$runtime_cpp graphics-jni=$graphics_cpp icu=$icu_cpp cached-tu=$cached_cpp archives=$cached_archives phases=${#phase_rules[@]} headless-artifact=graph-owned warm=no-op depfiles=gcc invalidation=direct-source phase_materialize_seconds=$phase_seconds warm_query_seconds=$warm_seconds audit_seconds=$audit_seconds"
