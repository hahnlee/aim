#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/aosp-core-apps-graphics-validation.sh"
fixtures="$root/tools/tests/fixtures/aosp-core-apps-graphics"
client=4242

darwin_art_validate_hwui_blast_visible "$client" "$fixtures/positive.log"
if darwin_art_validate_hwui_blast_visible "$client" "$fixtures/invalid-source.log"; then
  echo 'invalid source evidence unexpectedly passed' >&2
  exit 1
fi
if darwin_art_validate_hwui_blast_visible "$client" "$fixtures/unrelated-client.log"; then
  echo 'unrelated client unexpectedly satisfied BLAST validation' >&2
  exit 1
fi
if darwin_art_validate_hwui_blast_visible "$client" "$fixtures/missing-target-drain.log"; then
  echo 'missing target drain unexpectedly passed' >&2
  exit 1
fi
if darwin_art_validate_hwui_blast_visible "$client" "$fixtures/transaction-mismatch.log"; then
  echo 'transaction mismatch unexpectedly passed' >&2
  exit 1
fi
if darwin_art_validate_hwui_blast_visible 9998 "$fixtures/positive.log"; then
  echo 'unknown client PID unexpectedly passed' >&2
  exit 1
fi
if darwin_art_validate_hwui_blast_visible 0 "$fixtures/positive.log"; then
  echo 'invalid client PID unexpectedly passed' >&2
  exit 1
fi

echo 'aosp-core-apps-graphics-validation: PASS fixtures=positive,invalid-source,unrelated-client,missing-target-drain,transaction-mismatch,unknown-pid'
