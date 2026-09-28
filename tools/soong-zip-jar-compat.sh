#!/bin/sh
set -eu

# Minimal host implementation of the jar-producing soong_zip contracts used
# by pinned ART source generators: archive a directory, or explicit files.
case "${1:-}" in --jar|-j) shift;; esac
[ "${1:-}" = "-o" ] && [ "$#" -ge 3 ] || {
  echo "soong-zip-jar-compat: expected [-j|--jar] -o OUT ..." >&2
  exit 2
}
output=$2
shift 2
[ "${1:-}" = "-j" ] && shift
if [ "$#" -eq 4 ] && [ "$1" = "-C" ] && [ "$3" = "-D" ]; then
  [ "$2" = "$4" ] || {
    echo "soong-zip-jar-compat: -D must name the directory selected by -C" >&2
    exit 2
  }
  exec jar cf "$output" -C "$2" .
fi
set -- "$output" "$@"
output=$1
shift
files=
file_count=0
while [ "$#" -gt 0 ]; do
  [ "$1" = "-f" ] && [ "$#" -ge 2 ] || {
    echo "soong-zip-jar-compat: explicit archives accept only -f FILE" >&2
    exit 2
  }
  files="$files $2"
  file=$2
  file_count=$((file_count + 1))
  shift 2
done
[ -n "$files" ] || {
  echo "soong-zip-jar-compat: no archive inputs" >&2
  exit 2
}
# The pinned file names contain no shell whitespace; retaining their paths is
# the same behavior these run-test generators expect from soong_zip.
# shellcheck disable=SC2086
if [ "$file_count" -eq 1 ]; then
  exec jar cf "$output" -C "$(dirname "$file")" "$(basename "$file")"
fi
exec jar cf "$output" $files
