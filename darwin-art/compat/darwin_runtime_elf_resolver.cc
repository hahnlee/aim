#include <dlfcn.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdlib>
#include <cstring>
#include <string>

#include "darwin_runtime_adapters_internal.h"
#include "darwin_android_asset_manager.h"
#include "darwin_android_system_fonts.h"
#include "darwin_android_platform.h"
#include "network/multinetwork.h"
#include "darwin_android_media_ndk.h"
#include "darwin_angle_egl.h"
#include "loader/bionic_symbol_lookup.h"
#if defined(DARWIN_ART_REAL_GRAPHICS)
#include "loader/graphics_ndk_symbols.h"
#endif

namespace android {
namespace {

// Android's libgen basename is a pure guest-path operation.  Keep it local to
// the runtime so a native APK never receives Darwin's host path semantics (or
// a pointer into a host-owned buffer) while resolving this legacy libc export.
char* AndroidBasename(const char* path) {
  static char empty_path[] = ".";
  if (path == nullptr || *path == '\0') return empty_path;
  const char* end = path + std::strlen(path);
  while (end > path && end[-1] == '/') --end;
  if (end == path) return const_cast<char*>(path);
  const char* begin = end;
  while (begin > path && begin[-1] != '/') --begin;
  return const_cast<char*>(begin);
}

void SetResolverError(DarwinArtElfErrorBuffer* error, const char* message) {
  if (error == nullptr || message == nullptr) return;
  const size_t required = std::strlen(message) + 1;
  error->required = required;
  if (error->data == nullptr || error->capacity == 0) return;
  const size_t copied = std::min(required - 1, error->capacity - 1);
  std::memcpy(error->data, message, copied);
  error->data[copied] = '\0';
}

bool NeedsLibrary(const DarwinArtElfSymbolRequest* request,
                  const char* soname) {
  for (size_t index = 0; index < request->needed_library_count; ++index) {
    if (request->needed_libraries[index] != nullptr &&
        std::strcmp(request->needed_libraries[index], soname) == 0) {
      return true;
    }
  }
  return false;
}

void* OpenAngleProvider(const char* filename,
                        void** slot,
                        DarwinArtElfErrorBuffer* error) {
  if (*slot != nullptr) return *slot;
  const char* directory = std::getenv("DARWIN_ART_ANGLE_DIRECTORY");
  if (directory == nullptr || directory[0] != '/') {
    SetResolverError(error,
                     "ANGLE provider requires absolute DARWIN_ART_ANGLE_DIRECTORY");
    return nullptr;
  }
  const std::string path = std::string(directory) + "/" + filename;
  *slot = dlopen(path.c_str(), RTLD_NOW | RTLD_LOCAL);
  if (*slot == nullptr) {
    const char* message = dlerror();
    SetResolverError(error, message == nullptr ? "ANGLE provider dlopen failed" : message);
  }
  return *slot;
}

DarwinArtElfResolveStatus ResolvePlatformProvider(
    ElfLibrary* library,
    const DarwinArtElfSymbolRequest* request,
    uintptr_t* out_address,
    DarwinArtElfErrorBuffer* error) {
  if (out_address) *out_address = 0;
#if defined(DARWIN_ART_REAL_GRAPHICS)
  // libjnigraphics imports belong to the original NDK graphics module, not
  // Darwin's global symbol namespace. Headless intentionally has no owner.
  const bool graphics_version = request->version_soname != nullptr &&
      request->version_name != nullptr &&
      std::strcmp(request->version_soname, "libjnigraphics.so") == 0 &&
      std::strcmp(request->version_name, "LIBJNIGRAPHICS") == 0;
  const bool graphics_unversioned = request->version_soname == nullptr &&
      request->version_name == nullptr;
  if (NeedsLibrary(request, "libjnigraphics.so") &&
      (graphics_version || graphics_unversioned)) {
    const uintptr_t address =
        darwin_art::loader::GraphicsNdkSymbol(request->symbol);
    if (address != 0) {
      *out_address = address;
      return DARWIN_ART_ELF_RESOLVE_FOUND;
    }
  }
#endif
  if (request->version_soname != nullptr || request->version_name != nullptr) {
    if (request->version_soname != nullptr && request->version_name != nullptr &&
        std::strcmp(request->version_soname, "libandroid.so") == 0) {
      void* network = darwin_art_android_multinetwork_symbol(
          request->symbol, request->version_name);
      if (network != nullptr) {
        *out_address = reinterpret_cast<uintptr_t>(network);
        return DARWIN_ART_ELF_RESOLVE_FOUND;
      }
    }
    return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
  }
  uintptr_t result = 0;
  size_t matches = 0;
  auto consider = [&](void* handle) {
    if (handle == nullptr) return;
    dlerror();
    void* symbol = dlsym(handle, request->symbol);
    if (symbol != nullptr && dlerror() == nullptr) {
      result = reinterpret_cast<uintptr_t>(symbol);
      ++matches;
    }
  };
  auto consider_address = [&](void* symbol) {
    if (symbol != nullptr) {
      result = reinterpret_cast<uintptr_t>(symbol);
      ++matches;
    }
  };
  if (NeedsLibrary(request, "libGLESv2.so") &&
      std::strncmp(request->symbol, "gl", 2) == 0) {
    const bool debug_angle = std::getenv("DARWIN_ART_DEBUG_ANGLE") != nullptr;
    if (debug_angle && std::strcmp(request->symbol, "glTexImage2D") == 0) {
      consider_address(reinterpret_cast<void*>(&darwin_art_android_glTexImage2D));
    } else if (debug_angle &&
               std::strcmp(request->symbol, "glTexSubImage2D") == 0) {
      consider_address(
          reinterpret_cast<void*>(&darwin_art_android_glTexSubImage2D));
    } else if (debug_angle &&
               std::strcmp(request->symbol, "glDrawArrays") == 0) {
      consider_address(reinterpret_cast<void*>(&darwin_art_android_glDrawArrays));
    } else if (debug_angle &&
               std::strcmp(request->symbol, "glDrawElements") == 0) {
      consider_address(
          reinterpret_cast<void*>(&darwin_art_android_glDrawElements));
    } else if (debug_angle &&
               std::strcmp(request->symbol, "glUseProgram") == 0) {
      consider_address(reinterpret_cast<void*>(&darwin_art_android_glUseProgram));
    } else {
      consider_address(
          darwin_art::darwin_art_angle_dso_symbol("libGLESv2.so",
                                                  request->symbol));
    }
  }
  if (NeedsLibrary(request, "libEGL.so") &&
      std::strncmp(request->symbol, "egl", 3) == 0) {
    consider_address(darwin_art::darwin_art_angle_dso_symbol(
        "libEGL.so", request->symbol));
  }
  const bool zlib_symbol = std::strncmp(request->symbol, "inflate", 7) == 0 ||
                           std::strncmp(request->symbol, "deflate", 7) == 0 ||
                           std::strcmp(request->symbol, "adler32") == 0 ||
                           std::strcmp(request->symbol, "crc32") == 0 ||
                           std::strcmp(request->symbol, "compress") == 0 ||
                           std::strcmp(request->symbol, "compress2") == 0 ||
                           std::strcmp(request->symbol, "uncompress") == 0 ||
                           std::strcmp(request->symbol, "zlibVersion") == 0;
  if (NeedsLibrary(request, "libz.so") && zlib_symbol) {
    if (library->z_provider == nullptr) {
      library->z_provider = dlopen("/usr/lib/libz.1.dylib", RTLD_NOW | RTLD_LOCAL);
    }
    consider(library->z_provider);
  }
  if (NeedsLibrary(request, "libandroid.so")) {
    consider_address(darwin_art_android_asset_manager_symbol(request->symbol));
    consider_address(darwin_art_android_platform_symbol(request->symbol));
    consider_address(darwin_art_android_system_font_symbol(request->symbol));
    consider_address(darwin_art_android_multinetwork_symbol(
        request->symbol, request->version_name));
  }
  if (NeedsLibrary(request, "libmediandk.so")) {
    consider_address(darwin_art_android_media_ndk_symbol(request->symbol));
  }
  if (matches == 1) {
    *out_address = result;
    return DARWIN_ART_ELF_RESOLVE_FOUND;
  }
  if (matches > 1) {
    SetResolverError(error, "platform import is exported by multiple providers");
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
}

}  // namespace

DarwinArtElfResolveStatus ResolveRuntimeProvider(
    void* context,
    const DarwinArtElfSymbolRequest* request,
    uintptr_t* out_address,
    DarwinArtElfErrorBuffer* error) {
  if (out_address) *out_address = 0;
  if (request == nullptr || out_address == nullptr ||
      request->abi_version != DARWIN_ART_ELF_ABI_VERSION ||
      request->symbol == nullptr) {
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  auto* library = static_cast<ElfLibrary*>(context);
  if (library == nullptr || library->provider_namespace == nullptr) {
    SetResolverError(error, "Bionic provider namespace is unavailable");
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  const DarwinArtElfResolveStatus cached =
      ResolveCachedElfProvider(library->loader_namespace_id, request, out_address, error);
  if (cached != DARWIN_ART_ELF_RESOLVE_NOT_FOUND) return cached;
  if (request->version_soname != nullptr && request->version_name != nullptr &&
      std::strcmp(request->version_soname, "libc.so") == 0 &&
      std::strcmp(request->version_name, "LIBC_R") == 0) {
    if (library->android_unwind_provider == nullptr) {
      SetResolverError(error, "Android LIBC_R provider is unavailable");
      return DARWIN_ART_ELF_RESOLVE_ERROR;
    }
    std::array<char, 512> lookup_storage{};
    DarwinArtElfErrorBuffer lookup_error{lookup_storage.data(),
                                          lookup_storage.size(), 0};
    const DarwinArtElfStatus status = darwin_art_elf_lookup(
        library->android_unwind_provider, request->symbol, out_address,
        &lookup_error);
    if (status == DARWIN_ART_ELF_OK) return DARWIN_ART_ELF_RESOLVE_FOUND;
    SetResolverError(error, status == DARWIN_ART_ELF_SYMBOL_NOT_FOUND
                                ? "Android LIBC_R symbol is unsupported"
                                : lookup_storage.data());
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  const DarwinArtElfResolveStatus platform =
      ResolvePlatformProvider(library, request, out_address, error);
  if (platform != DARWIN_ART_ELF_RESOLVE_NOT_FOUND) return platform;
  const char* provider_soname = request->version_soname;
  const char* provider_version = request->version_name;
  if ((provider_soname == nullptr) != (provider_version == nullptr)) {
    SetResolverError(error, "Bionic symbol version request is incomplete");
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  if (provider_soname != nullptr && provider_version != nullptr &&
      std::strcmp(provider_soname, "libc.so") == 0 &&
      std::strcmp(provider_version, "LIBC") == 0 &&
      std::strcmp(request->symbol, "basename") == 0) {
    *out_address = reinterpret_cast<uintptr_t>(&AndroidBasename);
    return DARWIN_ART_ELF_RESOLVE_FOUND;
  }
  if (provider_soname == nullptr) {
    const auto result = darwin_art::loader::LookupBionicDependencies(
        library->provider_namespace, request->needed_libraries,
        request->needed_library_count, request->symbol);
    if (result.status == DARWIN_ART_BIONIC_NAMESPACE_OK && result.address) {
      *out_address = result.address;
      return DARWIN_ART_ELF_RESOLVE_FOUND;
    }
    if (darwin_art::loader::IsBionicSymbolMiss(result.status))
      return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
    SetResolverError(error, darwin_art_bionic_namespace_status_name(result.status));
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  // Bionic exposes the large-file stdio aliases as LIBC_N, while the closed
  // stdio owner intentionally publishes the same 64-bit ABI under its
  // canonical LIBC names. Resolve the aliases to that exact owner rather than
  // falling through to a host libc symbol.
  const char* namespace_symbol = request->symbol;
  const char* namespace_version = provider_version;
  if (std::strcmp(provider_soname, "libc.so") == 0 &&
      std::strcmp(provider_version, "LIBC_N") == 0) {
    if (std::strcmp(request->symbol, "fseeko64") == 0) {
      namespace_symbol = "fseeko";
      namespace_version = "LIBC";
    } else if (std::strcmp(request->symbol, "ftello64") == 0) {
      namespace_symbol = "ftello";
      namespace_version = "LIBC";
    }
  }
  // Android's legacy utime entry point has the same path-plus-times pointer
  // calling convention as the facade's immutable utimes rejection.  Keep the
  // request inside the closed Bionic namespace instead of accidentally
  // resolving Darwin's host utime (whose time structure and path authority
  // are not Android-compatible).
  if (std::strcmp(provider_soname, "libc.so") == 0 &&
      std::strcmp(provider_version, "LIBC") == 0 &&
      std::strcmp(request->symbol, "utime") == 0) {
    namespace_symbol = "utimes";
  }
  const DarwinArtBionicNamespaceResult result =
      darwin_art_bionic_namespace_resolve(
          library->provider_namespace, provider_soname, namespace_symbol,
          namespace_version);
  if (result.status != DARWIN_ART_BIONIC_NAMESPACE_OK || result.address == 0) {
    if (darwin_art::loader::IsBionicSymbolMiss(result.status)) {
      return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
    }
    const std::string detail =
        std::string(darwin_art_bionic_namespace_status_name(result.status)) +
        " soname=" + (provider_soname == nullptr ? "<null>" : provider_soname) +
        " symbol=" + request->symbol +
        " version=" + (provider_version == nullptr ? "<null>" : provider_version);
    SetResolverError(error, detail.c_str());
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  }
  *out_address = result.address;
  return DARWIN_ART_ELF_RESOLVE_FOUND;
}

}  // namespace android
