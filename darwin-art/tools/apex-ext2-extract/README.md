# Read-only APEX payload access

`apex-ext2-extract INPUT.apex OUTPUT /internal/file` extracts one new file.
`INPUT.apex - /directory` lists immediate names. Add `--inventory` to report
recursive inode modes, sizes and symlink targets without copying file contents.
Paths/targets are escaped in output; symlinks are never followed into the host.
Inventory traversal is separated in `src/inventory.rs`, with entry/depth caps
and repeated-directory rejection. Unsupported layouts fail explicitly.

This reports original payload contents, not installed module activation or
compatibility of its executables. Provisioning must preserve links/modes and
record actual installed payloads before publishing an APEX inventory.

`INPUT.apex NEW_DIRECTORY / --tree` materializes an offline payload tree.
The destination must not exist, and its parent must be a trusted build directory
not concurrently modified. The extractor creates directories/files before
symlinks and never resolves Android link targets against the host. It preserves
ordinary permission bits, not guest ownership, setuid/setgid bits or symlink
permissions. Unsupported inode types fail. On an I/O error the partial new tree
is retained for diagnosis; it must not be published as an installed module.
Absolute links remain Android paths and must only be resolved through the guest
filesystem at runtime. This command does not register or activate an APEX.

`bash tools/prepare-android16-apex-payloads.sh NEW_ABSOLUTE_BUILD_DIRECTORY`
prepares the six currently selected original core/module payloads, with archive hashes
from `upstream/android16-apex-payloads.lock`. It verifies the ps16k source image
before extracting the runtime APEX in-place from that image; other locked input
archives must already exist. Payloads, inventories and source receipts are kept
separate. Existing destinations are rejected, and a failed run has no completion
receipt. This does not certify executable compatibility or generate an active
APEX XML inventory. Nothing is copied into a user profile by this command.
