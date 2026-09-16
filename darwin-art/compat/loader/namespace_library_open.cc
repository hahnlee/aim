#include "namespace_handles.h"
#include "namespace_operation.h"
#include "android_dlext_types.h"
#include <cstring>

namespace darwin_art::loader {
uintptr_t NamespaceHandles::OpenLibrary(uintptr_t address, const char* path, int flags,
    uint64_t extensions, android_namespace_t* target, std::string* error) {
  if (error) error->clear();
  auto fail = [error](const char* text) -> uintptr_t { if (error) *error = text; return 0; };
  if (!path || !*path) return fail("main executable handle is not yet represented by this linker");
  const bool nodelete = (flags & 0x1000) != 0;
  const int binding = flags & ~0x1000;
  if (binding != 1 && binding != 2) return fail("requested Android load flags require unfinished linker policy");
  if (extensions & ~uint64_t{ANDROID_DLEXT_USE_NAMESPACE})
    return fail("requested Android extended loading mode is not implemented");
  if ((extensions & ANDROID_DLEXT_USE_NAMESPACE) && !target)
    return fail("ANDROID_DLEXT_USE_NAMESPACE requires a namespace");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker operation");
  LinkerImageLease* raw_caller = nullptr;
  if (!(extensions & ANDROID_DLEXT_USE_NAMESPACE)) {
    target = nullptr;
    if (FindElfCaller(address, &raw_caller, error) < 0) return 0;
  }
  std::unique_ptr<LinkerImageLease, decltype(&darwin_art_linker_image_release)> caller(
      raw_caller, darwin_art_linker_image_release);
  target = ParentForCaller(target, caller.get(), error);
  if (!target) return 0;
  LinkerImageLease* opened = nullptr;
  // These operations reuse resident identity or admit/map a new file with the
  // default owned Bionic lifecycle, and return one counted open in either case.
  const int status = std::strchr(path, '/')
      ? OpenPath(target, path, nullptr, &opened, error, nodelete)
      : OpenLocalSoname(target, path, nullptr, &opened, error, nodelete);
  if (status != 0) {
    darwin_art_linker_image_release(opened);
    if (error && error->empty()) *error = "library not found in selected namespace";
    return 0;
  }
  uintptr_t handle = 0;
  const int adopted = darwin_art_linker_handle_adopt(configured_->registry(), &opened, &handle);
  darwin_art_linker_image_release(opened);
  if (adopted != 0) return fail("cannot adopt Android library open");
  return handle;
}
int NamespaceHandles::LibraryImage(uintptr_t handle, LinkerImageLease** output) {
  NamespaceOperation operation(configured_->registry());
  if (!operation) { if (output) *output = nullptr; return -1; }
  return darwin_art_linker_handle_image(configured_->registry(), handle, output);
}
}
