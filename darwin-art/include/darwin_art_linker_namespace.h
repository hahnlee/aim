#pragma once
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif

// Private runtime-owner API; never expose registry/lease pointers directly to
// guest code. Callers drain registry operations before destroy. Paths must be
// resolved through the guest filesystem (including symlinks) before admission.
typedef struct LinkerRegistry LinkerRegistry;
typedef struct LinkerOperation LinkerOperation;
// Recursive operation ownership, separate from data locks. Enter/leave on same
// thread; competing threads wait. Drain operations before registry destruction.
LinkerOperation* darwin_art_linker_operation_enter(const LinkerRegistry*);
int32_t darwin_art_linker_operation_leave(LinkerOperation*);
typedef struct LinkerImageLease LinkerImageLease;
typedef struct LinkerImageGroup LinkerImageGroup;
// Default paths only (not LD_LIBRARY_PATH). required includes NUL; 0 copied,
// 1 query/undersized without writing, negative invalid/unknown registry namespace.
int32_t darwin_art_linker_default_paths(LinkerRegistry*, uint64_t, uint8_t*, size_t, size_t* required);
// Resolved colon-delimited guest paths, null clears. Atomic replacement only;
// does not mutate default paths, links or existing child namespace snapshots.
int32_t darwin_art_linker_replace_search_paths(LinkerRegistry*, uint64_t, const char*);
// Owned ordered snapshot. global_only=1: DF_1_GLOBAL, 0: RTLD_GLOBAL.
// Snapshot retains resources independently of registry lifetime. No callback
// runs under its mutex. Item names/payloads are borrowed until group_destroy.
int32_t darwin_art_linker_group_snapshot(LinkerRegistry*, uint64_t id,
    uint8_t global_only, LinkerImageGroup** out_group);
// One namespace's complete soinfo order (not global-only or registry union).
// Caller applies symbol eligibility; leases remain owned until group_destroy.
int32_t darwin_art_linker_namespace_image_snapshot(const LinkerRegistry*, uint64_t,
    LinkerImageGroup**);
// 0 item, 1 end (outputs null), -1 invalid arguments.
int32_t darwin_art_linker_group_item(const LinkerImageGroup*, size_t index,
    void** out_payload, const char** out_soname);
void darwin_art_linker_group_destroy(LinkerImageGroup*);
// Exact resident identities retained once; NOT a symbol visibility grant.
// Call under one complete linker operation; no callback runs under its mutex.
int32_t darwin_art_linker_registry_image_snapshot(const LinkerRegistry*, LinkerImageGroup**);
// 0 independent non-counting lease, 1 end, -1 invalid; output always cleared.
int32_t darwin_art_linker_group_image(const LinkerImageGroup*, size_t, LinkerImageLease**);
typedef void* (*DarwinArtRetainImage)(void*);
typedef void (*DarwinArtReleaseImage)(void*);
// Trusted native representation, not Android flags. Legacy publications are
// opaque (0) and cannot be retrieved through typed accessors.
enum { DARWIN_ART_IMAGE_ELF_SELECTED = 1, DARWIN_ART_IMAGE_MACHO = 2,
       DARWIN_ART_IMAGE_BIONIC_PROVIDER = 3, DARWIN_ART_IMAGE_ANGLE = 4,
       DARWIN_ART_IMAGE_NATIVE_WINDOW = 5, DARWIN_ART_IMAGE_LINKER = 6,
       DARWIN_ART_IMAGE_VULKAN = 7, DARWIN_ART_IMAGE_GRAPHICS_NDK = 8 };
// Retain returns a DarwinArtElfSelectedImage (1), dlopen handle (2), or private
// Bionic provider image owner (3), ANGLE dispatch image owner (4), or
// NativeWindow facade image owner (5), or linker-owned ld-android ABI image
// (6), Vulkan dispatch image owner (7), or graphics NDK resident owner (8).
// Private owners are not dyld
// handles.
// Only the matching accessor may consume it; (4) is never a dyld handle.
int32_t darwin_art_linker_namespace_publish_image(LinkerRegistry*, uint64_t id,
    const char* soname, const char* resolved_path, void* source,
    DarwinArtRetainImage, DarwinArtReleaseImage, uint64_t flags_1,
    uint8_t global, uint32_t kind);
