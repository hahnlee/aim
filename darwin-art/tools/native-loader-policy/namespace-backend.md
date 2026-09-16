# Android namespace backend

`tools/build-android-namespace-backend.sh` builds the explicit source manifest
into `_build/android-namespace-backend/libandroid-namespace-backend.a`.
The manifest excludes fixtures, NativeLoader lifecycle, NativeBridge lifecycle,
and the separately owned Bionic provider and ELF image registries. The test
links the existing image registry explicitly; production already owns its TU.
It is not a combined
runtime or a substitute for those owners.

The backend transports original Bionic configuration into the Rust namespace
registry, retains typed image/handle ownership, and supplies namespace and
library open/symbol/close ABI boundaries. Configuration parsing remains in
`libbionic-linker-config.a`; Java ClassLoader policy remains in original AOSP
NativeLoader. Startup must supply actual provider placements and a configured
guest filesystem before publishing the process namespace owner.

`tools/test-bionic-linker-config.sh` rebuilds and consumes this archive rather
than compiling a second private list of backend implementations. Its native
ELF/ICU/ANGLE checks validate the backend, not application lifecycle or Chrome.
The final production runtime has not yet switched to this namespace owner.
Do not link old and new NativeLoader owners together or treat an archive build
as evidence that app launch uses the new backend.

Symbol dispatch consumes the registry's typed representation: only a published
`DARWIN_ART_IMAGE_MACHO` payload is a dyld handle. Android numeric handles and
ELF/provider payloads must never be passed to host `dlsym`. The Mach-O test uses
an actual owned libSystem handle and verifies logical close versus publication
lifetime. This does not supply Mach-O admission or dylib selection policy.

ELF handle lookup now traverses original DT_NEEDED edges breadth-first with
AOSP primary/secondary/direct-primary-parent accessibility and subtree pruning.
Rust snapshots retain exact resident identities; selected-image dependency
APIs match the original ELF/native targets without fresh SONAME resolution.
Root exports take a fast path. A missing export searches the retained graph;
missing or ambiguous registered dependency identity is an explicit error.
Group-finalization roots are not used as substitutes for child image edges.
Real ELF fixtures check root precedence, sibling-before-grandchild order and
non-transitive namespace visibility. ART's non-NativeBridge call still requires
the original NativeLoader/startup ownership cutover before switching to this
ABI. RTLD_DEFAULT/NEXT and versioned public queries are separate unfinished APIs.
