#pragma once
#include "namespace_startup.h"

struct darwin_art_native_loader_config;

namespace darwin_art::loader {
// Owns publication, not Android ClassLoader policy. The caller must retain its
// filesystem/VM providers until this installation and all borrowed users drain.
// Original NativeLoader initialization follows successful installation; it is
// deliberately not called here and its preload failures must not be waived.
class ProcessNamespaceInstallation {
 public:
  // Consumes explicit host-owned startup inputs for the pinned, non-sanitized
  // Android image. No environment lookup and no NativeLoader initialization.
  // Input strings are borrowed for this call; bundle provenance is the caller's
  // responsibility. Publication retains all concrete system image resources.
  static std::unique_ptr<ProcessNamespaceInstallation> CreateFromConfig(
      const darwin_art_native_loader_config*, std::string* error);
  static std::unique_ptr<ProcessNamespaceInstallation> Create(
      const std::string& config_path, const std::string& executable_path,
      bool asan, bool hwasan, const std::string& library_path,
      const SharedBionicProviders&, std::span<const NativeProviderPlacement>,
      std::string* error);
  ~ProcessNamespaceInstallation();
  ProcessNamespaceInstallation(const ProcessNamespaceInstallation&) = delete;
  ProcessNamespaceInstallation& operator=(const ProcessNamespaceInstallation&) = delete;
  const std::shared_ptr<NamespaceHandles>& namespaces() const { return owner_; }
  // After ART/NativeLoader and foreign callbacks have drained. Idempotent; an
  // unexpected process slot is never removed. Borrowers retain image lifetime.
  bool Shutdown();
 private:
  explicit ProcessNamespaceInstallation(std::shared_ptr<NamespaceHandles> owner)
      : owner_(std::move(owner)) {}
  std::shared_ptr<NamespaceHandles> owner_;
};
}