typedef struct DarwinArtImagePublication {
  uint64_t id;
  const char* soname;
  const char* path;
  void* source;
  DarwinArtRetainImage retain;
  DarwinArtReleaseImage release;
  uint64_t flags_1;
  uint8_t global;
  uint32_t kind;
  // Zero device/inode means unknown/native-only identity, never a match.
  uint64_t device, inode, offset;
} DarwinArtImagePublication;
int32_t darwin_art_linker_namespace_find_file(const LinkerRegistry*, uint64_t id,
    uint64_t device, uint64_t inode, uint64_t offset, LinkerImageLease**);
// All-or-none publication. Inputs borrowed, successful retained images owned
// by registry. Callbacks execute outside registry lock; failure releases every
// acquired reference. Same typed contract as publish_image. Empty batch is OK.
int32_t darwin_art_linker_namespace_publish_batch(LinkerRegistry*,
    const DarwinArtImagePublication*, size_t count);
typedef struct LinkerPublication LinkerPublication;
// Original local-group identity from the same ELF loader instance, not a
// dlopen count, address, persistent handle or permission to unload.
typedef struct DarwinArtLocalGroupMetadata {
  uint64_t id;
  uint8_t is_root;
} DarwinArtLocalGroupMetadata;
// Complete ELF-only batch, exactly one root per group. Metadata array parallels
// records. Optional output has the same rollback-token semantics below.
int32_t darwin_art_linker_namespace_publish_groups(LinkerRegistry*,
    const DarwinArtImagePublication*, const DarwinArtLocalGroupMetadata*,
    size_t count, LinkerPublication**);
// -4 means legacy publication with unknown group; failure clears output.
int32_t darwin_art_linker_image_group_metadata(const LinkerImageLease*,
    DarwinArtLocalGroupMetadata*);
// Exact registered local-group root; retained lease adds no logical open.
// Unknown legacy group=-4, invalid/unregistered=-1, inconsistent owner=-3.
int32_t darwin_art_linker_image_local_group_root(const LinkerRegistry*,
    const LinkerImageLease*, LinkerImageLease**);
// Effective flags of the original group's root, NOT a union of member flags.
// Survives lookup/rollback leases. -4 for unknown legacy group, clears outputs.
// Does not decide linked state, reference counts or permission to unload.
int32_t darwin_art_linker_image_group_retention_flags(const LinkerImageLease*,
    uint8_t* global, uint8_t* nodelete);
// Explicit ELF open ownership under the caller's linker operation. A clone
// shares one open; another acquire creates a distinct open. Last lease release
// balances that open, but does not yet unpublish/finalize the group. -4 unknown
// group, -5 counter exhaustion. This is not the public Android dlopen ABI.
int32_t darwin_art_linker_image_acquire_open(const LinkerImageLease*, LinkerImageLease**);
// Consume an exclusively owned open into a non-counting same-image view.
// Distinct owned source/empty output slots; serialized linker operation required.
// -7 means internal open aliases remain; -4 means source is not an open.
// Errors preserve source. No finalization/unpublication/dependency cascade.
int32_t darwin_art_linker_image_consume_open(LinkerImageLease**, LinkerImageLease**);
// Explicit opens only, excluding dependency refs and mapping/lookup Arc owners.
int32_t darwin_art_linker_image_open_count(const LinkerImageLease*, size_t*);
typedef struct DarwinArtGroupDependency {
  size_t source_index;
  uint64_t target_group;
} DarwinArtGroupDependency;
// Original outgoing edge order/multiplicity. 0 target, 1 end, -4 unknown;
// output cleared on error/end. Borrowed lease under linker operation. Target ID
// is metadata, not registry authority. Does not change any reference count.
int32_t darwin_art_linker_image_dependency_target(const LinkerImageLease*, size_t, uint64_t*);
// Resolve before source retirement under the same linker operation. Checks
// exact source membership AND original target ownership, not just a group ID.
// Returns non-counting owning root lease; 1 end, -1 foreign source, -3 broken
// target, -4 unknown graph. Output cleared; no dependency/open count changes.
int32_t darwin_art_linker_image_dependency_root(const LinkerRegistry*,
    const LinkerImageLease*, size_t, LinkerImageLease**);
