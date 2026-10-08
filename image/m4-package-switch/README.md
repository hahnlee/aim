# Experimental native PackageManager image

This path creates an isolated C image for validation. The default image still
runs original PMS. Use a separate Git worktree with independent `target/` outputs
and the real original image tree; never point `--original` at a symlink.

Build the current device-services jar and `android-image-redirect` in that
worktree. From the primary checkout, prepare explicit artifacts:

```sh
python3 image/m4-package-switch/prepare.py prepare \
  --isolated-repo "$EXPERIMENT_REPO" \
  --out "$EXPERIMENT_REPO/target/aim/m4-package-switch" \
  --original "$ORIGINAL_TREE" \
  --redirect-cli "$EXPERIMENT_REPO/target/release/android-image-redirect"
```

The tool combines default redirects with the inactive C rows, replacing the
default PMS-main target, and adds the authoritative UM fixture rows. Its DEX
decoder requires exactly 14 original UM-to-PMS calls, zero remaining calls and
14 native bridge calls. Existing native service starts are removed through the
same symbolic patcher the default system-server node uses.

Copy the generated redirect table to `image/system-server-redirects` **only in
the isolated worktree**, then run its existing build owners:

```sh
cp "$EXPERIMENT_REPO/target/aim/m4-package-switch/system-server-redirects" \
   "$EXPERIMENT_REPO/image/system-server-redirects"
(cd "$EXPERIMENT_REPO" && cargo aim build system-server oat)
python3 image/m4-package-switch/prepare.py finalize \
  --isolated-repo "$EXPERIMENT_REPO" \
  --out "$EXPERIMENT_REPO/target/aim/m4-package-switch"
```

Keep the isolated checkout's baseline `image/native-services` unchanged during
this build: PMS starts through `main`, rather than a `startService` instruction.
The generated overlay supplies the experimental registration list afterward.

The existing `oat` owner recompiles services.jar and aim-services.jar against
the regenerated boot image and recompiles dependent class-loader contexts. It
converts the original profile checksums to the redirected DEX checksums and owns
the resulting odex/vdex files. Finalization refuses a different services.jar or
an oat stamp referencing older system-server/device-services keys. Reusing the
default image's compiled odex/vdex with this jar is unsupported.

Finalize also verifies that the prepared runtime registration pairs match
`native-services.experimental`. It writes `build-overlay.toml` with the graph's
actual services.jar path and the checked-in experimental registration list.

After finalization, install this manifest **only in the isolated worktree** and
use its existing derived-image owner:

```sh
cp "$EXPERIMENT_REPO/target/aim/m4-package-switch/build-overlay.toml" \
   "$EXPERIMENT_REPO/image/overlay.toml"
(cd "$EXPERIMENT_REPO" && cargo aim build derived-image)
```

The derived-image owner attaches the original image read-only and applies the
manifest to its own shadow. This also works when the original image's APFS
volume differs from the checkout's volume: direct `android-image assemble`
uses `clonefile` and cannot clone across those volumes. The original archive,
extracted tree, and default checkout remain unchanged.

The image remains experimental; successful image construction does not establish CTS,
app acceptance, or permission to activate the default image.
