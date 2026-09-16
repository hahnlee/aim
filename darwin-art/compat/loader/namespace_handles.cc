#include "namespace_handles.h"
#include "namespace_operation.h"
#include <stdexcept>

namespace darwin_art::loader {
NamespaceHandles::NamespaceHandles(std::unique_ptr<ConfiguredNamespaces> configured)
    : configured_(std::move(configured)) {
  if (!configured_) throw std::invalid_argument("namespace handles require a configured registry");
  // Initialize privately without changing process-wide fdsan while an owner
  // is merely being staged. Activation must explicitly apply policy effects.
  const int configured_sdk = configured_->target_sdk_version();
  target_sdk_version_.store(configured_sdk == 0 ? __ANDROID_API__ : configured_sdk);
}

android_namespace_t* NamespaceHandles::Intern(uint64_t id) {
  auto found = tokens_.find(id);
  if (found != tokens_.end())
    return reinterpret_cast<android_namespace_t*>(found->second.get());
  auto token = std::make_unique<Token>(Token{id});
  auto* handle = reinterpret_cast<android_namespace_t*>(token.get());
  tokens_.emplace(id, std::move(token));
  return handle;
}

uint64_t NamespaceHandles::Resolve(android_namespace_t* handle) const {
  // Membership check only: never dereference an untrusted or foreign handle.
  for (const auto& [id, token] : tokens_) {
    if (reinterpret_cast<android_namespace_t*>(token.get()) == handle) return id;
  }
  return 0;
}

android_namespace_t* NamespaceHandles::Exported(const char* name) {
  if (!name) return nullptr;
  std::lock_guard lock(mutex_);
  uint64_t id = 0;
  if (darwin_art_linker_namespace_exported(configured_->registry(), name, &id) != 0)
    return nullptr;
  return Intern(id);
}

bool NamespaceHandles::Link(android_namespace_t* from, android_namespace_t* to,
                            const char* sonames, std::string* error) {
  if (error) error->clear();
  auto fail = [error](const char* detail) {
    if (error) *error = detail;
    return false;
  };
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker operation");
  std::lock_guard lock(mutex_);
  const auto source = Resolve(from);
  if (!source) return fail("invalid source namespace");
  // Original bionic link_namespaces: null target means default, null source
  // is an error. Hidden default is still a valid implicit target.
  const auto target = to ? Resolve(to) : configured_->Find("default");
  if (!target) return fail("invalid target namespace");
  if (!sonames || !*sonames) return fail("empty shared library list");
  if (darwin_art_linker_namespace_link(configured_->registry(), source, target, sonames) != 0)
    return fail("namespace link rejected");
  return true;
}
}  // namespace darwin_art::loader
