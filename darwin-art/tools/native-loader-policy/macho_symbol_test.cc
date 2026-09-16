#include "linker_config.h"
#include "loader/configured_namespaces.h"
#include "loader/namespace_handles.h"
#include <cassert>
#include <cstdio>
#include <dlfcn.h>

namespace {
constexpr const char* kSystemImage = "/usr/lib/libSystem.B.dylib";
int retained = 0;
int released = 0;
void* Retain(void*) {
  void* handle = dlopen(kSystemImage, RTLD_NOW | RTLD_LOCAL);
  if (handle) ++retained;
  return handle;
}
void Release(void* handle) {
  assert(dlclose(handle) == 0);
  ++released;
}
}

void TestTypedMachOSymbols() {
  using namespace darwin_art::loader;
  std::vector<std::unique_ptr<NamespaceConfig>> configs;
  auto config = std::make_unique<NamespaceConfig>("default");
  config->set_visible(true);
  configs.push_back(std::move(config));
  std::string error;
  auto configured = ConfiguredNamespaces::Create(configs, "", &error);
  assert(configured && error.empty());
  auto* registry = configured->registry();
  void* original = dlopen(kSystemImage, RTLD_NOW | RTLD_LOCAL);
  assert(original);
  void* expected = dlsym(original, "strlen");
  assert(expected);
  assert(darwin_art_linker_namespace_publish_image(registry, configured->Find("default"),
      "libtyped-test.so", "/system/lib64/libtyped-test.so", original, Retain, Release,
      0, 0, DARWIN_ART_IMAGE_MACHO) == 0);
  assert(retained == 1 && released == 0);
  assert(dlclose(original) == 0);  // Only the published owner now retains it.
  {
    NamespaceHandles handles(std::move(configured));
    LinkerImageLease* image = nullptr;
    assert(handles.FindResident(handles.Exported("default"), "libtyped-test.so", &image) == 0);
    LinkerImageLease* expected_image = darwin_art_linker_image_clone(image);
    assert(expected_image);
    uintptr_t token = 0;
    assert(darwin_art_linker_handle_adopt(registry, &image, &token) == 0 && !image && token);
    assert(token != reinterpret_cast<uintptr_t>(original));
    LinkerImageLease* defining_image = nullptr;
    auto address = handles.LibrarySymbol(token, "strlen", &error, nullptr, &defining_image);
    assert(address == reinterpret_cast<uintptr_t>(expected) && error.empty());
    int32_t same = 0;
    assert(defining_image);
    assert(darwin_art_linker_image_same(defining_image, expected_image, &same) == 0 && same);
    darwin_art_linker_image_release(defining_image);
    assert(reinterpret_cast<size_t(*)(const char*)>(address)("native-owner") == 12);
    assert(!handles.LibrarySymbol(token, "darwin_art_absent_macho_test_symbol", &error));
    assert(!error.empty());
    // Misses clear a pre-seeded output without consuming the caller's old
    // lease. The caller remains responsible for releasing that old clone.
    LinkerImageLease* miss_sentinel = darwin_art_linker_image_clone(expected_image);
    assert(miss_sentinel);
    LinkerImageLease* old_miss_sentinel = miss_sentinel;
    assert(!handles.LibrarySymbol(token, "darwin_art_absent_macho_test_symbol_2", &error,
        nullptr, &miss_sentinel));
    assert(!miss_sentinel);
    darwin_art_linker_image_release(old_miss_sentinel);
    // A dyld handle is not an Android registry token: no host fallback.
    LinkerImageLease* invalid_sentinel = darwin_art_linker_image_clone(expected_image);
    assert(invalid_sentinel);
    LinkerImageLease* old_invalid_sentinel = invalid_sentinel;
    assert(!handles.LibrarySymbol(reinterpret_cast<uintptr_t>(original), "strlen", &error,
        nullptr, &invalid_sentinel));
    assert(!invalid_sentinel);
    darwin_art_linker_image_release(old_invalid_sentinel);
    assert(!error.empty());
    darwin_art_linker_image_release(expected_image);
    assert(handles.CloseLibrary(token, &error) == 0);
    assert(handles.CloseLibrary(token, &error) == -1);
    assert(released == 0);  // Resident publication survives logical dlclose.
  }
  assert(released == retained);
  std::puts("typed Mach-O symbol: real dyld payload, token rejection, owned release PASS");
}
