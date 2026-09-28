#ifndef AIM_ELF_LOADER_H_
#define AIM_ELF_LOADER_H_

#include <stddef.h>
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AIM_ELF_ABI_VERSION 1u

typedef enum AimElfStatus {
  AIM_ELF_OK = 0,
  AIM_ELF_INVALID_ARGUMENT = 1,
  AIM_ELF_IO = 2,
  AIM_ELF_FORMAT = 3,
  AIM_ELF_BOUNDS = 4,
  AIM_ELF_CAPABILITY = 5,
  AIM_ELF_PROTECTION = 6,
  AIM_ELF_RESOLVER = 7,
  AIM_ELF_UNRESOLVED_SYMBOL = 8,
  AIM_ELF_SYMBOL_NOT_FOUND = 9,
  AIM_ELF_INVALID_SYMBOL = 10,
  AIM_ELF_LIFECYCLE = 11,
  AIM_ELF_SYSTEM = 12,
  AIM_ELF_POISONED = 13,
  AIM_ELF_PANIC = 14,
} AimElfStatus;

typedef enum AimElfResolveStatus {
  AIM_ELF_RESOLVE_FOUND = 0,
  AIM_ELF_RESOLVE_NOT_FOUND = 1,
  AIM_ELF_RESOLVE_ERROR = 2,
} AimElfResolveStatus;

typedef struct AimElfErrorBuffer {
  char* data;
  size_t capacity;
  /* Required bytes including NUL. Zero means no error message. */
  size_t required;
} AimElfErrorBuffer;

typedef struct AimElfSymbolRequest {
  uint32_t abi_version;
  const char* symbol;
  /* Both are NULL for an unversioned symbol. */
  const char* version_soname;
  const char* version_name;
  uint16_t version_flags;
  uint8_t version_hidden;
  /* Non-zero when the undefined ELF symbol has STB_WEAK binding. */
  uint8_t symbol_weak;
  const char* const* needed_libraries;
  size_t needed_library_count;
} AimElfSymbolRequest;

typedef AimElfResolveStatus (*AimElfResolverCallback)(
    void* context,
    const AimElfSymbolRequest* request,
    uintptr_t* out_address,
    AimElfErrorBuffer* error);

typedef struct AimElfLoadOptions {
  uint32_t abi_version;
  AimElfResolverCallback resolver;
  void* resolver_context;
} AimElfLoadOptions;

typedef int (*AimElfPublishImageCallback)(void* context,
                                                uintptr_t start,
                                                uintptr_t end);
typedef int (*AimElfFinalizeImageCallback)(void* context,
                                                 uintptr_t start,
                                                 uintptr_t end);
typedef struct AimElfLifecycleCallbacks {
  uint32_t abi_version;
  AimElfPublishImageCallback publish_image;
  AimElfFinalizeImageCallback finalize_image;
  void* context;
} AimElfLifecycleCallbacks;
typedef struct AimElfLifecycleOwner AimElfLifecycleOwner;
/* Retain returns the callback context used by publish/finalize; release runs
 * after the last mapping reference. Source callbacks are copied. */
AimElfStatus aim_elf_lifecycle_owner_create(const AimElfLifecycleCallbacks*,
    void* (*retain)(void*), void (*release)(void*), AimElfLifecycleOwner**, AimElfErrorBuffer*);
void aim_elf_lifecycle_owner_destroy(AimElfLifecycleOwner*);

typedef struct AimElfHandle AimElfHandle;
typedef struct AimElfGraphHandle AimElfGraphHandle;
typedef struct AimElfInspection AimElfInspection;
typedef struct AimElfDiscoveredGraph AimElfDiscoveredGraph;

typedef struct AimElfGraphSource {
  const char* soname;
  const uint8_t* bytes;
  size_t length;
} AimElfGraphSource;

uint32_t aim_elf_abi_version(void);
const char* aim_elf_status_name(int32_t status);

