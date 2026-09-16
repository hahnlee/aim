#include "loader/process_namespace_installation.h"
#include "loader/process_namespaces.h"
#include <cassert>
#include <iostream>

void TestProcessNamespaceInstallation(
    const darwin_art::loader::SharedBionicProviders& providers,
    std::span<const darwin_art::loader::NativeProviderPlacement> placements) {
  using namespace darwin_art::loader;
  std::string error;
  const auto create = [&](const char* path) {
    return ProcessNamespaceInstallation::Create(path, "/system/bin/app_process64",
        false, false, "", providers, placements, &error);
  };
  assert(!AcquireProcessNamespaces());
  std::weak_ptr<NamespaceHandles> retired;
  {
    auto scoped = create("/linkerconfig/ld.config.txt");
    assert(scoped && error.empty());
    retired = scoped->namespaces();
    assert(AcquireProcessNamespaces() == scoped->namespaces());
    assert(!create("/linkerconfig/absent"));
    assert(!error.empty() && AcquireProcessNamespaces() == scoped->namespaces());
    assert(!create("/linkerconfig/ld.config.txt"));
    assert(error == "process namespace registry already installed");
    assert(AcquireProcessNamespaces() == scoped->namespaces());
    // A malformed candidate must fail privately, including when failure comes
    // after the first valid placement rather than from configuration parsing.
    std::vector<NativeProviderPlacement> invalid(placements.begin(), placements.end());
    assert(!invalid.empty());
    invalid.back().namespace_name = "missing-namespace";
    assert(!ProcessNamespaceInstallation::Create("/linkerconfig/ld.config.txt",
        "/system/bin/app_process64", false, false, "", providers, invalid, &error));
    assert(error == "native provider placement has no configured namespace");
    assert(AcquireProcessNamespaces() == scoped->namespaces());
    invalid.assign(placements.begin(), placements.end());
    invalid.front().backend = static_cast<NativeProviderPlacement::Backend>(-1);
    assert(!ProcessNamespaceInstallation::Create("/linkerconfig/ld.config.txt",
        "/system/bin/app_process64", false, false, "", providers, invalid, &error));
    assert(error == "unknown native provider backend");
    assert(AcquireProcessNamespaces() == scoped->namespaces());
  }
  assert(!AcquireProcessNamespaces() && retired.expired());
  auto next = create("/linkerconfig/ld.config.txt");
  assert(next && error.empty());
  auto borrowed = next->namespaces();
  retired = borrowed;
  assert(next->Shutdown() && next->Shutdown());
  assert(!AcquireProcessNamespaces() && !retired.expired());
  assert(borrowed->Exported("com_android_i18n"));
  borrowed.reset();
  assert(retired.expired());
  // Deliberate low-level misuse must not retire a newer installation. The old
  // wrapper still owns its registry even though it no longer owns the slot.
  auto stale = create("/linkerconfig/ld.config.txt");
  assert(stale && ShutdownProcessNamespaces(stale->namespaces()));
  auto replacement = create("/linkerconfig/ld.config.txt");
  assert(replacement);
  retired = stale->namespaces();
  assert(!stale->Shutdown() && !retired.expired());
  assert(AcquireProcessNamespaces() == replacement->namespaces());
  stale.reset();
  assert(retired.expired());
  assert(AcquireProcessNamespaces() == replacement->namespaces());
  assert(replacement->Shutdown());
  std::cout << "namespace installation: scope/explicit teardown, retained borrower, failed/duplicate isolation PASS\n";
}
