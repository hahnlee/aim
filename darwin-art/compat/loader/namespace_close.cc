#include "namespace_close.h"
#include "namespace_group_release.h"
#include "namespace_operation.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include "vulkan_image.h"
#include <cstdlib>
#include <deque>
#include <set>
namespace darwin_art::loader {
namespace {
void Restore(LinkerRegistry* registry, uintptr_t handle, LinkerImageLease* opened) {
  uintptr_t restored = 0;
  if (darwin_art_linker_handle_adopt(registry, &opened, &restored) != 0 ||
      restored != handle) std::abort();
}
}
int CloseNamespaceLibrary(LinkerRegistry* registry, uintptr_t handle, std::string* error) try {
  if (error) error->clear();
  auto fail = [error](const char* reason) { if (error) *error = reason; return -1; };
  NamespaceOperation operation(registry);
  if (!operation) return fail("cannot enter linker close operation");
  LinkerImageLease* opened = nullptr;
  if (darwin_art_linker_handle_take_open(registry, handle, &opened) != 0)
    return fail("invalid or already closed Android library handle");
  void* payload = nullptr;
  if (darwin_art_linker_image_typed_payload(opened, DARWIN_ART_IMAGE_BIONIC_PROVIDER, &payload) == 0 ||
      darwin_art_linker_image_typed_payload(opened, DARWIN_ART_IMAGE_MACHO, &payload) == 0 ||
      darwin_art_linker_image_typed_payload(opened, DARWIN_ART_IMAGE_ANGLE, &payload) == 0 ||
      darwin_art_linker_image_typed_payload(opened, DARWIN_ART_IMAGE_NATIVE_WINDOW, &payload) == 0 ||
      IsVulkanImage(opened) ||
      (IsGraphicsNdkImage(opened) || IsLinkerImage(opened))) {
    // These are configured resident native images. Close the acquired resource
    // lease; do not invent ELF finalization or unpublish process providers.
    darwin_art_linker_image_release(opened);
    return 0;
  }
  LinkerImageLease* raw = nullptr;
  if (darwin_art_linker_image_consume_open(&opened, &raw) != 0) {
    Restore(registry, handle, opened);
    return fail("cannot consume Android library open");
  }
  ImageLease source(raw);
  std::vector<ImageLease> targets;
  const int status = ReleaseNamespaceGroup(registry, source, &targets, error);
  if (status < 0) {
    // ReleaseNamespaceGroup has not committed any destructor on this path.
    if (darwin_art_linker_image_acquire_open(source.get(), &opened) != 0) std::abort();
    Restore(registry, handle, opened);
    return -1;
  }
  if (status == 1) return 0;
  // Parent resources are released before checking original external targets.
  // Revisit retained targets when another incoming edge later disappears.
  std::deque<ImageLease> pending;
  for (auto& target : targets) pending.push_back(std::move(target));
  std::set<uint64_t> retired;
  while (!pending.empty()) {
    ImageLease target = std::move(pending.front());
    pending.pop_front();
    DarwinArtLocalGroupMetadata group{};
    if (darwin_art_linker_image_group_metadata(target.get(), &group) != 0) std::abort();
    if (retired.count(group.id)) continue;
    std::vector<ImageLease> next;
    const int result = ReleaseNamespaceGroup(registry, target, &next, error);
    // The original close already committed: never claim rollback is possible.
    if (result < 0) std::abort();
    if (result == 0) {
      retired.insert(group.id);
      for (auto it = next.rbegin(); it != next.rend(); ++it)
        pending.push_front(std::move(*it));
    }
  }
  return 0;
} catch (...) {
  // Allocation failure must not unwind through libdl or lose a consumed open.
  std::abort();
}
}
