#pragma once
#include "configured_namespaces.h"
#include "darwin_art_elf_loader.h"
#include "namespace_scope_snapshot.h"
#include <mutex>
#include <atomic>
#include <unordered_map>
#include <vector>
#include <android-base/unique_fd.h>

struct android_namespace_t;
struct DarwinArtElfDiscoveredGraph;
struct DarwinArtElfLifecycleCallbacks;

namespace darwin_art::loader {
struct LoadedGraph;
struct NamespaceFile {
  android::base::unique_fd fd;
  std::string canonical_path;
  android_namespace_t* target = nullptr;
};
struct DiscoveredGraphDrop {
  void operator()(DarwinArtElfDiscoveredGraph*) const;
};
struct DiscoveredPlacement {
  uint64_t namespace_id;
  std::string canonical_path;
  std::string soname;
  DarwinArtElfFileIdentity file{};
};
// Staging bytes and placement have one lifetime. Placement order matches the
// ELF source array; paths are copied before the admission context is released.
struct DiscoveredGraph {
  explicit DiscoveredGraph(DarwinArtElfDiscoveredGraph* value = nullptr) : graph(value) {}
  DarwinArtElfDiscoveredGraph* get() const { return graph.get(); }
  explicit operator bool() const { return bool(graph); }
  void reset(DarwinArtElfDiscoveredGraph* value = nullptr) {
    graph.reset(value);
    placements.clear();
    scopes.reset();
  }
  std::unique_ptr<DarwinArtElfDiscoveredGraph, DiscoveredGraphDrop> graph;
  std::vector<DiscoveredPlacement> placements;
  std::unique_ptr<NamespaceScopeSnapshot> scopes;
};
// Android's opaque ABI never carries a Rust pointer or a numeric registry ID.
// This owner retains both stable handles and their registry. Callers must drain
// operations and discard handles before destruction (linker process lifetime).
// Library image loading and ClassLoader policy are not owned here.
class NamespaceHandles {
 public:
  explicit NamespaceHandles(std::unique_ptr<ConfiguredNamespaces> configured);
  android_namespace_t* Exported(const char* name);
  // Private creation boundary: caller/image lookup must resolve the effective
  // parent first. This does not implement the guest android_create_namespace
  // flag API or exempt-list policy. Paths must already be guest-canonical.
  android_namespace_t* CreateChild(android_namespace_t* effective_parent,
      bool isolated, bool shared, bool anonymous, const char* search_paths,
      const char* default_paths, const char* permitted_paths, std::string* error,
      const char* name = nullptr);
  android_namespace_t* CreateForAddress(uintptr_t caller, const char* name,
      const char* search, const char* defaults, uint64_t type, const char* permitted,
      android_namespace_t* parent, std::string* error);
  // Diagnostic label only; dynamic creation never adds an exported namespace.
  std::string Name(android_namespace_t*);
  // Initial anonymous namespace is default; first explicit assignment wins.
  android_namespace_t* Anonymous();
  // Original Bionic parent selection: explicit parent, otherwise caller's
  // original primary namespace, otherwise anonymous for an unmapped caller.
  // Caller image is the address-resolution result, not a SONAME guess.
  // Enclosing create/open must hold its linker operation across this selection.
  android_namespace_t* ParentForCaller(android_namespace_t* explicit_parent,
      const LinkerImageLease* caller, std::string* error);
  // ELF images only. Mach-O/provider caller lookup remains a separate boundary.
  int FindElfCaller(uintptr_t address, LinkerImageLease**, std::string* error);
  // Public opens retain the admitted image's owned Bionic lifecycle.
  uintptr_t OpenLibrary(uintptr_t caller, const char* path, int flags,
      uint64_t extension_flags, android_namespace_t* explicit_namespace, std::string* error);
  int LibraryImage(uintptr_t handle, LinkerImageLease**);
  int CloseLibrary(uintptr_t handle, std::string* error);
  uintptr_t LibrarySymbol(uintptr_t handle, const char* symbol, std::string* error,
      const char* version = nullptr, LinkerImageLease** defining_image = nullptr);
  uintptr_t SymbolForCaller(uintptr_t caller, uintptr_t handle,
      const char* symbol, std::string* error, const char* version = nullptr);
  // Resident lookup only, NOT dlopen. Result 0 transfers an owned image lease;
  // 1 means not resident, negative means invalid handle/registry failure.
  int FindResident(android_namespace_t*, const char* soname, LinkerImageLease**);
  int FindResidentFile(android_namespace_t*, const DarwinArtElfFileIdentity&, LinkerImageLease**,
      bool search_links = true);
  // Private SONAME/local operation, not guest dlopen: explicit namespace,
  // no path/FD/flag interpretation. 0 owned lease, 1 absent, -1 failure.
  int OpenLocalSoname(android_namespace_t*, const char* soname,
      const DarwinArtElfLifecycleCallbacks*, LinkerImageLease**, std::string* error, bool nodelete = false);
  // Explicit path in selected namespace (linked-namespace selection is external).
  int OpenLocalPath(android_namespace_t*, const char*, const DarwinArtElfLifecycleCallbacks*,
      LinkerImageLease**, std::string* error);
  // Ordinary path load: caller namespace then permitted direct links. No
  // exempt-list, guest flag interpretation or transitive namespace search.
  int OpenPath(android_namespace_t*, const char*, const DarwinArtElfLifecycleCallbacks*,
      LinkerImageLease**, std::string* error, bool nodelete = false);
  // File admission after resident lookup. Owns the host descriptor; no host
  // pathname reopen. Target handle remains borrowed from this owner. Result
  // 0 is admitted file, 1 not found, negative is failure; not a loaded DSO.
  int OpenFile(android_namespace_t*, const char* soname, const char* runpath,
               const char* source_image, NamespaceFile* output);
  // Inspect/discover only. File is an unmodified result of OpenFile, and the
  // caller prevents inode mutation. No provider SONAME bypasses or execution.
  // Current byte-graph API has one SONAME scope; not a cross-namespace loader.
  bool Discover(const NamespaceFile&, const char* soname, DiscoveredGraph*, std::string* error);
  // Publish all mapped files at their admitted placements; input stays owned.
  bool Publish(const LoadedGraph&, bool rtld_global, std::string* error,
      LinkerPublication** token = nullptr);
  // Already linked graph: publish before constructors, retract on init failure.
  // This is not a complete dlopen/reentrant-open scheduler.
  // Optional root result acquires its logical open before foreign constructors.
  // All three optional parameters must be supplied together.
  bool PublishAndInitialize(const LoadedGraph&, bool rtld_global, std::string* error,
      android_namespace_t* open_namespace = nullptr,
      const DarwinArtElfFileIdentity* open_identity = nullptr,
      LinkerImageLease** open_output = nullptr, bool nodelete = false);
  bool Link(android_namespace_t* from, android_namespace_t* to,
            const char* sonames, std::string* error);
  bool DefaultLibraryPath(std::string* output);
  void AppendWarning(const char* path, const char* message, const char* value);
  void ConsumeWarnings(void* context, void (*callback)(void*, const char*));
  bool UpdateLibrarySearchPath(const char* paths);
  bool InitializeAnonymous(const char* shared_sonames, const char* paths, std::string* error);
  // Process linker policy, shared by public ABI and serialized image loads.
  bool SetTargetSdkVersion(int target);
  int TargetSdkVersion() const { return target_sdk_version_.load(); }
  bool Set16KbAppCompatMode(bool enabled);
  bool Effective16KbAppCompatMode() const;
  // Linear dlsym phase only. 0 found, 1 absent, -1 invalid/unknown contract.
  // Full DEFAULT/NEXT also requires caller-local-group fallback on a miss.
  int LinearSymbol(android_namespace_t*, const LinkerImageLease* after,
      const char* symbol, uintptr_t* output, std::string* error, const char* version = nullptr);
 private:
  int LoadAdmittedFile(const NamespaceFile&, const char*, const DarwinArtElfLifecycleCallbacks*,
      LinkerImageLease**, std::string* error, bool search_links = true, bool nodelete = false);
  int OpenPathInNamespace(android_namespace_t*, const char*, const DarwinArtElfLifecycleCallbacks*,
      LinkerImageLease**, std::string*, bool search_links, bool nodelete = false);
  struct Token { uint64_t id; std::string name; };
  android_namespace_t* Intern(uint64_t id);
  uint64_t Resolve(android_namespace_t* handle) const;
  std::unique_ptr<ConfiguredNamespaces> configured_;
  std::mutex mutex_;
  std::unordered_map<uint64_t, std::unique_ptr<Token>> tokens_;
  uint64_t anonymous_id_ = 0;
  std::atomic<int> target_sdk_version_{0};
  std::atomic<bool> appcompat_16kb_{false};
};
}  // namespace darwin_art::loader
