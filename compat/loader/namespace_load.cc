#include "namespace_load.h"
#include "admitted_provider_symbols.h"
#include "namespace_image_lifecycle.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include "vulkan_image.h"
#include <vector>

namespace aim::loader {
namespace {
void* Retain(void* value) {
  return aim_linker_image_clone(static_cast<LinkerImageLease*>(value));
}
void Release(void* value) { aim_linker_image_release(static_cast<LinkerImageLease*>(value)); }
}
void LoadedGraphDrop::operator()(AimElfGraphHandle* graph) const {
  aim_elf_graph_unload(&graph, nullptr);
}
LoadedGraph LoadNamespaceGraph(DiscoveredGraph discovered,
    const AimElfLifecycleCallbacks* lifecycle, std::string* error,
    const AimElfLifecycleOwner* owned_lifecycle) {
  auto graph = LinkNamespaceGraph(std::move(discovered), lifecycle, error, owned_lifecycle);
  if (!graph) return graph;
  char detail[4096]{};
  AimElfErrorBuffer buffer{detail, sizeof(detail), 0};
  if (aim_elf_graph_initialize(graph.get(), &buffer) != AIM_ELF_OK) {
    if (error) *error = detail;
    return LoadedGraph{};
  }
  return graph;
}
LoadedGraph LinkNamespaceGraph(DiscoveredGraph discovered,
    const AimElfLifecycleCallbacks* lifecycle, std::string* error,
    const AimElfLifecycleOwner* owned_lifecycle) {
  if (error) error->clear();
  auto fail = [error](const char* text) -> LoadedGraph {
    if (error) *error = text;
    return LoadedGraph{};
  };
  if (!discovered) return fail("missing discovered namespace graph");
  if (lifecycle && owned_lifecycle) return fail("two lifecycle owners supplied");
  if (!discovered.scopes || !discovered.scopes->ready()) return fail("missing admitted namespace scopes");
  OwnedImageLifecycle default_lifecycle;
  if (!lifecycle && !owned_lifecycle) {
    default_lifecycle = CreateNamespaceImageLifecycle(discovered, error);
    if (!default_lifecycle) return LoadedGraph{};
    owned_lifecycle = default_lifecycle.get();
  }
  const auto& globals = discovered.scopes->globals();
  size_t resident_count = 0;
  if (aim_elf_discovered_graph_resident_count(discovered.get(), &resident_count, nullptr) != AIM_ELF_OK)
    return fail("invalid discovered graph metadata");
  std::vector<AimElfNativeOwner> owners;
  for (size_t i = 0; i < resident_count; ++i) {
    const char* name = nullptr;
    uint64_t id = 0;
    void* lease = nullptr;
    if (aim_elf_discovered_graph_resident(discovered.get(), i, &name, &id, &lease, nullptr) != AIM_ELF_OK)
      return fail("invalid admitted resident");
    void* payload = nullptr;
    if (aim_linker_image_typed_payload(static_cast<LinkerImageLease*>(lease),
        AIM_IMAGE_BIONIC_PROVIDER, &payload) != 0 &&
        aim_linker_image_typed_payload(static_cast<LinkerImageLease*>(lease),
        AIM_IMAGE_ELF_SELECTED, &payload) != 0 &&
        !IsVulkanImage(static_cast<LinkerImageLease*>(lease)) &&
        !IsGraphicsNdkImage(static_cast<LinkerImageLease*>(lease)) &&
        !IsLinkerImage(static_cast<LinkerImageLease*>(lease)))
      return fail("unsupported admitted image representation");
    owners.push_back({lease, Retain, Release});
  }
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, ResolveAdmittedProvider, discovered.get()};
  char detail[4096]{};
  AimElfErrorBuffer buffer{detail, sizeof(detail), 0};
  AimElfGraphHandle* graph = nullptr;
  const auto status = owned_lifecycle
      ? aim_elf_discovered_graph_link_with_owned_lifecycle(discovered.get(),
      &options, owned_lifecycle, globals.data(), globals.size(),
      owners.data(), owners.size(), &graph, &buffer)
      : aim_elf_discovered_graph_link_with_owners(discovered.get(),
      &options, lifecycle, globals.data(), globals.size(),
      owners.data(), owners.size(), &graph, &buffer);
  LoadedGraph owned(graph);
  if (status != AIM_ELF_OK) return fail(*detail ? detail : aim_elf_status_name(status));
  owned.placements = std::move(discovered.placements);
  return owned;
}
}
