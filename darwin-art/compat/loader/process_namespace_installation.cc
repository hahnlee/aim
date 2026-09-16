#include "process_namespace_installation.h"
#include "process_namespaces.h"

namespace darwin_art::loader {
std::unique_ptr<ProcessNamespaceInstallation> ProcessNamespaceInstallation::Create(
    const std::string& config_path, const std::string& executable_path,
    bool asan, bool hwasan, const std::string& library_path,
    const SharedBionicProviders& providers,
    std::span<const NativeProviderPlacement> placements, std::string* error) {
  if (error) error->clear();
  auto runtime = CreateNamespaceRuntime(config_path, executable_path, asan,
      hwasan, library_path, providers, placements, error);
  if (!runtime) return nullptr;
  // Allocate the scope before publishing: allocation failure cannot leave an
  // installed registry without its teardown owner.
  auto installation = std::unique_ptr<ProcessNamespaceInstallation>(
      new ProcessNamespaceInstallation(std::move(runtime)));
  if (!InstallProcessNamespaces(installation->owner_)) {
    installation->owner_.reset();
    if (error) *error = "process namespace registry already installed";
    return nullptr;
  }
  return installation;
}
ProcessNamespaceInstallation::~ProcessNamespaceInstallation() { Shutdown(); }
bool ProcessNamespaceInstallation::Shutdown() {
  if (!owner_) return true;
  if (!ShutdownProcessNamespaces(owner_)) return false;
  owner_.reset();
  return true;
}
}
