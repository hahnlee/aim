#include "registered_methods.h"
#include "loader/namespace_group_release.h"
#include "darwin_android_jni_trampoline.h"
#include <jni.h>
#include <cstdlib>
#include <exception>
#include <map>
#include <mutex>
#include <set>
#include <vector>

namespace darwin_art::jni {
struct RegisteredMethods::State {
  using Identity = std::pair<uint64_t, uint64_t>; // executable start, mapping group
  using Code = std::unique_ptr<android_jni::TrampolineSet,
      decltype(&android_jni::DestroyRegularTrampolines)>;
  std::shared_ptr<loader::NamespaceHandles> namespaces;
  std::shared_ptr<ProxyVm> vm;
  DarwinArtRegisteredNativeCache* cache = nullptr;
  std::mutex mutex;
  std::multimap<Identity, loader::ImageLease> leases;
  std::map<void*, Code> code;
  std::set<uint64_t> retired;

  static int Lookup(void* context, const void* function,
                    DarwinArtAndroidFunctionOwnerV1* output) noexcept {
    auto& self = *static_cast<State*>(context);
    try {
      LinkerImageLease* raw = nullptr;
      std::string error;
      const auto address = reinterpret_cast<uintptr_t>(function);
      const int status = self.namespaces->FindElfCaller(address, &raw, &error);
      loader::ImageLease image(raw);
      if (status != 0) return status == 1 ? 0 : -1;
      void* payload = nullptr;
      if (darwin_art_linker_image_typed_payload(image.get(), DARWIN_ART_IMAGE_ELF_SELECTED,
                                               &payload) != 0 || !payload) return -1;
      auto* selected = static_cast<DarwinArtElfSelectedImage*>(payload);
      uintptr_t begin = 0, end = 0;
      uint64_t group = 0;
      uint8_t root = 0;
      char detail[512]{};
      DarwinArtElfErrorBuffer buffer{detail, sizeof(detail), 0};
      if (darwin_art_elf_selected_executable_range(selected, address, &begin, &end,
                                                  &buffer) != DARWIN_ART_ELF_OK ||
          begin == 0 || end <= begin || address < begin || address >= end ||
          darwin_art_elf_selected_group_info(selected, &group, &root, &buffer) !=
              DARWIN_ART_ELF_OK || group == 0) return -1;
      // Group identities are loader-process monotonic; executable starts may
      // repeat after unmapping but never under the same mapping generation.
      std::lock_guard lock(self.mutex);
      if (self.retired.contains(group)) return -1;
      self.leases.emplace(Identity{begin, group}, std::move(image));
      *output = {begin, group, begin, end};
      return 1;
    } catch (...) { return -1; }
  }

  static void Release(void* context, const DarwinArtAndroidFunctionOwnerV1* owner) noexcept {
    auto& self = *static_cast<State*>(context);
    loader::ImageLease released;
    {
      std::lock_guard lock(self.mutex);
      auto found = self.leases.find({owner->image_id, owner->generation});
      if (found == self.leases.end()) std::abort();
      released = std::move(found->second);
      self.leases.erase(found);
    }
    // Native mapping release callbacks must not run with the registry locked.
  }

  static void* Build(void* context, const void* function,
      const DarwinArtAndroidFunctionOwnerV1*, const char* shorty, uint32_t,
      DarwinArtJniCallType kind) noexcept {
    auto& self = *static_cast<State*>(context);
    try {
      android_jni::TrampolineRequest request{const_cast<void*>(function), shorty, 1};
      Code code(nullptr, android_jni::DestroyRegularTrampolines);
      std::string error;
      if (kind == DARWIN_ART_JNI_CALL_CRITICAL_NATIVE) {
        code.reset(android_jni::CreateCriticalTrampolines(&request, 1, &error));
      } else if (kind == DARWIN_ART_JNI_CALL_REGULAR) {
        void* env = nullptr;
        auto* vm = static_cast<JavaVM*>(self.vm->JavaVm());
        if (vm->GetEnv(&env, JNI_VERSION_1_6) != JNI_OK || !env) return nullptr;
        code.reset(android_jni::CreateRegularTrampolines(env, &request, 1, &error));
      } else return nullptr;
      void* entry = android_jni::TrampolineEntry(code.get(), 0);
      if (!entry) return nullptr;
      std::lock_guard lock(self.mutex);
      self.code.emplace(entry, std::move(code));
      return entry;
    } catch (...) { return nullptr; }
  }

  static void Destroy(void* context, void* entry) noexcept {
    auto& self = *static_cast<State*>(context);
    Code released(nullptr, android_jni::DestroyRegularTrampolines);
    {
      std::lock_guard lock(self.mutex);
      auto found = self.code.find(entry);
      if (found == self.code.end()) std::abort();
      released = std::move(found->second);
      self.code.erase(found);
    }
  }
};

RegisteredMethods::RegisteredMethods() : state_(std::make_unique<State>()) {}
std::unique_ptr<RegisteredMethods> RegisteredMethods::Create(
    std::shared_ptr<loader::NamespaceHandles> namespaces, std::shared_ptr<ProxyVm> vm) try {
  if (!namespaces || !vm || !vm->JavaVm()) return nullptr;
  auto result = std::unique_ptr<RegisteredMethods>(new RegisteredMethods());
  auto& state = *result->state_;
  state.namespaces = std::move(namespaces);
  state.vm = std::move(vm);
  DarwinArtRegisteredNativeThunkFactoryV1 factory{
      DARWIN_ART_REGISTERED_NATIVE_BRIDGE_ABI_VERSION, sizeof(factory), &state,
      State::Lookup, State::Release, State::Build, State::Destroy};
  state.cache = darwin_art_registered_native_cache_create(&factory);
  if (!state.cache) return nullptr;
  return result;
} catch (...) { return nullptr; }

RegisteredMethods::~RegisteredMethods() {
  darwin_art_registered_native_cache_destroy(state_->cache);
}

DarwinArtRegisteredNativeResolution RegisteredMethods::Resolve(const void* function,
    bool bridged, const char* shorty, uint32_t length, DarwinArtJniCallType kind,
    const void** output) try {
  if (!output) return DARWIN_ART_REGISTERED_NATIVE_ERROR;
  *output = nullptr;
  // This registry recognizes only its own generated entries. Foreign/legacy
  // trampoline classification belongs to their owner, not a global mask guess.
  {
    std::lock_guard lock(state_->mutex);
    if (state_->code.contains(const_cast<void*>(function))) {
      *output = function;
      return DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE;
    }
  }
  return darwin_art_resolve_registered_native(
      state_->cache, function, bridged, shorty, length, kind, output);
} catch (...) {
  if (output) *output = nullptr;
  return DARWIN_ART_REGISTERED_NATIVE_ERROR;
}

void RegisteredMethods::RetireGroup(uint64_t group) try {
  std::set<uint64_t> images;
  {
    std::lock_guard lock(state_->mutex);
    state_->retired.insert(group);
    for (const auto& [identity, lease] : state_->leases) {
      if (identity.second == group) images.insert(identity.first);
    }
  }
  for (uint64_t image : images)
    darwin_art_registered_native_cache_retire_image(state_->cache, image, group);
} catch (...) {
  // No recoverable result channel: never authorize unmapping after a partial
  // retirement. No exception may escape into ART/native loader callbacks.
  std::terminate();
}
}
