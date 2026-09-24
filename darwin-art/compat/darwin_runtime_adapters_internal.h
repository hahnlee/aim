#pragma once

#include <array>
#include <atomic>
#include <cstdint>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

#include "darwin_android_jni_trampoline.h"
#include "darwin_android_elf_image_registry.h"
#include "darwin_art_elf_loader.h"
#include "darwin_art_bionic_dso_lifecycle.h"
#include "darwin_art_bionic_provider_namespace.h"
#include "darwin_art_jni_proxy.h"
#include "darwin_art_runtime_native_owner.h"
#include "loader/elf_graph_cache.h"

namespace android {

inline constexpr uint64_t kElfLibraryMagic = UINT64_C(0x44415257454c464a);
inline constexpr uint32_t kNativeOwnerFilesystem = 20;
inline constexpr uint32_t kNativeOwnerStdio = 30;
inline constexpr uint32_t kNativeOwnerIoctl = 40;
inline constexpr uint32_t kNativeOwnerSendfile = 50;
inline constexpr uint32_t kNativeOwnerStrftime = 60;
inline constexpr uint32_t kNativeOwnerNetwork = 70;
inline constexpr uint32_t kNativeOwnerVm = 75;
inline constexpr uint32_t kNativeOwnerDso = 80;
inline constexpr uint32_t kNativeOwnerImageRegistry = 90;
inline constexpr uint32_t kNativeOwnerNamespace = 100;
inline constexpr uint32_t kNativeOwnerAndroidUnwind = 105;
inline constexpr uint32_t kNativeOwnerGraph = 110;
inline constexpr uint32_t kNativeOwnerGraphHandle = 120;
struct ElfLibrary {
  uint64_t magic = kElfLibraryMagic;
  RuntimeNativeOwner* native_owner = nullptr;
  DarwinArtElfGraphHandle* graph = nullptr;
  DarwinArtElfHandle* android_unwind_provider = nullptr;
  DarwinArtBionicNamespace* provider_namespace = nullptr;
  DarwinArtBionicDsoLifecycleOwner* dso_lifecycle = nullptr;
  darwin_art_image_registry::Owner* image_registry = nullptr;
  void* egl_provider = nullptr;
  void* gles_provider = nullptr;
  void* z_provider = nullptr;
  uintptr_t jni_on_load = 0;
  uintptr_t jni_on_unload = 0;
  alignas(DARWIN_ART_JNI_PROXY_STORAGE_ALIGNMENT)
      std::array<unsigned char, DARWIN_ART_JNI_PROXY_STORAGE_SIZE> proxy_storage{};
  DarwinArtJniProxy* proxy = nullptr;
  JavaVM* art_vm = nullptr;
  // Legacy initiating-loader lease used by guest dlopen namespace routing.
  // JNI FindClass must instead use ART's native-call/ClassLoader context.
  void* app_loader = nullptr;
  // Stable NativeLoader namespace identity shared by every global reference
  // to the same Java ClassLoader. Detached native threads cannot call
  // IsSameObject, so cache reuse must not depend on jobject pointer identity.
  uint64_t loader_namespace_id = 0;
  // Exact resolved guest path and published graph handle. ART's resident
  // library cache is path + ClassLoader scoped; guest libdl must lease that
  // same owner rather than map a second copy with independent static state.
  std::string resolved_path;
  std::string cached_root_soname;
  void* graph_handle = nullptr;
  std::atomic<uint32_t> guest_open_refs{0};
  // bionic rtld_flags of this graph root. RTLD_GLOBAL is recorded only when a
  // guest dlopen creates the image (System.loadLibrary is RTLD_LOCAL) and
  // places it in the namespace's dlsym(RTLD_DEFAULT) scope. NODELETE may be
  // promoted by a later guest dlopen. Either flag makes dlclose retain it.
  std::atomic<bool> rtld_global{false};
  std::atomic<bool> rtld_nodelete{false};
  darwin_art::android_jni::TrampolineSet* trampolines = nullptr;
  // JNI_OnLoad may register methods on more than one app class.  Each
  // RegisterNatives call gets an independent executable trampoline mapping;
  // all mappings remain owned by the image until its graph is torn down.
  std::mutex trampoline_mutex;
  std::vector<darwin_art::android_jni::TrampolineSet*> trampoline_sets;
  std::unordered_map<std::string, void*> exported_jni_trampolines;
  std::mutex method_descriptor_mutex;
  std::unordered_map<void*, std::string> method_descriptors;
};

int PublishRuntimeElfImage(void* context, uintptr_t start, uintptr_t end);
int FinalizeRuntimeElfImage(void* context, uintptr_t start, uintptr_t end);
void TeardownProviderNamespace(ElfLibrary* library);
bool LookupOptionalElfSymbol(ElfLibrary* library,
                             const char* name,
                             uintptr_t* address,
                             std::string* error);
DarwinArtElfResolveStatus ResolveRuntimeProvider(
    void* context,
    const DarwinArtElfSymbolRequest* request,
    uintptr_t* out_address,
    DarwinArtElfErrorBuffer* error);
int DropRuntimeElfGraph(void* value, void* context);
int DropRuntimeAndroidUnwindProvider(void* value, void* context);
void DestroyRuntimeElfTrampolines(ElfLibrary* library);
int DropRuntimeElfLibrary(void* value, void* context);
int DropRuntimeElfImageRegistry(void* value, void* context);
int DropRuntimeDsoLifecycle(void* value, void* context);
int DropRuntimeProviderNamespace(void* value, void* context);
int DropRuntimeProviderKind(void* value, void* context);
ElfLibrary* AsElfLibrary(void* handle);
// Resolves a guest JNI function address to the runtime library whose published
// ELF image owns it.  The Android loader may share one process-wide libdl
// callback context across multiple ClassLoader DSOs, so JNI registration must
// derive ownership from the executable address rather than that callback's
// original context.
void RegisterElfLibrary(ElfLibrary* library);
void UnregisterElfLibrary(ElfLibrary* library);
// Unload all guest NativeLoader libraries while ART/JNI is still alive. This
// is the VM-shutdown equivalent of Android's JavaVMExt::UnloadNativeLibraries.
bool ShutdownElfLibraries();
ElfLibrary* FindElfLibraryForAddress(uintptr_t address);
ElfLibrary* FindElfLibraryByPath(JNIEnv* env, const char* path, jobject loader);
int32_t ProxyRegisterNatives(void* context,
                             void* clazz,
                             const DarwinArtJniNativeMethod* methods,
                             int32_t count);

void* ProxyCurrentEnv(void* context);
int32_t ProxyAttachCurrentThread(void* context, void* arguments,
                                 int32_t as_daemon);
int32_t ProxyDetachCurrentThread(void* context);
void* ProxyFindClass(void* context, const char* name);
int32_t ProxyThrowNew(void* context, void* clazz, const char* message);
void* ProxyGetMethodId(void* context, void* clazz, const char* name,
                       const char* signature, int32_t is_static);
uint64_t ProxyCallMethodV(void* context, void* object, void* method,
                          void* android_va_list, int32_t return_shorty,
                          int32_t is_static);

// Debug-only wrappers observe selected Unity lifecycle calls without changing
// the normal JNI registration path when the debug environment gate is absent.
void* MaybeWrapUnityLifecycleNative(const char* name, const char* signature,
                                    void* target);

}  // namespace android
