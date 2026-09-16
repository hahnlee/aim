#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-xsdc.lock"
src="$root/_aosp/android16-xsdc"
out="$root/_build/xsdc"
jdk=/opt/homebrew/opt/openjdk@17
if [[ ! -e "$src" ]]; then
  git init -q "$src"
  git -C "$src" remote add origin https://android.googlesource.com/platform/system/tools/xsdc
fi
[[ -d "$src/.git" ]]
if ! git -C "$src" rev-parse --verify HEAD >/dev/null 2>&1; then
  [[ -z "$(git -C "$src" status --porcelain)" ]]
  git -C "$src" fetch --depth=1 origin "$XSDC_REVISION"
  git -C "$src" -c advice.detachedHead=false checkout --detach "$XSDC_REVISION"
fi
[[ "$(git -C "$src" rev-parse HEAD)" == "$XSDC_REVISION" ]]
[[ -z "$(git -C "$src" status --porcelain)" ]]
mkdir -p "$out/classes"
dependency="$out/commons-cli-1.2.jar"
if [[ ! -f "$dependency" ]]; then
  staged="$(mktemp "$out/dependency.XXXXXX")"
  trap 'rm -f -- "$staged"' EXIT
  curl -fsSL https://repo.maven.apache.org/maven2/commons-cli/commons-cli/1.2/commons-cli-1.2.jar -o "$staged"
  [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$COMMONS_CLI_SHA256" ]]
  mv "$staged" "$dependency"
fi
[[ "$(shasum -a 256 "$dependency" | awk '{print $1}')" == "$COMMONS_CLI_SHA256" ]]
sources=()
while IFS= read -r source_file; do sources+=("$source_file"); done < <(rg --files "$src/src/main/java" -g '*.java' | sort)
"$jdk/bin/javac" --release 17 -cp "$dependency" -d "$out/classes" "${sources[@]}"
"$jdk/bin/jar" --create --file "$out/xsdc.jar" --main-class com.android.xsdc.Main -C "$out/classes" .
echo "AOSP xsdc built: $XSDC_REVISION ($out/xsdc.jar)"
