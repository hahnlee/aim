#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
verify="$root/tools/tests/production-runtime-graph-boundary.sh"
fixtures="$root/tools/tests/native-graph-boundary"
bash "$verify" "$fixtures/isolated.ninja" product
if bash "$verify" "$fixtures/contaminated.ninja" product; then
  echo 'graph boundary test: transitive fixture escaped rejection' >&2
  exit 1
fi
if bash "$verify" "$fixtures/other-fixture.ninja" product; then
  echo 'graph boundary test: non-runtime nested fixture escaped rejection' >&2
  exit 1
fi
if bash "$verify" "$fixtures/isolated.ninja" missing-product; then
  echo 'graph boundary test: missing target falsely passed' >&2
  exit 1
fi
echo 'production graph boundary: isolation/transitive/nested-fixture rejection/query failure PASS'
