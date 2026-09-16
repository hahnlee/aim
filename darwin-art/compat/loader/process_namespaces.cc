#include "process_namespaces.h"
#include "namespace_handles.h"
#include <mutex>
namespace darwin_art::loader {
namespace {
std::mutex publication_mutex;
std::shared_ptr<NamespaceHandles> current;
}
bool InstallProcessNamespaces(std::shared_ptr<NamespaceHandles> owner) {
  if (!owner) return false;
  std::lock_guard lock(publication_mutex);
  if (current) return false;
  current = std::move(owner);
  return true;
}
std::shared_ptr<NamespaceHandles> AcquireProcessNamespaces() {
  std::lock_guard lock(publication_mutex);
  return current;
}
bool ShutdownProcessNamespaces(const std::shared_ptr<NamespaceHandles>& expected) {
  if (!expected) return false;
  std::shared_ptr<NamespaceHandles> retired;
  {
    std::lock_guard lock(publication_mutex);
    if (current != expected) return false;
    retired = std::move(current);
  }
  // Destruction and any native finalizers never run under publication lock.
  return true;
}
}
