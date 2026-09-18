#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
for provider in float binary128; do
  module="bionic-$provider-conversion-facade"
  target="$root/_build/bionic-runtime-provider-closure/$provider-target"
  out="$(CARGO_TARGET_DIR="$target" cargo build --quiet --release --lib \
    --message-format=json --manifest-path "$root/tools/$module/Cargo.toml" |
    python3 -c 'import json, sys
module = sys.argv[1]
paths = set()
for line in sys.stdin:
    event = json.loads(line)
    if event.get("reason") == "build-script-executed" and "/" + module + "#" in event.get("package_id", ""):
        paths.add(event["out_dir"])
if len(paths) != 1:
    raise SystemExit("expected one current production build-script output")
print(next(iter(paths)))' "$module")"
  archive="$out/libdarwin_art_bionic_${provider}_conversion.a"
  [[ -f "$archive" && -f "$out/../output" ]] || {
    echo "$module: production output missing" >&2
    exit 1
  }
  # Inspect the selected build-script invocation, not an arbitrary old hash
  # directory. A fixture-free archive alone cannot prove fixture-free inputs.
  if rg -n 'probes/|audit_host_state|allocator_test|rustc-link-arg-bin=' "$out/../output"; then
    echo "$module: default producer still declares an audit dependency" >&2
    exit 1
  fi
  symbols="$(nm -gU "$archive")"
  if rg '_test_(prepare_host_state|host_state_is_preserved)$' <<<"$symbols"; then
    echo "$module: production archive exports fixture helpers" >&2
    exit 1
  fi
  members="$(ar -t "$archive")"
  if rg 'audit_host_state|allocator_test' <<<"$members"; then
    echo "$module: production archive embeds audit objects" >&2
    exit 1
  fi
  echo "$module: production library inputs/archive PASS"
done
