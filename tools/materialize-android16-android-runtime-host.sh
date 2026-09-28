#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-android-runtime-host.lock"
source_root="$project_root/_aosp/frameworks/base/core/jni"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

relative=platform/host/HostRuntime.cpp
destination="$source_root/$relative"
mkdir -p "$(dirname "$destination")"
temporary="$(mktemp "${TMPDIR:-/tmp}/aim-host-runtime.XXXXXX")"
trap 'rm -f "$temporary"' EXIT

url="https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/$relative?format=TEXT"
curl -fsSL "$url" | base64 --decode > "$temporary"
actual="$(sha256 "$temporary")"
if [[ "$actual" != "$HOST_RUNTIME_CPP_SHA256" ]]; then
  echo "android-runtime-host: HostRuntime.cpp checksum mismatch" >&2
  echo "expected=$HOST_RUNTIME_CPP_SHA256 actual=$actual" >&2
  exit 3
fi
mv "$temporary" "$destination"

if find "$source_root/platform/host" -name .git -o -name .gitmodules | grep -q .; then
  echo "android-runtime-host: unexpected Git metadata" >&2
  exit 3
fi

echo "android-runtime-host: revision=$FRAMEWORKS_BASE_REVISION files=1 git-metadata=0"
