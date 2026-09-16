#include "darwin_art_bionic_builtin_adapters.h"
#include "darwin_art_bionic_process_state.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <set>

namespace {
using Find = const void* (*)(const char*);
using Read = void (*)(const void*, void (*)(void*, const char*, const char*, uint32_t), void*);
using Foreach = int (*)(void (*)(const void*, void*), void*);
struct Visit {
  Find find;
  Read read;
  const void* current = nullptr;
  std::set<const void*> tokens{};
  size_t reads = 0;
  bool sdk = false;
  bool failed = false;
};
void ReadValue(void* opaque, const char* name, const char* value, uint32_t) {
  auto& visit = *static_cast<Visit*>(opaque);
  ++visit.reads;
  if (!name || !value || visit.find(name) != visit.current) {
    visit.failed = true;
    return;
  }
  if (std::strcmp(name, "ro.build.version.sdk") == 0) visit.sdk = value[0] != '\0';
}
void VisitProperty(const void* property, void* opaque) {
  auto& visit = *static_cast<Visit*>(opaque);
  if (!property || !visit.tokens.insert(property).second) visit.failed = true;
  visit.current = property;
  // Reenter the same property owner through its public ABI, not a copied map.
  visit.read(property, ReadValue, &visit);
}
}

extern "C" int darwin_art_property_iteration_smoke(DarwinArtBionicNamespace* ns) {
  auto resolve = [ns](const char* name) {
    return darwin_art_bionic_namespace_resolve(ns, "libc.so", name, "LIBC");
  };
  const auto each = resolve("__system_property_foreach");
  const auto find = resolve("__system_property_find");
  const auto read = darwin_art_bionic_namespace_resolve(
      ns, "libc.so", "__system_property_read_callback", "LIBC_O");
  if (each.status != DARWIN_ART_BIONIC_NAMESPACE_OK || !each.address ||
      find.status != DARWIN_ART_BIONIC_NAMESPACE_OK || !find.address ||
      read.status != DARWIN_ART_BIONIC_NAMESPACE_OK || !read.address) return 1;
  if (darwin_art_bionic_process_state_process_install() != 0) return 2;
  Visit first{reinterpret_cast<Find>(find.address), reinterpret_cast<Read>(read.address)};
  Visit second{first.find, first.read};
  const auto iterate = reinterpret_cast<Foreach>(each.address);
  const int a = iterate(VisitProperty, &first);
  const int b = iterate(VisitProperty, &second);
  const int uninstall = darwin_art_bionic_process_state_process_uninstall();
  if (a || b || uninstall || first.failed || second.failed || !first.sdk ||
      !second.sdk || first.tokens.empty() || first.tokens != second.tokens ||
      first.reads != first.tokens.size() || second.reads != second.tokens.size()) return 3;
  std::fprintf(stderr, "property iteration: sealed ABI, stable tokens, reentrant reads PASS\n");
  return 0;
}
