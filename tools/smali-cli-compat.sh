#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
jar="$root/_aosp/google-smali/smali/build/libs/smali-3.0.7-dev-fat.jar"
if [ "${1:-}" != "${1#-J}" ]; then
  java_option=${1#-J}
  shift
  exec java "-$java_option" -jar "$jar" "$@"
fi
exec java -jar "$jar" "$@"
