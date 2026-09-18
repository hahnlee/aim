#include "darwin_runtime_adapters_internal.h"

#include <android/bitmap.h>

#include <cassert>
#include <cstdint>
#include <cstring>
#include <iostream>

#include "darwin_angle_egl.h"
#include "darwin_art_bionic_provider_namespace.h"
#include "darwin_art_elf_loader.h"

namespace {

// Test-only provider marker.  The resolver must not use any of these mocks to
// satisfy a libjnigraphics request; they only make the unrelated providers
// deterministic and isolate this test from the native runtime graph.
int g_unlock_calls = 0;

DarwinArtElfSymbolRequest Request(const char* symbol,
                                  const char* version_soname,
                                  const char* version_name,
                                  const char* const* needed,
                                  size_t needed_count) {
  DarwinArtElfSymbolRequest request{};
  request.abi_version = DARWIN_ART_ELF_ABI_VERSION;
  request.symbol = symbol;
  request.version_soname = version_soname;
  request.version_name = version_name;
  request.needed_libraries = needed;
  request.needed_library_count = needed_count;
  return request;
}

void Check(android::ElfLibrary* library, const DarwinArtElfSymbolRequest& request,
           DarwinArtElfResolveStatus expected, uintptr_t expected_address = 0) {
  uintptr_t address = UINTPTR_MAX;
  char error_storage[256]{};
  DarwinArtElfErrorBuffer error{error_storage, sizeof(error_storage), 0};
  const auto actual = android::ResolveRuntimeProvider(library, &request, &address, &error);
  if (actual != expected) {
    std::cerr << "unexpected status=" << actual << " expected=" << expected
              << " symbol=" << request.symbol << " soname="
              << (request.version_soname ? request.version_soname : "<null>")
              << " version=" << (request.version_name ? request.version_name : "<null>")
              << " address=" << address << " error=" << error_storage << "\n";
  }
  assert(actual == expected);
  if (expected == DARWIN_ART_ELF_RESOLVE_FOUND) {
    assert(address != 0);
    if (expected_address != 0) assert(address == expected_address);
  } else {
    assert(address == 0);
  }
}

}  // namespace

// This is the sole functional mock in the test: it gives the real graphics
// inventory a callable address so the positive route can invoke unlock.
extern "C" int AndroidBitmap_unlockPixels(JNIEnv*, jobject) {
  ++g_unlock_calls;
  return ANDROID_BITMAP_RESULT_SUCCESS;
}

// Required by the actual resolver TU. These providers intentionally miss.
void* darwin_art_android_asset_manager_symbol(const char*) { return nullptr; }
void* darwin_art_android_system_font_symbol(const char*) { return nullptr; }
void* darwin_art_android_media_ndk_symbol(const char*) { return nullptr; }
extern "C" void* darwin_art_android_multinetwork_symbol(const char*, const char*) {
  return nullptr;
}
extern "C" void* darwin_art_angle_dso_symbol(const char*, const char*) {
  return nullptr;
}
void darwin_art_android_glTexImage2D(uint32_t, int32_t, int32_t, int32_t, int32_t,
                                     int32_t, uint32_t, uint32_t, const void*) {}
void darwin_art_android_glTexSubImage2D(uint32_t, int32_t, int32_t, int32_t, int32_t,
                                        int32_t, uint32_t, uint32_t, const void*) {}
void darwin_art_android_glDrawArrays(uint32_t, int32_t, int32_t) {}
void darwin_art_android_glDrawElements(uint32_t, int32_t, uint32_t, const void*) {}
void darwin_art_android_glUseProgram(uint32_t) {}

namespace android {

DarwinArtElfResolveStatus ResolveCachedElfProvider(
    uint64_t, const DarwinArtElfSymbolRequest*, uintptr_t* out,
    DarwinArtElfErrorBuffer*) {
  *out = 0;
  return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
}

}  // namespace android

