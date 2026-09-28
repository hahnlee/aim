#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/app-launch-arguments.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/app-launch-arguments.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
unset AIM_APP_COMMAND_LINE_FILE AIM_APP_COMMAND_LINE
aim_load_app_launch_arguments "$stage/missing"
[[ "${AIM_APP_COMMAND_LINE+x}" != x ]]
printf '%s\n' aim-app-launch-arguments-v1 \
  command-line-file=chrome-command-line \
  'command-line=--enable-skia-graphite --skia-graphite-dawn-backend=vulkan --no-first-run' > "$stage/valid"
aim_load_app_launch_arguments "$stage/valid"
[[ "$AIM_APP_COMMAND_LINE_FILE" == chrome-command-line ]]
[[ "$AIM_APP_COMMAND_LINE" == '--enable-skia-graphite --skia-graphite-dawn-backend=vulkan --no-first-run' ]]
export AIM_APP_COMMAND_LINE='' AIM_APP_COMMAND_LINE_FILE=other-file
aim_load_app_launch_arguments "$stage/valid"
[[ "$AIM_APP_COMMAND_LINE" == '' && "$AIM_APP_COMMAND_LINE_FILE" == other-file ]]
unset AIM_APP_COMMAND_LINE_FILE AIM_APP_COMMAND_LINE
printf '%s\n' aim-app-launch-arguments-v1 \
  command-line-file=../outside command-line=bad > "$stage/invalid"
if aim_load_app_launch_arguments "$stage/invalid"; then exit 1; fi
[[ "${AIM_APP_COMMAND_LINE+x}" != x ]]
printf '%s\n' aim-app-launch-arguments-v1 \
  command-line-file=valid command-line=first command-line=second > "$stage/duplicate"
if aim_load_app_launch_arguments "$stage/duplicate"; then exit 1; fi
printf '%s\n' aim-app-launch-arguments-v1 \
  command-line-file=valid 'command-line=$(touch NOT_EXECUTED)' > "$stage/literal"
aim_load_app_launch_arguments "$stage/literal"
[[ "$AIM_APP_COMMAND_LINE" == '$(touch NOT_EXECUTED)' ]]
[[ ! -e NOT_EXECUTED ]]
unset AIM_APP_COMMAND_LINE_FILE AIM_APP_COMMAND_LINE
printf '%s\n' aim-app-launch-arguments-v1 \
  'command-line-file=실행 인자' 'command-line=--lang=ko' > "$stage/unicode"
aim_load_app_launch_arguments "$stage/unicode"
[[ "$AIM_APP_COMMAND_LINE_FILE" == '실행 인자' ]]
[[ "$AIM_APP_COMMAND_LINE" == '--lang=ko' ]]
ln -s "$stage/valid" "$stage/symlink"
if aim_load_app_launch_arguments "$stage/symlink"; then exit 1; fi
ln -s "$stage/missing-target" "$stage/dangling"
if aim_load_app_launch_arguments "$stage/dangling"; then exit 1; fi
echo 'app launch arguments: defaults, explicit overrides, strict fields, literal transport PASS'
