#!/bin/bash
set -euo pipefail

runtime_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$runtime_root"
# Use a separate test build directory: declaring the guest x18 ABI must not
# replace a shared runtime executable while another application is running.
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/direct-syscall-audit.XXXXXX")"
export CARGO_TARGET_DIR="$probe_dir/target"
cargo test -p aim-elf-loader --no-run --message-format=json \
  > "$probe_dir/artifacts.jsonl"
probe_binary="$(python3 -c '
import json, sys
for line in sys.stdin:
    item = json.loads(line)
    if (item.get("reason") == "compiler-artifact"
            and item.get("target", {}).get("name") == "aim_elf_loader"
            and item.get("profile", {}).get("test") and item.get("executable")):
        print(item["executable"])
' < "$probe_dir/artifacts.jsonl")"
[[ -n "$probe_binary" && -x "$probe_binary" ]] || exit 1
bash tools/declare-darwin-x18-abi.sh "$probe_binary"
codesign --force --sign - "$probe_binary"
"$probe_binary" --include-ignored
printf 'Direct syscall ABI audit artifacts: %s\n' "$probe_dir"