/*
 * Pure byte inspection: parses Android arm64 ELF identity and DT_NEEDED without
 * mapping, relocating, or executing the image. Returned strings are opaque byte
 * strings terminated for C and remain borrowed until inspection_destroy.
 */
AimElfStatus aim_elf_inspect_bytes(
    const uint8_t* bytes,
    size_t length,
    AimElfInspection** out_inspection,
    AimElfErrorBuffer* error);
/* Borrows an already-admitted host fd and performs bounded positional reads.
 * Does not change its offset or reopen a path. Caller prevents concurrent inode
 * writes: stable descriptor identity is not an immutable-content guarantee.
 * Same inspection lifetime as inspect_bytes; namespace grants remain external. */
AimElfStatus aim_elf_inspect_fd(
    int fd, AimElfInspection** out_inspection, AimElfErrorBuffer* error);
AimElfStatus aim_elf_inspection_soname(
    const AimElfInspection* inspection,
    const char** out_soname,
    AimElfErrorBuffer* error);
AimElfStatus aim_elf_inspection_needed_count(
    const AimElfInspection* inspection,
    size_t* out_count,
    AimElfErrorBuffer* error);
AimElfStatus aim_elf_inspection_needed_at(
    const AimElfInspection* inspection,
    size_t index,
    const char** out_soname,
    AimElfErrorBuffer* error);
void aim_elf_inspection_destroy(AimElfInspection** inspection);
/* Raw DT_RUNPATH, borrowed until inspection_destroy. Null means absent;
 * expansion and namespace authorization remain the caller's responsibility. */
AimElfStatus aim_elf_inspection_runpath(
    const AimElfInspection*, const char** out_runpath, AimElfErrorBuffer* error);
// Raw DT_FLAGS_1, zero if absent. Not RTLD flags or proof the image is loadable.
// Failure leaves output untouched; caller retains inspection during this call.
AimElfStatus aim_elf_inspection_flags_1(
    const AimElfInspection*, uint64_t* out_flags, AimElfErrorBuffer* error);
// Query actual loaded member (logical SONAME), not a file opened again.
// Graph must remain live. Missing member fails without changing output.
AimElfStatus aim_elf_graph_flags_1(
    const AimElfGraphHandle*, const char* soname, uint64_t* out_flags,
    AimElfErrorBuffer* error);

/*
 * Discovers a closed recursive graph below one caller-selected, already-open
 * directory fd. The root and every non-provider DT_NEEDED value are exact byte
 * components opened by the no-follow filesystem broker. Absolute/path-like
 * names, symlinks, non-regular nodes, and embedded SONAME mismatch fail closed.
 * Limits are fixed at 64 files, 64 MiB per file, 256 MiB total, and 255 bytes
 * per component. Provider SONAMEs are never looked up on disk. RPATH/RUNPATH,
 * dyld, and alternative path fallback are not consulted.
 * Filename walking itself is byte-preserving, but the current closed namespace
 * requires embedded SONAME/DT_NEEDED keys to be UTF-8 and rejects other bytes
 * with a format error after component-policy validation.
 *
 * The directory fd is borrowed and duplicated synchronously. Source pointers
 * remain borrowed from the discovered graph until graph_destroy; graph_load
 * copies their bytes and therefore may outlive the discovery handle.
 * out_root_is_elf is set after the first four bytes are read from the same
 * authorized root descriptor, even when a later cap/read/parse step fails; it
 * lets the host preserve non-ELF dyld behavior without masking ELF failures.
 * The caller owns selection of this trusted directory and must prevent writes
 * to graph files during discovery; same-descriptor fstat/read removes pathname
 * reopen races but cannot make a concurrently mutated inode immutable.
 */
AimElfStatus aim_elf_discover_sibling_graph(
    int library_directory_fd,
    const uint8_t* root_component,
    size_t root_component_length,
    const char* const* provider_sonames,
    size_t provider_count,
    int* out_root_is_elf,
    AimElfDiscoveredGraph** out_graph,
    AimElfErrorBuffer* error);
