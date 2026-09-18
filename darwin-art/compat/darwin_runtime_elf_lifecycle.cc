#include <dlfcn.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>

#include "darwin_provider_owners.h"
#include "darwin_runtime_adapters_internal.h"
#include <cstdint>
#include "darwin_art_bionic_vm.h"

namespace android {
namespace {
extern "C" int32_t darwin_art_bionic_errno_load(void);


std::string ElfError(DarwinArtElfStatus status,
                     const DarwinArtElfErrorBuffer& error) {
  std::ostringstream stream;
  stream << darwin_art_elf_status_name(status);
  if (error.data != nullptr && error.data[0] != '\0') {
    stream << ": " << error.data;
  }
  return stream.str();
}

}  // namespace


void DestroyRuntimeElfTrampolines(ElfLibrary* library) {
  if (library == nullptr) return;
  std::lock_guard<std::mutex> lock(library->trampoline_mutex);
  for (auto* trampolines : library->trampoline_sets) {
    darwin_art::android_jni::DestroyRegularTrampolines(trampolines);
  }
  library->trampoline_sets.clear();
  library->exported_jni_trampolines.clear();
  library->trampolines = nullptr;
}

int PublishRuntimeElfImage(void* context, uintptr_t start, uintptr_t end) {
  ElfLibrary* library = static_cast<ElfLibrary*>(context);
  if (library == nullptr || library->dso_lifecycle == nullptr ||
      library->image_registry == nullptr) {
    std::fprintf(stderr, "DARWIN ELF loader: publish rejected missing owner range=[0x%llx,0x%llx)\n",
                 static_cast<unsigned long long>(start), static_cast<unsigned long long>(end));
    return -1;
  }
  if (darwin_art_image_registry::Publish(library->image_registry, start, end) != 0) {
    std::fprintf(stderr, "DARWIN ELF loader: publish rejected image registry range=[0x%llx,0x%llx)\n",
                 static_cast<unsigned long long>(start), static_cast<unsigned long long>(end));
    return -1;
  }
  if (darwin_art_bionic_vm_register_borrowed_range(
          reinterpret_cast<void*>(start), end - start) != 0) {
    std::fprintf(stderr, "DARWIN ELF loader: publish rejected borrowed-range registry errno=%d range=[0x%llx,0x%llx)\n",
                 static_cast<int>(darwin_art_bionic_errno_load()),
                 static_cast<unsigned long long>(start), static_cast<unsigned long long>(end));
    if (darwin_art_image_registry::RollbackPublish(library->image_registry,
                                                    start, end) != 0) {
      std::abort();
    }
    return -1;
  }
  if (darwin_art_bionic_dso_lifecycle_publish_image(
          library->dso_lifecycle, start, end) == 0) {
    std::fprintf(stderr, "DARWIN ELF loader: published image=[0x%llx,0x%llx) soname=%s path=%s\n",
                 static_cast<unsigned long long>(start),
                 static_cast<unsigned long long>(end),
                 library->cached_root_soname.empty() ? "<unknown>" : library->cached_root_soname.c_str(),
                 library->resolved_path.empty() ? "<unknown>" : library->resolved_path.c_str());
    return 0;
  }
  std::fprintf(stderr, "DARWIN ELF loader: publish rejected DSO lifecycle range=[0x%llx,0x%llx)\n",
               static_cast<unsigned long long>(start), static_cast<unsigned long long>(end));
  if (darwin_art_bionic_vm_unregister_borrowed_range(
          reinterpret_cast<void*>(start), end - start) != 0) {
    std::abort();
  }
  if (darwin_art_image_registry::RollbackPublish(library->image_registry, start,
                                                  end) != 0) {
    std::abort();
  }
  return -1;
}

int FinalizeRuntimeElfImage(void* context, uintptr_t start, uintptr_t end) {
  ElfLibrary* library = static_cast<ElfLibrary*>(context);
  if (library == nullptr || library->dso_lifecycle == nullptr ||
      library->image_registry == nullptr) {
    std::fprintf(stderr, "DARWIN ELF loader: finalize rejected missing lifecycle context\n");
    return -1;
  }
  if (darwin_art_bionic_dso_lifecycle_finalize_image(
          library->dso_lifecycle, start, end) != 0) {
    std::fprintf(stderr, "DARWIN ELF loader: finalize rejected DSO lifecycle range=[0x%llx,0x%llx)\n",
                 static_cast<unsigned long long>(start), static_cast<unsigned long long>(end));
    return -1;
  }
  if (darwin_art_bionic_vm_unregister_borrowed_range(
          reinterpret_cast<void*>(start), end - start) != 0) {
    std::fprintf(stderr, "DARWIN ELF loader: finalize rejected VM borrowed range\n");
    return -1;
  }
  const int result = darwin_art_image_registry::Finalize(library->image_registry, start,
                                                          end);
  if (result != 0) {
    std::fprintf(stderr, "DARWIN ELF loader: finalize rejected image registry\n");
  }
  return result;
}

int DropRuntimeElfGraph(void* value, void* context) {
  auto* graph = static_cast<DarwinArtElfGraphHandle*>(value);
  auto* library = static_cast<ElfLibrary*>(context);
  if (graph == nullptr) return -1;
  if (library != nullptr) {
    UnregisterCachedElfGraph(library->loader_namespace_id,
                            library->cached_root_soname.c_str(), graph);
  }
  std::array<char, 1024> storage{};
  DarwinArtElfErrorBuffer error{storage.data(), storage.size(), 0};
  const DarwinArtElfStatus status = darwin_art_elf_graph_unload(&graph, &error);
  if (status == DARWIN_ART_ELF_OK && library != nullptr) {
    library->graph = nullptr;
  }
  return status == DARWIN_ART_ELF_OK ? 0 : -1;
}

int DropRuntimeAndroidUnwindProvider(void* value, void* context) {
  auto* handle = static_cast<DarwinArtElfHandle*>(value);
  auto* library = static_cast<ElfLibrary*>(context);
  if (handle == nullptr) return -1;
  std::array<char, 1024> storage{};
  DarwinArtElfErrorBuffer error{storage.data(), storage.size(), 0};
  const DarwinArtElfStatus status = darwin_art_elf_unload(&handle, &error);
  if (status == DARWIN_ART_ELF_OK && library != nullptr) {
    library->android_unwind_provider = nullptr;
  }
  return status == DARWIN_ART_ELF_OK ? 0 : -1;
}

int DropRuntimeElfLibrary(void* value, void*) {
  auto* library = static_cast<ElfLibrary*>(value);
  if (library == nullptr || library->magic != kElfLibraryMagic) return -1;
  if (library->app_loader != nullptr) {
    if (JNIEnv* env = static_cast<JNIEnv*>(ProxyCurrentEnv(library))) {
      env->DeleteGlobalRef(static_cast<jobject>(library->app_loader));
    }
    library->app_loader = nullptr;
  }
  DestroyRuntimeElfTrampolines(library);
  TeardownProviderNamespace(library);
  for (void** handle : {&library->gles_provider, &library->egl_provider,
                        &library->z_provider}) {
    if (*handle != nullptr) {
      dlclose(*handle);
      *handle = nullptr;
    }
  }
  library->magic = 0;
  delete library;
  return 0;
}

int DropRuntimeElfImageRegistry(void* value, void*) {
  if (value == nullptr) return -1;
  darwin_art_image_registry::Destroy(
      static_cast<darwin_art_image_registry::Owner*>(value));
  return 0;
}

int DropRuntimeDsoLifecycle(void* value, void*) {
  if (value == nullptr) return -1;
  darwin_art_bionic_dso_lifecycle_owner_destroy(
      static_cast<DarwinArtBionicDsoLifecycleOwner*>(value));
  return 0;
}

int DropRuntimeProviderNamespace(void* value, void*) {
  if (value == nullptr) return -1;
  const auto status = darwin_art_bionic_namespace_teardown(
      static_cast<DarwinArtBionicNamespace*>(value));
  if (status != DARWIN_ART_BIONIC_NAMESPACE_OK) return -1;
  darwin_art_bionic_namespace_destroy(
      static_cast<DarwinArtBionicNamespace*>(value));
  return 0;
}

int DropRuntimeProviderKind(void* value, void*) {
  const auto kind = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(value));
  switch (kind) {
    case static_cast<uint32_t>(darwin_art::providers::Kind::Filesystem):
      darwin_art::providers::release_filesystem();
      return 0;
    case static_cast<uint32_t>(darwin_art::providers::Kind::Network):
      darwin_art::providers::release_network();
      return 0;
    case static_cast<uint32_t>(darwin_art::providers::Kind::Stdio):
      darwin_art::providers::release_stdio();
      return 0;
    case static_cast<uint32_t>(darwin_art::providers::Kind::Ioctl):
      darwin_art::providers::release_ioctl();
      return 0;
    case static_cast<uint32_t>(darwin_art::providers::Kind::Strftime):
      darwin_art::providers::release_strftime();
      return 0;
    case static_cast<uint32_t>(darwin_art::providers::Kind::Sendfile):
      darwin_art::providers::release_sendfile();
      return 0;
    case static_cast<uint32_t>(darwin_art::providers::Kind::Vm):
      darwin_art::providers::release_vm();
      return 0;
    default:
      return -1;
  }
}