// Complete original image edges, not a transitive closure or group-pair set.
// Target groups must be in this batch or the same registry. Atomic rollback.
int32_t darwin_art_linker_namespace_publish_linked_groups(LinkerRegistry*,
    const DarwinArtImagePublication*, const DarwinArtLocalGroupMetadata*, size_t,
    const DarwinArtGroupDependency*, size_t, LinkerPublication**);
// Additive ABI: immutable normalized SDK for newly linked images. Caller holds
// the linker operation across snapshot/publication. Existing records unchanged.
int32_t darwin_art_linker_namespace_publish_linked_groups_for_sdk(LinkerRegistry*,
    const DarwinArtImagePublication*, const DarwinArtLocalGroupMetadata*, size_t,
    const DarwinArtGroupDependency*, size_t, LinkerPublication**, int32_t);
// Returns -4 for legacy/unknown metadata, clearing output. Retained image lease
// owns its SDK even after registry removal; never reads current process SDK.
int32_t darwin_art_linker_image_target_sdk(const LinkerImageLease*, int32_t*);
// Linear dlsym phase only (not handle lookup). Uses effective GLOBAL plus the
// image's linked SDK. Unknown non-global metadata returns -4, output cleared.
int32_t darwin_art_linker_image_linear_lookup_eligible(const LinkerImageLease*, uint8_t*);
// Incoming dependency references only; not opens or permission to unload.
// Metadata-only publications have unknown edges and return -4, never zero.
int32_t darwin_art_linker_image_dependency_count(const LinkerImageLease*, size_t*);
typedef struct DetachedGroup DetachedGroup;
// Internal eligible detachment under the caller's complete linker operation.
// 0 detached, 1 retained by refs/root flags, -4 unknown, -1 invalid/retired.
// Does not consume opens, force finalization or implement public guest dlclose.
int32_t darwin_art_linker_group_detach(LinkerRegistry*, const LinkerImageLease*, DetachedGroup**);
typedef int32_t (*DarwinArtFinalizeGroup)(void* original_root_payload, void* context);
// Finalize while still registered, with opens blocked and no registry lock held.
// -6 means callback rejected before side effects: membership/admission restored.
// Caller must serialize the whole operation and arrange subsequent cascade.
int32_t darwin_art_linker_group_finalize_and_detach(LinkerRegistry*, const LinkerImageLease*,
    DarwinArtFinalizeGroup, void* context, DetachedGroup**);
// Release resources outside locks, after native execution has been quiesced.
void darwin_art_linker_detached_group_destroy(DetachedGroup*);
// Same operation with exact rollback identity. Destroy token before registry.
// Token destruction alone keeps publication; rollback removes memberships,
// including shared snapshots, without invalidating outstanding image leases.
int32_t darwin_art_linker_namespace_publish_transaction(LinkerRegistry*,
    const DarwinArtImagePublication*, size_t count, LinkerPublication**);
int32_t darwin_art_linker_publication_rollback(LinkerRegistry*, const LinkerPublication*);
void darwin_art_linker_publication_destroy(LinkerPublication*);
// Borrowed payload: 0 success, -4 representation mismatch, -1 invalid.
int32_t darwin_art_linker_image_typed_payload(const LinkerImageLease*,
    uint32_t kind, void** out_payload);
