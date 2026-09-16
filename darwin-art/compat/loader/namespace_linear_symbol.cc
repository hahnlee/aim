#include "namespace_handles.h"
#include "namespace_operation.h"
#include "namespace_group_release.h"
#include "namespace_symbol_lookup.h"

namespace darwin_art::loader {
int NamespaceHandles::LinearSymbol(android_namespace_t* ns, const LinkerImageLease* after,
    const char* symbol, uintptr_t* output, std::string* error, const char* version) {
  if (error) error->clear();
  if (output) *output = 0;
  auto fail = [error](const char* reason) { if (error) *error = reason; return -1; };
  if (!output || !symbol || !*symbol) return fail("invalid linear symbol query");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linear symbol operation");
  uint64_t id;
  { std::lock_guard lock(mutex_); id = Resolve(ns); }
  if (!id) return fail("unknown linear symbol namespace");
  LinkerImageGroup* raw = nullptr;
  if (darwin_art_linker_namespace_image_snapshot(configured_->registry(), id, &raw) != 0)
    return fail("cannot retain namespace symbol list");
  std::unique_ptr<LinkerImageGroup, decltype(&darwin_art_linker_group_destroy)> list(
      raw, darwin_art_linker_group_destroy);
  bool skipping = after != nullptr;
  for (size_t index = 0;; ++index) {
    LinkerImageLease* lease = nullptr;
    const int item = darwin_art_linker_group_image(raw, index, &lease);
    if (item == 1) break;
    if (item != 0) return fail("cannot retain linear candidate");
    ImageLease image(lease);
    if (skipping) {
      int32_t same = 0;
      if (darwin_art_linker_image_same(lease, after, &same) != 0)
        return fail("invalid NEXT caller identity");
      if (same) skipping = false;
      continue;
    }
    uint8_t eligible = 0;
    if (darwin_art_linker_image_linear_lookup_eligible(lease, &eligible) != 0)
      return fail("linear candidate SDK/flags unavailable");
    if (!eligible) continue;
    const int result = ResolveNamespaceImageSymbol(lease, symbol, output, error, version);
    if (result < 0) return -1;
    if (result == 0) return 0;
  }
  if (skipping) return fail("NEXT caller is outside namespace list");
  if (error) error->clear();
  return 1;
}
}
