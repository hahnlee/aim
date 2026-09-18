#!/usr/bin/env bash

# Validate the compositor evidence produced by HWUI's BLAST path. The
# transaction-builder diagnostic is not part of this contract: correlate the
# source pixels and central-queue records by transaction, require the expected
# Android raster, require the queue record to belong to the app client, and
# finally require AppKit to drain the queued target.
darwin_art_validate_hwui_blast_visible() {
  [[ "$#" == 2 ]] || {
    echo "usage: darwin_art_validate_hwui_blast_visible CLIENT_PID LOG" >&2
    return 64
  }

  local client="$1" log="$2"
  [[ "$client" =~ ^[1-9][0-9]*$ ]] || return 64
  [[ -r "$log" ]] || return 66

  LC_ALL=C awk -v client="$client" '
    function value(key, i, pair) {
      for (i = 1; i <= NF; ++i) {
        split($i, pair, "=");
        if (pair[1] == key) return pair[2];
      }
      return "";
    }
    function positive(number) {
      return number ~ /^[1-9][0-9]*$/;
    }
    /SurfaceFlinger: source pixels/ {
      transaction = value("transaction");
      if (positive(transaction) && value("size") == "720x1280" &&
          value("element") == "4" && positive(value("layer")) &&
          positive(value("surface")))
        sources[transaction] = 1;
    }
    /SurfaceFlinger: central compose queued/ {
      transaction = value("transaction");
      target = value("target");
      if (value("client") == client && positive(transaction) &&
          positive(value("layers")) && positive(target))
        targets[transaction] = target;
    }
    /SurfaceFlinger: AppKit scanout drain/ {
      target = value("target");
      if (positive(target)) scanned[target] = 1;
    }
    END {
      for (transaction in targets)
        if (sources[transaction] && scanned[targets[transaction]]) exit 0;
      exit 1;
    }
  ' "$log"
}
