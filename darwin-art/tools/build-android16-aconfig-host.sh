#!/usr/bin/env bash
# Builds AOSP aconfig for the host from the pinned platform/build source.
# prebuilts/build-tools only carries a darwin-x86 binary, which Apple silicon
# cannot execute without Rosetta. The output replaces the surfaceflinger-core
# tool at the same path; its identity is part of that build's generation key.
set -euo pipefail
script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source "$project_root/upstream/android16-surfaceflinger-core.lock"
tool_root="${DARWIN_ART_SURFACEFLINGER_CORE_TOOLS:-$project_root/_downloads/android16-surfaceflinger-core/tools}"
work="$project_root/_build/aconfig-host"
tree="$work/src"
aconfig_dir="$tree/build/make/tools/aconfig"

rm -rf "$tree"
mkdir -p "$aconfig_dir" "$tree/prebuilts/sdk" "$work"
curl -fsSL --retry 3 \
  "https://android.googlesource.com/$ACONFIG_SOURCE_PROJECT/+archive/$ACONFIG_SOURCE_REVISION/tools/aconfig.tar.gz" \
  -o "$work/aconfig.tar.gz"
tar -xzf "$work/aconfig.tar.gz" -C "$aconfig_dir"
manifest="$(cd "$aconfig_dir" && tar -tzf "$work/aconfig.tar.gz" | grep -v '/$' | LC_ALL=C sort |
  while read -r file; do
    if [[ -L "$file" ]]; then
      printf 'link:%s  %s\n' "$(readlink "$file")" "$file"
    else
      printf '%s  %s\n' "$(shasum -a 256 "$file" | awk '{print $1}')" "$file"
    fi
  done |
  shasum -a 256 | awk '{print $1}')"
[[ "$manifest" == "$ACONFIG_SOURCE_MANIFEST_SHA256" ]] || {
  echo "aconfig-host: source manifest mismatch: $manifest" >&2
  exit 3
}
# The upstream workspace also lists device-side crates that need other AOSP
# projects; the host tool needs only these members.
cat > "$aconfig_dir/Cargo.toml" <<'TOML'
[workspace]
members = ["aconfig", "aconfig_protos", "aconfig_storage_file", "convert_finalized_flags"]
resolver = "2"
TOML
# build.rs reads finalized flags from <top>/prebuilts/sdk; C++ codegen does not
# use them, so an empty SDK directory yields the empty finalized-flag record.
(cd "$aconfig_dir/aconfig" && cargo build --release --quiet)
mkdir -p "$tool_root"
install -m 0755 "$aconfig_dir/target/release/aconfig" "$tool_root/aconfig"
echo "aconfig-host: installed $(file -b "$tool_root/aconfig" | cut -d, -f1) revision=$ACONFIG_SOURCE_REVISION"
