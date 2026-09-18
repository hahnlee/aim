#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
bash "$root/tools/tests/root-key-ingress-test.sh" root-key-ingress-lifetime-test.mm
