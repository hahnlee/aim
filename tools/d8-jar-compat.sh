#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
java_args=()
d8_args=()
while (($#)); do
  case "$1" in
    # AOSP's d8 wrapper consumes -J-prefixed JVM arguments before invoking
    # the Java entrypoint. Preserve that contract for generated test builds.
    -J?*) java_args+=("-${1#-J}") ;;
    *) d8_args+=("$1") ;;
  esac
  shift
done
# macOS still ships Bash 3.2, where expanding an empty array under `set -u`
# raises "unbound variable". AOSP generators frequently invoke D8 without any
# -J options, so restore ordinary empty-array expansion only for the final exec.
set +u
exec java "${java_args[@]}" -cp "$project_root/_prebuilt/android-16/tools/r8.jar" \
  com.android.tools.r8.D8 "${d8_args[@]}"
