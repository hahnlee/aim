#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-document-panel.XXXXXX")
trap 'rm -rf "$stage"' EXIT
clang -std=c11 -Wall -Wextra -Werror -fsyntax-only -I"$root" \
  "$root/tools/tests/document-panel-abi-test.c"
clang++ -std=c++20 -fobjc-arc -Wall -Wextra -Werror \
  -Wno-deprecated-declarations -pthread -I"$root" \
  "$root/compat/filesystem/document_panel.mm" \
  "$root/tools/tests/document-panel-test.mm" \
  -framework AppKit -o "$stage/test"
"$stage/test"
