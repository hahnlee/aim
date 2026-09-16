#pragma once
#include <memory>
namespace darwin_art::loader {
class NamespaceHandles;
// Install a fully constructed owner into an empty process slot. No partial
// registry publication or replacement while namespace handles can escape.
bool InstallProcessNamespaces(std::shared_ptr<NamespaceHandles>);
std::shared_ptr<NamespaceHandles> AcquireProcessNamespaces();
// Process shutdown only, NOT NativeLoader Reset. Caller drains application
// threads and invalidates all borrowed namespace handles before this call.
// Expected owner prevents accidentally removing a different installation.
bool ShutdownProcessNamespaces(const std::shared_ptr<NamespaceHandles>& expected);
}
