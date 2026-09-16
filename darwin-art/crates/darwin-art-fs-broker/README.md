# Darwin ART filesystem broker gate

This standalone crate is the first read-only filesystem authorization gate for
a previously selected mount. `ReadOnlyBroker` owns a directory file descriptor;
guest-relative byte paths are resolved one component at a time with Darwin
`openat(2)`. Intermediate components use `O_DIRECTORY | O_NOFOLLOW`, and the
leaf uses `O_NOFOLLOW`. A trailing slash additionally requires the leaf to be a
directory. Metadata is obtained from the opened descriptor, never by looking up
the path a second time.

## Strict broker restrictions

Symlinks are denied at every path component, including the final component.
The broker does not emulate Linux symlink resolution, does not return a
best-effort target, and does not turn unsupported cases into success. Callers
that need symlinks can explicitly select the separate `guest_path::GuestRoot`
resolver described below; the strict broker does not change behavior.

Paths are relative to the broker's mount-root descriptor. Empty bytes name the
mount root; absolute paths, NUL, `.`, `..`, repeated separators, and a lone
trailing separator are rejected before any lookup. Non-UTF-8 bytes are passed
unchanged to Darwin; the mounted filesystem may itself reject them (APFS
commonly returns `EILSEQ`) and that remains a hard error.

Holding every opened directory descriptor prevents a concurrent rename from
redirecting subsequent components to a replacement at the old pathname. As on
other systems without Linux `openat2(RESOLVE_BENEATH)`, the mount root and
namespace mutation authority remain part of the trust boundary: a party that
can relocate an already-open directory and then mutate that directory can
change what the descriptor itself contains.

Only regular files and directories are admitted. Other node types are rejected
after a nonblocking read-only open. This crate currently targets macOS/Darwin
only and fails to compile elsewhere rather than providing a weaker fallback.

## Guest-root resolver

`guest_path::GuestRoot` is a separate read-only resolver anchored to an owned
directory descriptor. It resolves relative symlink targets from the current
directory and absolute targets from that supplied root, handles dot/parent
components without escaping it, and bounds links to 40 and expanded paths to
4095 bytes. Intermediate descriptors are retained; ordinary opens never follow
symlinks. A concurrent replacement with a symlink fails rather than following
it through Darwin. The same directory-mutation trust boundary above applies.

The result pairs a walk-resolved byte path with the opened node. Consumers must
use that descriptor, not reopen the path after checking permissions. This is
not a Linux mount namespace or procfs magic-link implementation. The caller
supplies the correct guest root and may configure owned mount directory
descriptors at canonical guest prefixes before publishing the resolver. Links
can cross these mounts, while `..` at a mount root returns to its guest parent.
Process filesystem realpath uses this for full-root installs, including its
separate private `/data` root. Submount-only installs retain the strict broker.
Results also retain an opaque `MountOrigin`, scoped to this resolver instance.
`open_relative_with_origin` carries the directory owner's retained origin and
updates it on mount/symlink/parent crossings; another resolver's token fails.
The caller must keep the token paired with its opened FD through duplication
and cwd changes. A mount identity is not immutable-image authentication or
permission to trust inode xattrs. Unknown/host descriptors have no such token;
the older descriptor-only API intentionally supplies no provenance guarantee.
Private loader admission returns a canonical guest path paired with an owned
host file descriptor. Its directory variant resolves relative paths against
guest cwd and is composed with RUNPATH expansion in runtime contract tests.
Neither interface grants linker namespace access; APK entry resolution and
production NativeLoader integration remain incomplete.
Tests use a real temporary directory and
verify absolute/relative links, root parent handling, loops, directory-only
suffixes and retained file identity after replacement.
