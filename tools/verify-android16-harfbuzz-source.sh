#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-harfbuzz.lock"

if [[ ! -f "$lock_file" ]]; then
  echo "harfbuzz-source: missing source lock $lock_file" >&2
  exit 2
fi
# shellcheck disable=SC1090
source "$lock_file"

source_root="${HARFBUZZ_SOURCE_ROOT:-$project_root/_aosp/external/harfbuzz_ng}"
android_bp="$source_root/Android.bp"
source_tree="$source_root/$HARFBUZZ_SOURCE_SUBTREE"

print_materialization_blocker() {
  local missing_path="$1"
  echo "harfbuzz-source: missing revision-locked AOSP source: $missing_path" >&2
  echo "required project=$HARFBUZZ_PROJECT" >&2
  echo "required revision=$HARFBUZZ_REVISION" >&2
  echo "required destination=$source_root" >&2
  echo "required files=Android.bp,$HARFBUZZ_SOURCE_SUBTREE/" >&2
  echo "Gitiles metadata: https://android.googlesource.com/$HARFBUZZ_PROJECT/+/$HARFBUZZ_REVISION/Android.bp" >&2
  echo "Gitiles subtree archive: https://android.googlesource.com/$HARFBUZZ_PROJECT/+archive/$HARFBUZZ_REVISION/$HARFBUZZ_SOURCE_SUBTREE.tar.gz" >&2
}

if [[ ! -f "$android_bp" ]]; then
  print_materialization_blocker "$android_bp"
  exit 2
fi
if [[ ! -d "$source_tree" ]]; then
  print_materialization_blocker "$source_tree"
  exit 2
fi

actual_bp_sha="$(shasum -a 256 "$android_bp" | awk '{print $1}')"
if [[ "$actual_bp_sha" != "$HARFBUZZ_ANDROID_BP_SHA256" ]]; then
  echo "harfbuzz-source: Android.bp identity mismatch" >&2
  echo "path=$android_bp" >&2
  echo "expected=$HARFBUZZ_ANDROID_BP_SHA256 actual=$actual_bp_sha" >&2
  exit 3
fi

extract_module_sources() {
  awk '
    /name:[[:space:]]*"libharfbuzz_ng"/ { in_module = 1 }
    in_module && /srcs:[[:space:]]*\[/ { in_sources = 1; next }
    in_sources && /\],[[:space:]]*$/ { exit }
    in_sources {
      line = $0
      if (match(line, /"[^"]+"/)) {
        print substr(line, RSTART + 1, RLENGTH - 2)
      }
    }
  ' "$android_bp"
}

selected_sources="$(extract_module_sources)"
selected_count="$(printf '%s\n' "$selected_sources" | wc -l | tr -d ' ')"
if [[ "$selected_count" != "$HARFBUZZ_HOST_SOURCE_COUNT" ]]; then
  echo "harfbuzz-source: Android.bp libharfbuzz_ng source count changed" >&2
  echo "expected=$HARFBUZZ_HOST_SOURCE_COUNT actual=$selected_count" >&2
  exit 3
fi

selected_manifest="$(while IFS= read -r relative_file; do
  if [[ ! -f "$source_root/$relative_file" ]]; then
    print_materialization_blocker "$source_root/$relative_file"
    exit 2
  fi
  digest="$(shasum -a 256 "$source_root/$relative_file" | awk '{print $1}')"
  printf '%s  %s\n' "$digest" "$relative_file"
done <<< "$selected_sources")"

selected_manifest_sha="$(printf '%s\n' "$selected_manifest" | shasum -a 256 | awk '{print $1}')"
if [[ "$selected_manifest_sha" != "$HARFBUZZ_HOST_SOURCE_MANIFEST_SHA256" ]]; then
  echo "harfbuzz-source: libharfbuzz_ng translation-unit manifest mismatch" >&2
  echo "expected=$HARFBUZZ_HOST_SOURCE_MANIFEST_SHA256 actual=$selected_manifest_sha" >&2
  exit 3
fi

tree_manifest="$(find "$source_tree" -type f ! -name .source-revision -print \
  | sed "s#^$source_root/##" \
  | LC_ALL=C sort \
  | while IFS= read -r relative_file; do
      digest="$(shasum -a 256 "$source_root/$relative_file" | awk '{print $1}')"
      printf '%s  %s\n' "$digest" "$relative_file"
    done)"

tree_count="$(printf '%s\n' "$tree_manifest" | wc -l | tr -d ' ')"
tree_manifest_sha="$(printf '%s\n' "$tree_manifest" | shasum -a 256 | awk '{print $1}')"
if [[ "$tree_count" != "$HARFBUZZ_SOURCE_TREE_FILE_COUNT" ||
      "$tree_manifest_sha" != "$HARFBUZZ_SOURCE_TREE_MANIFEST_SHA256" ]]; then
  echo "harfbuzz-source: complete src/ content manifest mismatch" >&2
  echo "expected_count=$HARFBUZZ_SOURCE_TREE_FILE_COUNT actual_count=$tree_count" >&2
  echo "expected_sha=$HARFBUZZ_SOURCE_TREE_MANIFEST_SHA256 actual_sha=$tree_manifest_sha" >&2
  exit 3
fi

echo "harfbuzz-source: project=$HARFBUZZ_PROJECT revision=$HARFBUZZ_REVISION"
echo "harfbuzz-source: Android.bp=$actual_bp_sha host_sources=$selected_count host_manifest=$selected_manifest_sha"
echo "harfbuzz-source: src_files=$tree_count src_manifest=$tree_manifest_sha"
