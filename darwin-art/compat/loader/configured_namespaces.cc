#include "configured_namespaces.h"
#include "linker_config.h"

namespace darwin_art::loader {
namespace {
std::string Join(const std::vector<std::string>& paths) {
  std::string result;
  for (const auto& path : paths) {
    if (!result.empty()) result += ':';
    result += path;
  }
  return result;
}
}  // namespace

uint64_t ConfiguredNamespaces::Find(const std::string& name) const {
  auto found = ids_.find(name);
  return found == ids_.end() ? 0 : found->second;
}

std::unique_ptr<ConfiguredNamespaces> ConfiguredNamespaces::Create(
    const std::vector<std::unique_ptr<NamespaceConfig>>& configs,
    const std::string& ld_paths, std::string* error) {
  auto fail = [error](const std::string& message) -> std::unique_ptr<ConfiguredNamespaces> {
    if (error) *error = message;
    return nullptr;
  };
  auto owner = std::unique_ptr<ConfiguredNamespaces>(new ConfiguredNamespaces);
  owner->registry_.reset(darwin_art_linker_registry_create());
  if (!owner->registry_) return fail("namespace registry allocation failed");
  for (const auto& config : configs) {
    if (!config || !*config->name() || owner->Find(config->name()))
      return fail("null, empty or duplicate namespace configuration");
    const bool is_default = std::string(config->name()) == "default";
    const auto search = Join(config->search_paths());
    const auto permitted = Join(config->permitted_paths());
    // Original bionic init_default_namespaces applies allowed_libs only to
    // additional namespaces; default retains its startup LD_LIBRARY_PATH.
    const auto allowed = is_default ? std::string() : Join(config->allowed_libs());
    uint64_t id = 0;
    const auto status = darwin_art_linker_namespace_create_configured(owner->registry(),
        config->isolated(), is_default ? ld_paths.c_str() : "", search.c_str(),
        permitted.c_str(), allowed.c_str(), 0, &id);
    if (status != 0) return fail("invalid namespace configuration: " + std::string(config->name()));
    owner->ids_.emplace(config->name(), id);
    if (config->visible() && darwin_art_linker_namespace_export(owner->registry(), id, config->name()) != 0)
      return fail("namespace export failed: " + std::string(config->name()));
  }
  if (!owner->Find("default")) return fail("missing default namespace");
  for (const auto& config : configs) {
    for (const auto& link : config->links()) {
      const uint64_t from = owner->Find(config->name()), to = owner->Find(link.ns_name());
      if (!to) return fail("unknown namespace link target: " + link.ns_name());
      const auto status = link.allow_all_shared_libs()
          ? darwin_art_linker_namespace_link_all(owner->registry(), from, to)
          : darwin_art_linker_namespace_link(owner->registry(), from, to, link.shared_libs().c_str());
      if (status != 0) return fail("invalid namespace link: " + link.ns_name());
    }
  }
  if (error) error->clear();
  return owner;
}
}  // namespace darwin_art::loader
