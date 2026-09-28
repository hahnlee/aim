#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-openjdkjvmti.lock"
download_repo="$project_root/_downloads/android16-openjdkjvmti/art"
source_root="$project_root/_aosp/art/openjdkjvmti"

# shellcheck disable=SC1090
source "$lock_file"

if [[ ! -d "$download_repo/.git" ]]; then
  mkdir -p "$download_repo"
  git -C "$download_repo" init -q
  git -C "$download_repo" remote add origin \
    "https://android.googlesource.com/$OPENJDKJVMTI_PROJECT"
fi
if ! git -C "$download_repo" cat-file -e \
    "$OPENJDKJVMTI_REVISION^{commit}" 2>/dev/null; then
  git -C "$download_repo" fetch -q --depth=1 origin "$OPENJDKJVMTI_REVISION"
fi

actual_tree="$(git -C "$download_repo" rev-parse \
  "$OPENJDKJVMTI_REVISION:openjdkjvmti")"
if [[ "$actual_tree" != "$OPENJDKJVMTI_TREE" ]]; then
  echo "openjdkjvmti-sync: tree mismatch expected=$OPENJDKJVMTI_TREE actual=$actual_tree" >&2
  exit 2
fi

identity="$OPENJDKJVMTI_REVISION:$OPENJDKJVMTI_TREE"
if [[ ! -f "$source_root/.aim-identity" ]] ||
   [[ "$(<"$source_root/.aim-identity")" != "$identity" ]]; then
  stage="$(mktemp -d "$project_root/_aosp/art/.openjdkjvmti.XXXXXX")"
  git -C "$download_repo" archive "$OPENJDKJVMTI_REVISION" openjdkjvmti |
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

echo "openjdkjvmti-sync: revision=$OPENJDKJVMTI_REVISION tree=$OPENJDKJVMTI_TREE"