AimElfStatus aim_elf_discovered_graph_root_soname(
    const AimElfDiscoveredGraph* graph,
    const char** out_soname,
    AimElfErrorBuffer* error);
/* Host-only, single closed SONAME scope. Root fd is borrowed/duplicated, never
 * reopened by name. Each dependency callback receives referring image ID and SONAME/name
 * plus raw RUNPATH (null if absent), borrowed only during callback. No token
 * expansion is performed here. Callback transfers an owned host fd, or returns
 * negative failure. Provider names
 * bypass file discovery as in sibling discovery. Caller enforces namespace
 * permissions and prevents inode mutation. Null opener fails if deps exist.
 * Image IDs are caller-owned opaque keys (not namespace IDs or host fds).
 * Caller retains their canonical guest path/namespace context until this call
 * returns, including failure. A successful opener writes its admitted image ID
 * to out_image; descendants receive that exact ID, not a SONAME-derived key.
 * Zero is allowed for callers whose policy requires no per-image context.
 * Repeated admissions of the same selected image must return the same ID.
 * Every non-provider dependency edge is admitted, including cycles. The closed
 * graph rejects a duplicate SONAME with a different ID or file identity.
 * IDs are not retained by the returned byte graph.
 * This does not implement cross-namespace duplicate-SONAME identity. */
AimElfStatus aim_elf_discover_admitted_graph(
    int root_fd, uint64_t root_image, const uint8_t* root_name, size_t root_length,
    const char* const* providers, size_t provider_count,
    int (*open_dependency)(void*, uint64_t parent_image, const char* needed_by,
                           const char* name, const char* raw_runpath,
                           uint64_t* out_image), void* context,
    int* out_is_elf, AimElfDiscoveredGraph** out_graph, AimElfErrorBuffer* error);
/* Per-edge alternative without provider-name bypass. Callback 0 transfers
 * kind=1: fd>=0, resident/release=NULL; kind=2: fd=-1, live resident+release.
 * image is a stable nonzero identity. Nonzero callback result transfers nothing.
 * Release must not unwind and remains callable through graph destruction.
 * All root/file inputs must remain immutable during discovery. This remains
 * a closed SONAME graph: different identities under one name are rejected. */
typedef struct AimElfAdmission {
  uint32_t kind;
  int32_t fd;
  uint64_t image;
  void* resident;
  void (*release)(void*);
} AimElfAdmission;
AimElfStatus aim_elf_discover_resident_graph(
    int root_fd, uint64_t root_image, const uint8_t* root_name, size_t root_length,
    int (*admit)(void*, uint64_t, const char*, const char*, const char*, AimElfAdmission*),
    void* context, int* out_is_elf, AimElfDiscoveredGraph** out_graph,
    AimElfErrorBuffer* error);
AimElfStatus aim_elf_discovered_graph_sources(
    const AimElfDiscoveredGraph* graph,
    const AimElfGraphSource** out_sources,
    size_t* out_count,
    AimElfErrorBuffer* error);
typedef struct AimElfFileIdentity { uint64_t device, inode, offset; } AimElfFileIdentity;
// Exact retained fd identity; current whole-file discovery has offset=0.
AimElfStatus aim_elf_discovered_graph_file_identity(
    const AimElfDiscoveredGraph*, size_t index, AimElfFileIdentity*, AimElfErrorBuffer*);
/* Source index matches discovered_graph_sources. Returns the original caller
 * admission ID, not a namespace ID or permission grant. Output reset on error.
 * Resolve/copy placement before destroying the caller's discovery context. */
AimElfStatus aim_elf_discovered_graph_source_image(
    const AimElfDiscoveredGraph* graph, size_t index, uint64_t* output,
    AimElfErrorBuffer* error);
/* Extended discovery. Metadata returns ordered DT_NEEDED array/count, borrowed
 * while the transferred resident lease lives (not callback-local storage).
 * Nonzero fails discovery and releases leases; zero count permits NULL array.
 * NULL callback preserves legacy leaf-only residents. No visibility grant. */
