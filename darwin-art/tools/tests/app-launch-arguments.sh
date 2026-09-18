#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/app-launch-arguments.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/app-launch-arguments.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
unset DARWIN_ART_APP_COMMAND_LINE_FILE DARWIN_ART_APP_COMMAND_LINE
darwin_art_load_app_launch_arguments "$stage/missing"
[[ "${DARWIN_ART_APP_COMMAND_LINE+x}" != x ]]
printf '%s\n' darwin-art-app-launch-arguments-v1 \
  command-line-file=chrome-command-line \
  'command-line=--enable-skia-graphite --skia-graphite-dawn-backend=vulkan --no-first-run' > "$stage/valid"
darwin_art_load_app_launch_arguments "$stage/valid"
[[ "$DARWIN_ART_APP_COMMAND_LINE_FILE" == chrome-command-line ]]
[[ "$DARWIN_ART_APP_COMMAND_LINE" == '--enable-skia-graphite --skia-graphite-dawn-backend=vulkan --no-first-run' ]]
export DARWIN_ART_APP_COMMAND_LINE='' DARWIN_ART_APP_COMMAND_LINE_FILE=other-file
darwin_art_load_app_launch_arguments "$stage/valid"
[[ "$DARWIN_ART_APP_COMMAND_LINE" == '' && "$DARWIN_ART_APP_COMMAND_LINE_FILE" == other-file ]]
unset DARWIN_ART_APP_COMMAND_LINE_FILE DARWIN_ART_APP_COMMAND_LINE
printf '%s\n' darwin-art-app-launch-arguments-v1 \
  command-line-file=../outside command-line=bad > "$stage/invalid"
if darwin_art_load_app_launch_arguments "$stage/invalid"; then exit 1; fi
[[ "${DARWIN_ART_APP_COMMAND_LINE+x}" != x ]]
printf '%s\n' darwin-art-app-launch-arguments-v1 \
  command-line-file=valid command-line=first command-line=second > "$stage/duplicate"
if darwin_art_load_app_launch_arguments "$stage/duplicate"; then exit 1; fi
printf '%s\n' darwin-art-app-launch-arguments-v1 \
  command-line-file=valid 'command-line=$(touch NOT_EXECUTED)' > "$stage/literal"
darwin_art_load_app_launch_arguments "$stage/literal"
[[ "$DARWIN_ART_APP_COMMAND_LINE" == '$(touch NOT_EXECUTED)' ]]
[[ ! -e NOT_EXECUTED ]]
unset DARWIN_ART_APP_COMMAND_LINE_FILE DARWIN_ART_APP_COMMAND_LINE
printf '%s\n' darwin-art-app-launch-arguments-v1 \
  'command-line-file=실행 인자' 'command-line=--lang=ko' > "$stage/unicode"
darwin_art_load_app_launch_arguments "$stage/unicode"
[[ "$DARWIN_ART_APP_COMMAND_LINE_FILE" == '실행 인자' ]]
[[ "$DARWIN_ART_APP_COMMAND_LINE" == '--lang=ko' ]]
ln -s "$stage/valid" "$stage/symlink"
if darwin_art_load_app_launch_arguments "$stage/symlink"; then exit 1; fi
ln -s "$stage/missing-target" "$stage/dangling"
if darwin_art_load_app_launch_arguments "$stage/dangling"; then exit 1; fi
echo 'app launch arguments: defaults, explicit overrides, strict fields, literal transport PASS'
