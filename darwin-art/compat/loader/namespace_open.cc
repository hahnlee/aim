#include "namespace_load.h"
#include "namespace_operation.h"
#include "namespace_open_reference.h"
#include "namespace_elf_group.h"
#include <cstring>
#include <sys/stat.h>

namespace darwin_art::loader {
int NamespaceHandles::OpenLocalSoname(android_namespace_t* handle, const char* name,
    const DarwinArtElfLifecycleCallbacks* lifecycle, LinkerImageLease** output,
    std::string* error, bool nodelete) {
  if (error) error->clear();
  auto fail = [error](const char* message) {
    if (error) *error = message;
    return -1;
  };
  if (!output) return fail("missing library output");
  *output = nullptr;
  if (!name || !*name || std::strchr(name, '/') || std::strchr(name, ':') ||
      std::strcmp(name, ".") == 0 || std::strcmp(name, "..") == 0)
    return fail("SONAME operation requires a library basename");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker operation");
  const int resident = FindResident(handle, name, output);
  if (resident == 0) return AcquireNamespaceOpen(output, error, nodelete);
  if (resident != 1) return fail("invalid resident namespace lookup");
  NamespaceFile file;
  const int opened = OpenFile(handle, name, nullptr, nullptr, &file);
  if (opened == 1) return 1;
  if (opened != 0) return fail("namespace file admission failed");
  return LoadAdmittedFile(file, name, lifecycle, output, error, true, nodelete);
}

int NamespaceHandles::LoadAdmittedFile(const NamespaceFile& file, const char* name,
    const DarwinArtElfLifecycleCallbacks* lifecycle, LinkerImageLease** output, std::string* error,
    bool search_links, bool nodelete) {
  auto fail = [error](const char* message) { if (error) *error = message; return -1; };
  struct stat metadata{};
  if (fstat(file.fd.get(), &metadata) != 0) return fail("cannot inspect admitted fd");
  DarwinArtElfFileIdentity identity{static_cast<uint64_t>(metadata.st_dev), metadata.st_ino, 0};
  const int resident = FindResidentFile(file.target, identity, output, search_links);
  if (resident == 0) return AcquireNamespaceOpen(output, error, nodelete);
  if (resident != 1) return fail("file identity lookup failed");
  DiscoveredGraph discovered;
  if (!Discover(file, name, &discovered, error)) return -1;
  char policy_error[1024]{};
  DarwinArtElfErrorBuffer policy_detail{policy_error, sizeof(policy_error), 0};
  if (darwin_art_elf_discovered_graph_set_16kb_appcompat(discovered.get(),
      Effective16KbAppCompatMode(), &policy_detail) != DARWIN_ART_ELF_OK)
    return fail(*policy_error ? policy_error : "cannot configure ELF page compatibility");
  auto graph = LinkNamespaceGraph(std::move(discovered), lifecycle, error);
  if (!graph || !PublishAndInitialize(graph, false, error, file.target, &identity, output, nodelete)) return -1;
  return 0;
}
}
