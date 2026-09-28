#include "namespace_handles.h"
#include "namespace_admission.h"
#include "aim_elf_loader.h"
#include "aim_guest_image.h"
#include "aim_bionic_errno.h"
#include <cstring>

namespace aim::loader {
void DiscoveredGraphDrop::operator()(AimElfDiscoveredGraph* graph) const {
  aim_elf_discovered_graph_destroy(&graph);
}

bool NamespaceHandles::Discover(const NamespaceFile& file, const char* soname,
    DiscoveredGraph* output, std::string* error) {
  if (error) error->clear();
  auto fail = [error](const char* text) {
    if (error) *error = text;
    return false;
  };
  if (!output) return fail("missing discovery output");
  output->reset();
  std::lock_guard lock(mutex_);
  const auto id = Resolve(file.target);
  if (!id || file.fd.get() < 0 || !soname || !*soname)
    return fail("invalid admitted namespace file");
  std::unique_ptr<LinkerDiscovery, decltype(&aim_linker_discovery_destroy)> context(
      aim_linker_discovery_create(configured_->registry(), id,
          file.canonical_path.c_str(), aim_fs_open_native_directory,
          aim_fs_open_native_image, aim_bionic_errno_load),
      aim_linker_discovery_destroy);
  if (!context) return fail("cannot create admitted discovery context");
  char detail[4096]{};
  AimElfErrorBuffer buffer{detail, sizeof(detail), 0};
  AimElfDiscoveredGraph* graph = nullptr;
  int is_elf = 0;
  NamespaceAdmission admission{context.get()};
  const auto status = aim_elf_discover_resident_graph_with_edges(file.fd.get(), 1,
      reinterpret_cast<const uint8_t*>(soname), std::strlen(soname),
      AdmitNamespaceDependency, ReadNamespaceResidentDependencies, &admission, &is_elf, &graph, &buffer);
  DiscoveredGraph owned(graph);
  if (status != AIM_ELF_OK) return fail(*detail ? detail : aim_elf_status_name(status));
  const AimElfGraphSource* sources = nullptr;
  size_t count = 0;
  if (aim_elf_discovered_graph_sources(graph, &sources, &count, &buffer) != AIM_ELF_OK)
    return fail("cannot read admitted graph sources");
  for (size_t index = 0; index < count; ++index) {
    uint64_t image = 0, target = 0;
    const char* path = nullptr;
    if (aim_elf_discovered_graph_source_image(graph, index, &image, &buffer) != AIM_ELF_OK ||
        aim_linker_discovery_file_placement(context.get(), image, &target, &path) != 0)
      return fail("missing admitted file placement");
    AimElfFileIdentity identity{};
    if (aim_elf_discovered_graph_file_identity(graph, index, &identity, &buffer) != AIM_ELF_OK)
      return fail("missing admitted file identity");
    owned.placements.push_back({target, path, sources[index].soname, identity});
  }
  owned.scopes = std::make_unique<NamespaceScopeSnapshot>();
  if (!owned.scopes->Capture(graph, context.get(), configured_->registry(), id, error)) return false;
  *output = std::move(owned);
  return true;
}
}  // namespace aim::loader
