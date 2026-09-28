#include "elf_graph_cache.h"

#include <algorithm>
#include <array>
#include <cstdio>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <sstream>
#include <utility>

namespace android {
namespace {

struct CachedGraph {
  AimElfGraphHandle* source;
  std::shared_ptr<AimElfGraphHandle> retained;
};

void ReleaseGraph(AimElfGraphHandle* graph) {
  std::array<char, 1024> storage{};
  AimElfErrorBuffer buffer{storage.data(), storage.size(), 0};
  (void)aim_elf_graph_unload(&graph, &buffer);
}

std::mutex& CachedElfMutex() {
  static auto* mutex = new std::mutex();
  return *mutex;
}

std::map<std::pair<uint64_t, std::string>, std::vector<CachedGraph>>& CachedElfGraphs() {
  // Android app native namespaces normally live for the ClassLoader/process
  // lifetime. Retained graph handles deliberately follow that lifetime so a
  // later dlopen can bind against the exact earlier DSO mapping and state.
  static auto* graphs =
      new std::map<std::pair<uint64_t, std::string>, std::vector<CachedGraph>>();
  return *graphs;
}

void SetCacheError(AimElfErrorBuffer* error, const std::string& message) {
  if (error == nullptr) return;
  error->required = message.size() + 1;
  if (error->data == nullptr || error->capacity == 0) return;
  const size_t copied = std::min(message.size(), error->capacity - 1);
  std::memcpy(error->data, message.data(), copied);
  error->data[copied] = '\0';
}

std::string ElfError(AimElfStatus status,
                     const AimElfErrorBuffer& error) {
  std::ostringstream stream;
  stream << aim_elf_status_name(status);
  if (error.data != nullptr && error.data[0] != '\0') {
    stream << ": " << error.data;
  }
  return stream.str();
}

}  // namespace

std::vector<std::string> SnapshotCachedElfSonames(uint64_t namespace_id) {
  std::lock_guard<std::mutex> lock(CachedElfMutex());
  std::vector<std::string> result;
  result.reserve(CachedElfGraphs().size());
  for (const auto& [soname, graph] : CachedElfGraphs()) {
    if (soname.first == namespace_id && !graph.empty()) result.push_back(soname.second);
  }
  std::sort(result.begin(), result.end());
  return result;
}

bool RegisterCachedElfGraph(uint64_t namespace_id, const char* root_soname,
                            AimElfGraphHandle* graph,
                            std::string* error) {
  if (root_soname == nullptr || root_soname[0] == '\0' || graph == nullptr) {
    if (error != nullptr) *error = "invalid ELF namespace cache registration";
    return false;
  }
  const auto key = std::make_pair(namespace_id, std::string(root_soname));
  const auto registered = [&] {
    if (const auto found = CachedElfGraphs().find(key); found != CachedElfGraphs().end()) {
      for (const auto& entry : found->second) {
        if (entry.source == graph) return true;
      }
    }
    return false;
  };
  {
    std::lock_guard<std::mutex> lock(CachedElfMutex());
    if (registered()) return true;
  }
  AimElfGraphHandle* retained = nullptr;
  std::array<char, 1024> storage{};
  AimElfErrorBuffer buffer{storage.data(), storage.size(), 0};
  const AimElfStatus status =
      aim_elf_graph_clone(graph, &retained, &buffer);
  if (status != AIM_ELF_OK || retained == nullptr) {
    if (error != nullptr) *error = ElfError(status, buffer);
    return false;
  }
  // Distinct absolute paths can contain the same SONAME. Keep their ownership
  // separate and preserve load order for dependency lookup.
  std::shared_ptr<AimElfGraphHandle> lease(retained, ReleaseGraph);
  {
    std::lock_guard<std::mutex> lock(CachedElfMutex());
    // Another caller may have published while we acquired the graph lease.
    if (registered()) return true;
    CachedElfGraphs()[key].push_back(CachedGraph{graph, lease});
  }
  std::fprintf(stderr, "DARWIN ELF namespace: retained root=%s\n", root_soname);
  return true;
}

void UnregisterCachedElfGraph(uint64_t namespace_id, const char* root_soname,
                              AimElfGraphHandle* source) {
  if (root_soname == nullptr || root_soname[0] == '\0') return;
  std::shared_ptr<AimElfGraphHandle> retained;
  {
    std::lock_guard<std::mutex> lock(CachedElfMutex());
    const auto it = CachedElfGraphs().find({namespace_id, root_soname});
    if (it == CachedElfGraphs().end()) return;
    auto& entries = it->second;
    const auto entry = std::find_if(entries.begin(), entries.end(),
                                    [source](const CachedGraph& value) {
                                      return value.source == source;
                                    });
    if (entry == entries.end()) return;
    retained = std::move(entry->retained);
    entries.erase(entry);
    if (entries.empty()) CachedElfGraphs().erase(it);
  }
  // Finalizers may re-enter native loading; never invoke them under the cache lock.
  retained.reset();
}

AimElfResolveStatus ResolveCachedElfProvider(
    uint64_t namespace_id,
    const AimElfSymbolRequest* request,
    uintptr_t* out_address,
    AimElfErrorBuffer* error) {
  if (request == nullptr || request->symbol == nullptr || out_address == nullptr ||
      (request->needed_library_count != 0 && request->needed_libraries == nullptr)) {
    return AIM_ELF_RESOLVE_ERROR;
  }
  auto lookup = [&](const char* soname) -> AimElfResolveStatus {
    std::shared_ptr<AimElfGraphHandle> retained;
    {
      std::lock_guard<std::mutex> lock(CachedElfMutex());
      const auto found = soname == nullptr ? CachedElfGraphs().end()
                                           : CachedElfGraphs().find({namespace_id, soname});
      if (found == CachedElfGraphs().end()) return AIM_ELF_RESOLVE_NOT_FOUND;
      retained = found->second.front().retained;
    }
    // A lookup owns a lease independent of cache membership. Symbol resolution
    // may re-enter the loader; neither it nor final release runs under our lock.
    std::array<char, 1024> storage{};
    AimElfErrorBuffer buffer{storage.data(), storage.size(), 0};
    const AimElfStatus status = aim_elf_graph_lookup_root_symbol(
        retained.get(), request->symbol, out_address, &buffer);
    if (status == AIM_ELF_OK) return AIM_ELF_RESOLVE_FOUND;
    if (status == AIM_ELF_SYMBOL_NOT_FOUND) {
      return AIM_ELF_RESOLVE_NOT_FOUND;
    }
    SetCacheError(error, ElfError(status, buffer));
    return AIM_ELF_RESOLVE_ERROR;
  };
  if (request->version_soname != nullptr) return lookup(request->version_soname);
  for (size_t index = 0; index < request->needed_library_count; ++index) {
    const AimElfResolveStatus status =
        lookup(request->needed_libraries[index]);
    if (status != AIM_ELF_RESOLVE_NOT_FOUND) return status;
  }
  return AIM_ELF_RESOLVE_NOT_FOUND;
}
}  // namespace android