namespace darwin_art::loader {

bool IsBionicSymbolMiss(DarwinArtBionicNamespaceStatus status) {
  return status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME ||
         status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION ||
         status == DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL;
}

DarwinArtBionicNamespaceResult LookupBionicDependencies(
    DarwinArtBionicNamespace*, const char* const*, size_t, const char*) {
  return {DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
          DARWIN_ART_BIONIC_PROVIDER_LEAF, 0};
}

}  // namespace darwin_art::loader

extern "C" DarwinArtElfStatus darwin_art_elf_lookup(
    DarwinArtElfHandle*, const char*, uintptr_t*, DarwinArtElfErrorBuffer*) {
  return DARWIN_ART_ELF_SYMBOL_NOT_FOUND;
}

extern "C" DarwinArtBionicNamespaceResult darwin_art_bionic_namespace_resolve(
    DarwinArtBionicNamespace*, const char*, const char*, const char*) {
  return {DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
          DARWIN_ART_BIONIC_PROVIDER_LEAF, 0};
}

extern "C" const char* darwin_art_bionic_namespace_status_name(
    DarwinArtBionicNamespaceStatus) {
  return "test-only provider miss";
}

int main() {
  android::ElfLibrary library;
  // Opaque provider context: all namespace calls are test-only miss mocks.
  library.provider_namespace = reinterpret_cast<DarwinArtBionicNamespace*>(0x1);

  const char* graphics_needed[] = {"libjnigraphics.so"};
  const auto exact = Request("AndroidBitmap_unlockPixels", "libjnigraphics.so",
                             "LIBJNIGRAPHICS", graphics_needed, 1);
  const auto unversioned = Request("AndroidBitmap_unlockPixels", nullptr, nullptr,
                                   graphics_needed, 1);
  const auto unknown = Request("AndroidBitmap_notARealExport", "libjnigraphics.so",
                               "LIBJNIGRAPHICS", graphics_needed, 1);

#if defined(DARWIN_ART_REAL_GRAPHICS)
  Check(&library, exact, DARWIN_ART_ELF_RESOLVE_FOUND,
        reinterpret_cast<uintptr_t>(&AndroidBitmap_unlockPixels));
  using Unlock = int (*)(JNIEnv*, jobject);
  uintptr_t resolved_unlock = 0;
  assert(android::ResolveRuntimeProvider(&library, &exact, &resolved_unlock, nullptr) ==
         DARWIN_ART_ELF_RESOLVE_FOUND);
  assert(reinterpret_cast<Unlock>(resolved_unlock)(nullptr, nullptr) ==
         ANDROID_BITMAP_RESULT_SUCCESS);
  assert(g_unlock_calls == 1);
  Check(&library, unversioned, DARWIN_ART_ELF_RESOLVE_FOUND,
        reinterpret_cast<uintptr_t>(&AndroidBitmap_unlockPixels));
  Check(&library, unknown, DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
#else
  Check(&library, exact, DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  Check(&library, unversioned, DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  Check(&library, unknown, DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
#endif

  const char* missing_needed[] = {"libandroid.so"};
  Check(&library, Request("AndroidBitmap_unlockPixels", "libjnigraphics.so",
                          "LIBJNIGRAPHICS", missing_needed, 1),
        DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  Check(&library, Request("AndroidBitmap_unlockPixels", "libandroid.so",
                          "LIBJNIGRAPHICS", missing_needed, 1),
        DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  Check(&library, Request("AndroidBitmap_unlockPixels", "libjnigraphics.so",
                          "WRONG_VERSION", graphics_needed, 1),
        DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  Check(&library, Request("AndroidBitmap_unlockPixels", "wrong.so",
                          "LIBJNIGRAPHICS", graphics_needed, 1),
        DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  Check(&library, Request("AndroidBitmap_unlockPixels", "libjnigraphics.so", nullptr,
                          graphics_needed, 1), DARWIN_ART_ELF_RESOLVE_ERROR);
  Check(&library, Request("AndroidBitmap_unlockPixels", nullptr, "LIBJNIGRAPHICS",
                          graphics_needed, 1), DARWIN_ART_ELF_RESOLVE_ERROR);

  std::cout << "runtime ELF resolver graphics/headless routing PASS\n";
}