// Also returns 1 at end. Never skips mismatched images; caller chooses policy.
int32_t darwin_art_linker_group_typed_item(const LinkerImageGroup*, size_t index,
    uint32_t kind, void** out_payload);
LinkerRegistry* darwin_art_linker_registry_create(void);
void darwin_art_linker_registry_destroy(LinkerRegistry*);
// Results: 0 success, -1 invalid/denied, -2 poisoned owner, -3 retain failed.
// This creates owner state only, NOT an Android native-loader namespace API.
int32_t darwin_art_linker_namespace_create(LinkerRegistry*, uint8_t isolated,
    const char* search_paths, const char* default_search_paths,
    const char* permitted_paths, uint64_t shared_parent, uint64_t* out_id);
int32_t darwin_art_linker_namespace_link(LinkerRegistry*, uint64_t from, uint64_t to, const char* sonames);
// Linker configuration creation. allowed_libraries is colon-delimited names;
// null/empty means unrestricted names, not unrestricted filesystem access.
// Invalid configuration fails without publishing an ID (out_id=0).
int32_t darwin_art_linker_namespace_create_configured(LinkerRegistry*, uint8_t isolated,
    const char* search_paths, const char* default_search_paths,
    const char* permitted_paths, const char* allowed_libraries,
    uint64_t shared_parent, uint64_t* out_id);
// Explicit linker-config allow_all_shared_libs; neither ID may be zero.
// Grants only the direct target, never its transitive namespace links.
int32_t darwin_art_linker_namespace_link_all(LinkerRegistry*, uint64_t from, uint64_t to);
// Non-shared child inherits only the caller-selected real parent image group.
// Select from soinfo flags: default parent's DF_1_GLOBAL, otherwise RTLD_GLOBAL.
// Leases are borrowed through the call; every image must belong to parent.
// No parent paths/links are inherited. Same result codes as create.
int32_t darwin_art_linker_namespace_inherit(LinkerRegistry*, uint8_t isolated,
    const char* search_paths, const char* default_search_paths,
    const char* permitted_paths, uint64_t parent,
    const LinkerImageLease* const* group, size_t count, uint64_t* out_id);
int32_t darwin_art_linker_namespace_export(LinkerRegistry*, uint64_t id, const char* name);
// Returns 1 when the name has not been exported.
int32_t darwin_art_linker_namespace_exported(LinkerRegistry*, const char* name, uint64_t* out_id);
// Callback token is retained once; release must be infallible and thread-safe
// (schedule owner-thread cleanup if required). Neither runs under owner mutex.
int32_t darwin_art_linker_namespace_publish(LinkerRegistry*, uint64_t id, const char* soname,
    const char* resolved_path, void* source, DarwinArtRetainImage, DarwinArtReleaseImage);
// Publish actual mapped flags, plus Android RTLD_GLOBAL as 0/1 (not host bits).
int32_t darwin_art_linker_namespace_publish_flags(LinkerRegistry*, uint64_t id,
    const char* soname, const char* resolved_path, void* source,
    DarwinArtRetainImage, DarwinArtReleaseImage, uint64_t flags_1, uint8_t global);
// Caller serializes promotion and creation in the linker owner. Selects
// DF_1_GLOBAL for the real default parent, RTLD_GLOBAL for other parents.
int32_t darwin_art_linker_namespace_inherit_group(LinkerRegistry*, uint8_t isolated,
    const char* search_paths, const char* default_search_paths,
    const char* permitted_paths, uint64_t parent, uint8_t default_parent, uint64_t* out_id);
int32_t darwin_art_linker_image_promote_global(const LinkerImageLease*);
/* Effective flags only: not a group reference count or unload permission. */
int32_t darwin_art_linker_image_promote_nodelete(const LinkerImageLease*);
int32_t darwin_art_linker_image_retention_flags(const LinkerImageLease*, uint8_t* global, uint8_t* nodelete);
// Find additionally returns 1 for not resident, distinct from invalid/poisoned.
int32_t darwin_art_linker_namespace_find(LinkerRegistry*, uint64_t id,
    const char* soname, LinkerImageLease** out_lease);