void TeardownProviderNamespace(ElfLibrary* library) {
  if (library == nullptr) return;
  UnregisterElfLibrary(library);
  if (library->native_owner != nullptr) {
    const int status = darwin_art_runtime_native_owner_destroy(library->native_owner);
    library->native_owner = nullptr;
    library->graph = nullptr;
    library->android_unwind_provider = nullptr;
    library->provider_namespace = nullptr;
    library->image_registry = nullptr;
    library->dso_lifecycle = nullptr;
    if (status != 0) std::abort();
    return;
  }
  // Provider installation is only reachable after the Rust native-owner slot
  // has been created. Reaching this branch with any graph resource would mean
  // that a C++ fallback has escaped the Rust ownership boundary.
  if (library->graph != nullptr || library->android_unwind_provider != nullptr ||
      library->provider_namespace != nullptr ||
      library->image_registry != nullptr || library->dso_lifecycle != nullptr) {
    std::abort();
  }
}

bool LookupOptionalElfSymbol(ElfLibrary* library,
                             const char* name,
                             uintptr_t* address,
                             std::string* error) {
  if (library == nullptr || library->graph == nullptr || name == nullptr ||
      address == nullptr || error == nullptr) {
    return false;
  }
  std::array<char, 1024> storage{};
  DarwinArtElfErrorBuffer error_buffer{storage.data(), storage.size(), 0};
  const DarwinArtElfStatus status =
      darwin_art_elf_graph_lookup_root(library->graph, name, address, &error_buffer);
  if (status == DARWIN_ART_ELF_OK) return true;
  if (status == DARWIN_ART_ELF_SYMBOL_NOT_FOUND) {
    *address = 0;
    return true;
  }
  *error = ElfError(status, error_buffer);
  return false;
}

}  // namespace android