AimElfStatus aim_elf_discover_resident_graph_with_edges(
    int root_fd, uint64_t root_image, const uint8_t* root_name, size_t root_length,
    int (*admit)(void*, uint64_t, const char*, const char*, const char*, AimElfAdmission*),
    int (*metadata)(void*, void* resident, const char* const** names, size_t* count),
    void* context, int* out_is_elf, AimElfDiscoveredGraph** out_graph,
    AimElfErrorBuffer* error);
/* Native residents admitted by discover_resident_graph. The returned name and
 * resident are borrowed: do not release them. Keep the discovered graph alive
 * throughout resolution/use, or retain a separate owner using its native API.
 * Invalid index/graph clears outputs and returns INVALID_ARGUMENT. */
AimElfStatus aim_elf_discovered_graph_resident_count(
    const AimElfDiscoveredGraph* graph, size_t* count,
    AimElfErrorBuffer* error);
AimElfStatus aim_elf_discovered_graph_resident(
    const AimElfDiscoveredGraph* graph, size_t index,
    const char** name, uint64_t* image, void** resident,
    AimElfErrorBuffer* error);

void aim_elf_discovered_graph_destroy(
    AimElfDiscoveredGraph** graph);

AimElfStatus aim_elf_load_bytes(
    const uint8_t* bytes,
    size_t length,
    const AimElfLoadOptions* options,
    AimElfHandle** out_handle,
    AimElfErrorBuffer* error);

AimElfStatus aim_elf_load_path(
    const char* path,
    const AimElfLoadOptions* options,
    AimElfHandle** out_handle,
    AimElfErrorBuffer* error);

/*
 * Atomically maps, eagerly relocates, and dependency-first initializes the
 * complete recursive graph rooted at root_soname. Every DT_NEEDED SONAME must
 * be present exactly once in sources or provider_sonames. The returned handle
 * is published only after the complete graph succeeds.
 */
AimElfStatus aim_elf_graph_load(
    const char* root_soname,
    const AimElfGraphSource* sources,
    size_t source_count,
    const char* const* provider_sonames,
    size_t provider_count,
    const AimElfLoadOptions* options,
    AimElfGraphHandle** out_handle,
    AimElfErrorBuffer* error);

/*
 * Graph load with per-image Bionic destructor ownership. Every reservation is
 * published after relocation and before constructors. finalize_image is called
 * dependent-first, while the image is live, before DT_FINI_ARRAY/DT_FINI and
 * unmapping. The callback record and context must outlive the graph.
 */
AimElfStatus aim_elf_graph_load_with_lifecycle(
    const char* root_soname,
    const AimElfGraphSource* sources,
    size_t source_count,
    const char* const* provider_sonames,
    size_t provider_count,
    const AimElfLoadOptions* options,
    const AimElfLifecycleCallbacks* lifecycle,
    AimElfGraphHandle** out_handle,
    AimElfErrorBuffer* error);

typedef struct AimElfGlobalSource {
  // Owning graph or borrowed read-only selected-image source. A source returned
  // by selected_image_source may only be used for global input/selection of its
  // own SONAME, not graph initialize/finalize/clone or unrelated member access.
  const AimElfGraphHandle* graph;
  const char* soname;
} AimElfGlobalSource;
typedef struct AimElfSelectedImage AimElfSelectedImage;
// Synchronous original local-group finalizers only; no unmap/dependency close.
// Caller must establish eligibility, complete parent ownership and quiescence;
// no later library-state use is permitted. This is not public guest dlclose.
AimElfStatus aim_elf_selected_group_finalize(
    const AimElfSelectedImage*, AimElfErrorBuffer*);
// Retain the original declared ELF dependency without searching a namespace.
// Native/ambiguous/unavailable edges fail and clear output. Release the result
// with selected_image_release; this is not an Android dlopen reference.
AimElfStatus aim_elf_selected_dependency_image(
    const AimElfSelectedImage* parent, const char* name,
    AimElfSelectedImage** output, AimElfErrorBuffer* error);
