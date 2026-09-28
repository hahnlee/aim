#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
download_repo="$project_root/_downloads/android16-dexter/dexter"
source_root="$project_root/_aosp/tools/dexter/slicer"

# shellcheck disable=SC1091
source "$project_root/upstream/android16-dexter-slicer.lock"

if [[ ! -d "$download_repo/.git" ]]; then
  mkdir -p "$download_repo"
  git -C "$download_repo" init -q
  git -C "$download_repo" remote add origin \
    "https://android.googlesource.com/$DEXTER_PROJECT"
fi
if ! git -C "$download_repo" cat-file -e \
    "$DEXTER_REVISION^{commit}" 2>/dev/null; then
  git -C "$download_repo" fetch -q --depth=1 origin "$DEXTER_REVISION"
fi

actual_tree="$(git -C "$download_repo" rev-parse "$DEXTER_REVISION:slicer")"
if [[ "$actual_tree" != "$DEXTER_SLICER_TREE" ]]; then
  echo "dexter-slicer-sync: tree mismatch expected=$DEXTER_SLICER_TREE actual=$actual_tree" >&2
  exit 2
fi

identity="$DEXTER_REVISION:$DEXTER_SLICER_TREE"
if [[ ! -f "$source_root/.aim-identity" ]] ||
   [[ "$(<"$source_root/.aim-identity")" != "$identity" ]]; then
  mkdir -p "$(dirname "$source_root")"
  stage="$(mktemp -d "$(dirname "$source_root")/.slicer.XXXXXX")"
  git -C "$download_repo" archive "$DEXTER_REVISION" slicer |
    tar -x -C "$stage" --strip-components=1
  printf '%s\n' "$identity" > "$stage/.aim-identity"
  if [[ -e "$source_root" ]]; then
    old="$source_root.old.$$"
    mv "$source_root" "$old"
    mv "$stage" "$source_root"
    rm -rf "$old"
  else
    mv "$stage" "$source_root"
  fi
fi

echo "dexter-slicer-sync: $DEXTER_TAG revision=$DEXTER_REVISION tree=$DEXTER_SLICER_TREE"