void* darwin_art_linker_image_payload(const LinkerImageLease*);
void darwin_art_linker_image_release(LinkerImageLease*);
// Borrow input, return an independent reference; does not duplicate payload or
// broaden namespace visibility. Release result once. NULL input returns NULL.
LinkerImageLease* darwin_art_linker_image_clone(const LinkerImageLease*);
/* Original publishing namespace, unaffected by shared-child visibility.
 * Identity belongs to the original live registry; not an access grant. */
int32_t darwin_art_linker_image_primary_namespace(const LinkerImageLease*, uint64_t*);
// Exact current membership check, rejecting foreign/detached image identities.
// Hold the linker operation across this query and subsequent namespace use.
int32_t darwin_art_linker_image_registered_primary_namespace(const LinkerRegistry*, const LinkerImageLease*, uint64_t*);
// Exact primary/secondary membership only. Namespace links do not grant this.
// Returns -1 for foreign/unpublished images or unknown namespace, clearing out.
int32_t darwin_art_linker_image_namespace_member(const LinkerRegistry*, uint64_t,
    const LinkerImageLease*, uint8_t*);
int32_t darwin_art_linker_image_same(const LinkerImageLease*, const LinkerImageLease*, int32_t*);
// Address query over deduplicated registered images of the requested kind.
// Hold encompassing linker operation. Callback returns 1 match, 0 miss, <0
// error without changing publication; called with retained payload outside locks.
// Result 0 transfers a non-counting lease; 1 absent; negative failure/ambiguity.
typedef int32_t (*DarwinArtContainsImageAddress)(void*, uintptr_t, void*);
// Thread-owned copied diagnostic; dlerror consumes pending status, not storage.
int32_t darwin_art_linker_set_error(const char*);
char* darwin_art_linker_dlerror(void);
// Internal handle ownership under linker operation, not public dlopen/dlclose.
// Adopt consumes a unique opened lease; image borrows without an open count;
// take transfers one open to close coordination, reversible by re-adoption.
int32_t darwin_art_linker_handle_adopt(const LinkerRegistry*, LinkerImageLease**, uintptr_t*);
int32_t darwin_art_linker_handle_image(const LinkerRegistry*, uintptr_t, LinkerImageLease**);
int32_t darwin_art_linker_handle_take_open(const LinkerRegistry*, uintptr_t, LinkerImageLease**);
// Only after successful group retirement; invalidates guest handles outside
// foreign callbacks and refuses groups with outstanding opens.
int32_t darwin_art_linker_handles_retire_group(const LinkerRegistry*, uint64_t);
int32_t darwin_art_linker_find_image_address(const LinkerRegistry*, uint32_t kind,
    uintptr_t, DarwinArtContainsImageAddress, void* context, LinkerImageLease**);
// Private file search only: caller performs resident lookup first. Callbacks
// are the guest-image opener and Android errno getter. No callback holds the
// namespace mutex. Returns 0 + owned host fd/target namespace, 1 not found,
// -1 invalid/buffer too small, -2 poisoned, -3 I/O failure. Drain before destroy.
// canonical capacity includes NUL; all input/output buffers must not overlap.
// raw_runpath is unexpanded DT_RUNPATH, null if absent. When present, supply
// the referring image's canonical guest source_image and guest directory
// opener. Resolves ORIGIN/LIB and directory existence before search; never
// grants namespace access. APK archive-entry RUNPATH remains unsupported.
int32_t darwin_art_linker_namespace_open(LinkerRegistry*, uint64_t id,
    const char* soname, const char* raw_runpath, const char* source_image,
    int (*open_directory)(const char*, char*, size_t, int*),
    int (*open_image)(const char*, char*, size_t, int*), int (*android_errno)(void),
    char* canonical, size_t capacity, uint64_t* out_namespace, int* out_fd);