/* Original local-group identity in this loader instance; never an address or
 * a persistent handle. No reference counting or close eligibility implied. */
AimElfStatus aim_elf_selected_group_info(const AimElfSelectedImage*,
    uint64_t* id, uint8_t* is_root, AimElfErrorBuffer*);
// Typed owner suitable for a namespace payload. Retains its mapping group and
// dependencies, not the whole original graph; only
// selected image's symbols are offered via its borrowed global source.
AimElfStatus aim_elf_select_image(const AimElfGraphHandle*,
    const char* soname, AimElfSelectedImage**, AimElfErrorBuffer*);
AimElfStatus aim_elf_selected_image_clone(const AimElfSelectedImage*,
    AimElfSelectedImage**, AimElfErrorBuffer*);
AimElfStatus aim_elf_selected_image_source(const AimElfSelectedImage*,
    AimElfGlobalSource*, uint64_t* flags_1, AimElfErrorBuffer*);
void aim_elf_selected_image_release(AimElfSelectedImage*);
/* Original PT_LOAD memory coverage only; query address is never dereferenced.
 * Excludes mapping holes, rounding and other images retained as dependencies. */
AimElfStatus aim_elf_selected_contains_address(const AimElfSelectedImage*,
    uintptr_t address, int32_t* contains, AimElfErrorBuffer*);
/* Exact executable PT_LOAD range [begin,end) containing address in this
 * retained image. Data segments, mapping holes and other images return
 * AIM_ELF_SYMBOL_NOT_FOUND with both outputs cleared. */
AimElfStatus aim_elf_selected_executable_range(
    const AimElfSelectedImage*, uintptr_t address, uintptr_t* begin,
    uintptr_t* end, AimElfErrorBuffer* error);
/* Exact retained mapping identity, not SONAME or file-byte equality. */
AimElfStatus aim_elf_selected_image_same(const AimElfSelectedImage*,
    const AimElfSelectedImage*, int32_t* same, AimElfErrorBuffer*);
/* Compare candidate against parent's retained DT_NEEDED ELF owner. Unknown or
 * non-ELF original identity is an error, never guessed from its SONAME. */
AimElfStatus aim_elf_selected_dependency_same(const AimElfSelectedImage*,
    const char* needed, const AimElfSelectedImage*, int32_t* same, AimElfErrorBuffer*);
/* Original retained native callback result for declared DT_NEEDED only.
 * Borrowed until selected image release; unavailable identity is an error. */
AimElfStatus aim_elf_selected_native_dependency(const AimElfSelectedImage*,
    const char* needed, void** borrowed_owner, AimElfErrorBuffer*);
/* DT_NEEDED in original order. Borrowed until image release; NULL at end.
 * Enumeration grants neither namespace access nor global symbol visibility. */
AimElfStatus aim_elf_selected_image_needed(const AimElfSelectedImage*,
    size_t index, const char** name, AimElfErrorBuffer*);
/* Lookup only the selected image. Null version selects a non-hidden export.
 * OK with address zero means absent. Returned address is borrowed from image. */
AimElfStatus aim_elf_selected_image_lookup(const AimElfSelectedImage*,
    const char* symbol, const char* version, uintptr_t* address, AimElfErrorBuffer*);
/* Android dynamic-linker export matching for the selected image. Same bounded
 * inputs and output contract as selected_image_lookup, but applies Android's
 * version metadata rules. OK with address zero means an Android lookup miss. */
AimElfStatus aim_elf_selected_image_lookup_android(
    const AimElfSelectedImage*, const char* symbol, const char* version,
    uintptr_t* address, AimElfErrorBuffer*);
/* Retained original local-group root from this image's graph, not namespace
 * re-search. Identity only; does not increment Android dlopen counts or grant
 * independent group-unmap eligibility. Release with selected_image_release. */
AimElfStatus aim_elf_selected_group_root(const AimElfSelectedImage*,
    AimElfSelectedImage**, AimElfErrorBuffer*);
