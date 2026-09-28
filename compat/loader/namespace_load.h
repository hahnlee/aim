#pragma once
#include "namespace_handles.h"
#include "aim_elf_loader.h"
#include <span>

namespace aim::loader {
struct LoadedGraphDrop { void operator()(AimElfGraphHandle*) const; };
struct LoadedGraph {
  explicit LoadedGraph(AimElfGraphHandle* value = nullptr) : graph(value) {}
  AimElfGraphHandle* get() const { return graph.get(); }
  explicit operator bool() const { return bool(graph); }
  std::unique_ptr<AimElfGraphHandle, LoadedGraphDrop> graph;
  std::vector<DiscoveredPlacement> placements;
};
// Input must be a NamespaceHandles::Discover result. Consumes staging bytes;
// Retains actual native image leases and copied publication placement after
// eager load, not staging bytes or the discovery context.
// Discovery owns all participating namespace global snapshots and scope input.
// Without an explicit lifecycle, creates and retains the Bionic DSO/VM/image
// owner from exact discovery placements. The process VM must be installed.
LoadedGraph LoadNamespaceGraph(DiscoveredGraph,
    const AimElfLifecycleCallbacks*, std::string* error,
    const AimElfLifecycleOwner* owned_lifecycle = nullptr);
// Same ownership, but no constructors: register through the namespace owner
// before initialization. Caller serializes opens and readiness transitions.
LoadedGraph LinkNamespaceGraph(DiscoveredGraph,
    const AimElfLifecycleCallbacks*, std::string* error,
    const AimElfLifecycleOwner* owned_lifecycle = nullptr);
}