// Explicit path in selected namespace, not linked-name search. Canonical path
// and fd come from one guest filesystem admission. FD=-1/path empty on failure.
int32_t darwin_art_linker_namespace_open_path(const LinkerRegistry*, uint64_t,
    const char*, int (*)(const char*,char*,size_t,int*), int (*)(void),
    char* canonical, size_t capacity, int* out_fd);
// Also checks visible inode residency before new-file path permission. 2 returns
// an owned resident lease with fd=-1/path empty; otherwise the contract above.
// No namespace-link creation/search fallback and no FORCE_LOAD/FD-offset flags.
int32_t darwin_art_linker_namespace_open_path_resident(const LinkerRegistry*, uint64_t,
    const char*, int (*)(const char*,char*,size_t,int*), int (*)(void),
    char* canonical, size_t capacity, int* out_fd, LinkerImageLease** resident, uint8_t search_links);
int32_t darwin_art_linker_namespace_find_file_scoped(const LinkerRegistry*, uint64_t,
    uint64_t device, uint64_t inode, uint64_t offset, uint8_t search_links, LinkerImageLease**);
// Ordered direct link target for a path basename, 1=end; does not walk links of links.
int32_t darwin_art_linker_namespace_path_target(const LinkerRegistry*, uint64_t,
    const char* path, size_t index, uint64_t* target);
// Single-threaded, non-reentrant context for the admitted ELF graph callback.
// Root path must match the retained root fd; root image ID passed to graph=1.
// Destroy after discovery (including failure), before destroying registry.
// Preserves target namespace for descendants; not a multi-namespace symbol
// resolver or ClassLoader policy. Context owns path records, not returned fds.
typedef struct LinkerDiscovery LinkerDiscovery;
// Finished discovery's original primary membership and AOSP symbol visibility.
// Includes admitted direct-parent edges, not recursive namespace-link grants.
int32_t darwin_art_linker_discovery_image_scope(const LinkerDiscovery*, uint64_t image,
    uint64_t requester, uint64_t* primary, uint8_t* accessible);
LinkerDiscovery* darwin_art_linker_discovery_create(LinkerRegistry*, uint64_t,
    const char* canonical_root, int (*open_directory)(const char*,char*,size_t,int*),
    int (*open_image)(const char*,char*,size_t,int*), int (*android_errno)(void));
int darwin_art_linker_discovery_open(void*, uint64_t parent_image,
    const char* parent_soname, const char* name, const char* raw_runpath,
    uint64_t* out_image);
void darwin_art_linker_discovery_destroy(LinkerDiscovery*);
// Original file placement only; rejects resident/invalid IDs, clears outputs.
// Path borrowed until discovery destruction. IDs grant no new visibility.
int32_t darwin_art_linker_discovery_file_placement(const LinkerDiscovery*, uint64_t image,
    uint64_t* out_namespace, const char** out_canonical_path);
// Per-referrer resident lookup before file admission. Root parent ID=1.
// 0 transfers an owned lease, 1 not resident, negative invalid/failed. Retain
// lease through symbol use even after the discovery context is destroyed.
int32_t darwin_art_linker_discovery_find_resident(LinkerDiscovery*, uint64_t parent,
    const char* soname, LinkerImageLease** out_lease);
// Also retains the actual resident identity in discovery and returns its ID.
// Descendant lookup uses its primary namespace; same image yields same ID.
int32_t darwin_art_linker_discovery_admit_resident(LinkerDiscovery*, uint64_t parent,
    const char* soname, LinkerImageLease** out_lease, uint64_t* out_image);
// Exact stored image: owned lease, 1 for file entry, negative invalid ID.
int32_t darwin_art_linker_discovery_resident_image(const LinkerDiscovery*, uint64_t image,
    LinkerImageLease** out_lease);
#ifdef __cplusplus
}
#endif