/* Global array is ordered and borrowed during load only. Selected graphs are
 * retained by the result through finalizers/unload; owner locks are released
 * before callbacks. No private dependency exports are implicitly granted. */
AimElfStatus aim_elf_graph_load_with_globals(
    const char* root_soname, const AimElfGraphSource* sources, size_t source_count,
    const char* const* provider_sonames, size_t provider_count,
    const AimElfLoadOptions* options, const AimElfLifecycleCallbacks* lifecycle,
    const AimElfGlobalSource* globals, size_t global_count,
    AimElfGraphHandle** out_handle, AimElfErrorBuffer* error);

typedef struct AimElfImagePlacement {
  uint64_t image, namespace_id;
} AimElfImagePlacement;
typedef struct AimElfNamespaceScope {
  uint64_t namespace_id;
  const uint64_t* visible_images;
  size_t visible_count;
  const size_t* global_indices;
  size_t global_count;
} AimElfNamespaceScope;
/* Copies complete trusted linker decisions by original admitted image ID.
 * Discovery exclusively borrowed. Failure preserves the previous scope set.
 * global_count must match the subsequent load's ordered global-image array. */
AimElfStatus aim_elf_discovered_graph_set_namespace_scopes(
    AimElfDiscoveredGraph*, const AimElfImagePlacement*, size_t placement_count,
    const AimElfNamespaceScope*, size_t scope_count, size_t global_count,
    AimElfErrorBuffer*);
/* Snapshot the linker-owned 16KiB app-compat decision for this discovered
 * graph. Discovery remains metadata-only; the decision is applied to every
 * image only when the graph is linked. */
AimElfStatus aim_elf_discovered_graph_set_16kb_appcompat(
    AimElfDiscoveredGraph*, bool enabled, AimElfErrorBuffer*);

/* Borrowed owner record. retain returns an independently retained resource or
 * NULL on failure. release must be thread-safe, infallible and must not unwind.
 * A retained owner survives graph clones, selected/global images and finalizers.
 * Retained prefix is released on load/retain failure; input ownership unchanged.
 * These records grant lifetime only, not symbol lookup or namespace admission. */
typedef struct AimElfNativeOwner {
  void* context;
  void* (*retain)(void*);
  void (*release)(void*);
} AimElfNativeOwner;
/* Load admitted sources and resident edges together. Discovery stays borrowed;
 * owners must match resident order/count and exact admitted context pointers.
 * Retained results preserve named identity through finalizers and graph clones. */
AimElfStatus aim_elf_discovered_graph_load_with_owners(
    const AimElfDiscoveredGraph*, const AimElfLoadOptions*,
    const AimElfLifecycleCallbacks*, const AimElfGlobalSource*, size_t global_count,
    const AimElfNativeOwner*, size_t owner_count,
    AimElfGraphHandle**, AimElfErrorBuffer*);
/* Same inputs, but constructors deferred. Caller owns publication/rollback. */
AimElfStatus aim_elf_discovered_graph_link_with_owners(
    const AimElfDiscoveredGraph*, const AimElfLoadOptions*,
    const AimElfLifecycleCallbacks*, const AimElfGlobalSource*, size_t global_count,
    const AimElfNativeOwner*, size_t owner_count,
    AimElfGraphHandle**, AimElfErrorBuffer*);
/* Same discovery/resident identities, but lifecycle context is owned by all
 * resulting mappings. Source lifecycle handle can be destroyed after return. */
AimElfStatus aim_elf_discovered_graph_link_with_owned_lifecycle(
    const AimElfDiscoveredGraph*, const AimElfLoadOptions*,
    const AimElfLifecycleOwner*, const AimElfGlobalSource*, size_t global_count,
    const AimElfNativeOwner*, size_t owner_count,
    AimElfGraphHandle**, AimElfErrorBuffer*);
/* Clones graph under lock then releases lock before constructors. Duplicate
 * initialization is an error; linker owner handles competing/reentrant opens. */
AimElfStatus aim_elf_graph_initialize(
    const AimElfGraphHandle*, AimElfErrorBuffer*);
