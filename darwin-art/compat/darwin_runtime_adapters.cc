#include <dlfcn.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <mutex>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include "jni/jni_env_ext.h"
#include "hprof/hprof.h"
#include "darwin_provider_owners.h"
#include "darwin_jni_shorty.h"
#include "darwin_runtime_adapters_internal.h"
#include "darwin_art_bionic_builtin_adapters.h"
#include "darwin_art_bionic_dns.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_ioctl.h"
#include "darwin_art_bionic_sendfile.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_art_bionic_stdio.h"
#include "darwin_art_bionic_strftime.h"
#include "nativebridge/native_bridge.h"
#include "nativeloader/native_loader.h"
#include "odr_statslog/odr_statslog.h"

extern "C" DarwinArtBionicSendfileTransferStatus
darwin_art_bionic_fs_sendfile_transfer(
    void* context, const DarwinArtBionicSendfileRequest* request,
    DarwinArtBionicSendfileResult* result);

namespace android {


namespace {

thread_local ElfLibrary* g_pending_on_load = nullptr;
thread_local ElfLibrary* g_pending_on_unload = nullptr;

jint ElfJniOnLoadTrampoline(JavaVM*, void*) {
  ElfLibrary* library = std::exchange(g_pending_on_load, nullptr);
  if (library == nullptr || library->jni_on_load == 0 || library->proxy == nullptr) {
    return JNI_ERR;
  }
  using AndroidJniOnLoad = jint (*)(JavaVM*, void*);
  auto function = reinterpret_cast<AndroidJniOnLoad>(library->jni_on_load);
  // JNI_OnLoad itself has only two register arguments, so its Android and
  // Darwin AArch64 calling sequences coincide. The ELF library sees only the
  // closed proxy VM and never ART's real JavaVM function table.
  const jint result =
      function(static_cast<JavaVM*>(darwin_art_jni_proxy_java_vm(library->proxy)), nullptr);
  return result;
}

void ElfJniOnUnloadTrampoline(JavaVM*, void*) {
  ElfLibrary* library = std::exchange(g_pending_on_unload, nullptr);
  if (library == nullptr || library->jni_on_unload == 0 || library->proxy == nullptr) {
    return;
  }
  using AndroidJniOnUnload = void (*)(JavaVM*, void*);
  auto function = reinterpret_cast<AndroidJniOnUnload>(library->jni_on_unload);
  function(static_cast<JavaVM*>(darwin_art_jni_proxy_java_vm(library->proxy)), nullptr);
}

}  // namespace

extern "C" {

void* DarwinNativeBridgeGetTrampoline2(void* handle,
                                       const char* name,
                                       const char* shorty,
                                       uint32_t,
                                       JNICallType call_type) {
  ElfLibrary* library = AsElfLibrary(handle);
  const bool debug_native_bridge =
      std::getenv("DARWIN_ART_DEBUG_NATIVE_BRIDGE") != nullptr;
  if (library == nullptr || name == nullptr) {
    return nullptr;
  }
  if (std::strcmp(name, "JNI_OnLoad") == 0 && library->jni_on_load != 0) {
    if (g_pending_on_load != nullptr) {
      return nullptr;
    }
    g_pending_on_load = library;
    return reinterpret_cast<void*>(&ElfJniOnLoadTrampoline);
  }
  if (std::strcmp(name, "JNI_OnUnload") == 0 && library->jni_on_unload != 0) {
    if (g_pending_on_unload != nullptr) {
      return nullptr;
    }
    g_pending_on_unload = library;
    return reinterpret_cast<void*>(&ElfJniOnUnloadTrampoline);
  }
  // CriticalNative omits the regular JNI environment/class prefix and needs a
  // distinct PCS bridge. Never expose a raw Android function while that ABI is
  // unsupported.
  if (call_type != kJNICallTypeRegular || shorty == nullptr || shorty[0] == '\0') {
    return nullptr;
  }

  const std::string cache_key = std::string(name) + '\n' + shorty;
  std::lock_guard<std::mutex> lock(library->trampoline_mutex);
  if (auto cached = library->exported_jni_trampolines.find(cache_key);
      cached != library->exported_jni_trampolines.end()) {
    if (debug_native_bridge) {
      std::fprintf(stderr,
                   "DARWIN NativeBridge JNI cached name=%s shorty=%s entry=%p\n",
                   name, shorty, cached->second);
    }
    return cached->second;
  }

  uintptr_t target = 0;
  std::string lookup_error;
  if (!LookupOptionalElfSymbol(library, name, &target, &lookup_error) || target == 0) {
    if (debug_native_bridge) {
      std::fprintf(stderr,
                   "DARWIN NativeBridge JNI missing name=%s shorty=%s error=%s\n",
                   name, shorty, lookup_error.c_str());
    }
    return nullptr;
  }
  JavaVM* proxy_vm =
      static_cast<JavaVM*>(darwin_art_jni_proxy_java_vm(library->proxy));
  void* proxy_env = nullptr;
  if (proxy_vm == nullptr ||
      proxy_vm->GetEnv(&proxy_env, JNI_VERSION_1_6) != JNI_OK ||
      proxy_env == nullptr) {
    return nullptr;
  }
  darwin_art::android_jni::TrampolineRequest request{
      reinterpret_cast<void*>(target), shorty, 1u};
  std::string trampoline_error;
  auto* trampolines = darwin_art::android_jni::CreateRegularTrampolines(
      proxy_env, &request, 1, &trampoline_error);
  void* entry = darwin_art::android_jni::TrampolineEntry(trampolines, 0);
  if (trampolines == nullptr || entry == nullptr) {
    darwin_art::android_jni::DestroyRegularTrampolines(trampolines);
    return nullptr;
  }
  if (library->trampolines == nullptr) {
    library->trampolines = trampolines;
  }
  library->trampoline_sets.push_back(trampolines);
  library->exported_jni_trampolines.emplace(cache_key, entry);
  if (debug_native_bridge) {
    std::fprintf(stderr,
                 "DARWIN NativeBridge JNI mapped name=%s shorty=%s target=%p entry=%p\n",
                 name, shorty, reinterpret_cast<void*>(target), entry);
  }
  return entry;
}

bool DarwinNativeBridgeIsNativeBridgeFunctionPointer(const void* pointer) {
  return darwin_art::android_jni::TrampolineEntryMask(pointer) != 0;
}

}  // extern "C"
}  // namespace android
