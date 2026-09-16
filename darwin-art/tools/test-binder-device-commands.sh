#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
bash tools/build-original-binder-core.sh
cargo test -p darwin-art-binder-device
"$project_root/_build/original-binder-core/device-uapi-test" --commands | \
  cargo run -p darwin-art-binder-device --example verify_catalog
"$project_root/_build/original-binder-core/device-uapi-test" --objects | \
  cargo run -p darwin-art-binder-device --example verify_objects
"$project_root/_build/original-binder-core/device-uapi-test" --object-fields | \
  cargo run -p darwin-art-binder-device --example verify_object_fields
"$project_root/_build/original-binder-core/device-uapi-test" --node-work | \
  cargo run -p darwin-art-binder-device --example verify_node_work
"$project_root/_build/original-binder-core/device-uapi-test" --transactions | \
  cargo run -p darwin-art-binder-device --example verify_transactions