/* Destructor phase only, not dlclose. Owner must establish group eligibility,
 * serialize execution, retain mappings/registry during callbacks, and prohibit
 * later use of torn-down library state. Repeated calls do not repeat callbacks.
 * No graph mutex spans guest code; mapping release remains a separate phase. */
AimElfStatus aim_elf_graph_finalize(
    const AimElfGraphHandle*, AimElfErrorBuffer*);
AimElfStatus aim_elf_graph_load_with_owners(
    const char* root_soname, const AimElfGraphSource* sources, size_t source_count,
    const char* const* provider_sonames, size_t provider_count,
    const AimElfLoadOptions* options, const AimElfLifecycleCallbacks* lifecycle,
    const AimElfGlobalSource* globals, size_t global_count,
    const AimElfNativeOwner* owners, size_t owner_count,
    AimElfGraphHandle** out_handle, AimElfErrorBuffer* error);

AimElfStatus aim_elf_graph_lookup_root(
    AimElfGraphHandle* handle,
    const char* name,
    uintptr_t* out_address,
    AimElfErrorBuffer* error);

/* Looks up a root DSO export of any supported ELF symbol type. */
AimElfStatus aim_elf_graph_lookup_root_symbol(
    AimElfGraphHandle* handle,
    const char* name,
    uintptr_t* out_address,
    AimElfErrorBuffer* error);

/* Retains the complete graph and all of its mappings in a new unique handle. */
AimElfStatus aim_elf_graph_clone(
    AimElfGraphHandle* handle,
    AimElfGraphHandle** out_handle,
    AimElfErrorBuffer* error);

/* Finalizes dependents before dependencies, then nulls the unique handle. */
AimElfStatus aim_elf_graph_unload(
    AimElfGraphHandle** handle,
    AimElfErrorBuffer* error);

AimElfStatus aim_elf_run_initializers(
    AimElfHandle* handle,
    AimElfErrorBuffer* error);

AimElfStatus aim_elf_lookup(
    AimElfHandle* handle,
    const char* name,
    uintptr_t* out_address,
    AimElfErrorBuffer* error);

/* Idempotent for *handle == NULL. On success, sets *handle to NULL. */
AimElfStatus aim_elf_unload(
    AimElfHandle** handle,
    AimElfErrorBuffer* error);

/*
 * Ownership and lifetime contract:
 *
 * - A non-NULL handle returned by load is uniquely owned by the caller and must
 *   be released exactly once with aim_elf_unload.
 * - Init and lookup may occur on threads different from load and are internally
 *   serialized. Reentrant operations on the same handle from an initializer are
 *   forbidden.
 * - Unload may run on any thread, but the caller must first establish quiescence:
 *   no concurrent ABI operation and no execution through a borrowed lookup or
 *   imported function address. The raw handle does not provide reference counting.
 * - Resolver request strings/arrays are borrowed and valid only during the
 *   synchronous callback. The callback/context itself is not retained because
 *   all supported relocations are eager and lazy PLT binding is rejected.
 * - Every FOUND provider address must remain ABI-compatible and valid until the
 *   loaded handle is unloaded. The loader never falls back to Darwin's global
 *   symbol namespace.
 * - A lookup address is borrowed from the handle and becomes invalid at unload.
 *   The caller owns the ABI cast and must run required init arrays before use.
 * - A NativeBridge open adapter must keep a newly loaded handle private, run
 *   initializers and any preflight atomically, and publish it only on success. On
 *   failure it must unload the still-private handle after quiescing callbacks.
 * - Resolver callbacks must not throw C++/Objective-C exceptions or unwind.
 *   Rust panics originating inside this library are contained and reported as
 *   AIM_ELF_PANIC.
 * - Graph source/provider arrays and strings are borrowed only for graph_load.
 *   The graph copies source bytes and retains no resolver callback after eager
 *   relocation. Graph lookup addresses remain borrowed until graph_unload.
 */

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // AIM_ELF_LOADER_H_
