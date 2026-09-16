#include <cstdlib>
#include <cstdio>
#include <dlfcn.h>
#include <string>
#include <sys/stat.h>

#include "jni.h"
#include "loader/classloader_namespaces.h"
#include "nativebridge/native_bridge.h"
#include "nativeloader/native_loader.h"

// Keep libnativebridge's process state machine source-identical to the pinned
// Android 16 implementation. Darwin's Android-ELF graph remains a lower-level
// trampoline backend; it does not replace NativeBridge lifecycle semantics.
#ifndef ABI_STRING
#define ABI_STRING "arm64"
#endif
#define NativeBridgeGetTrampoline2 AospNativeBridgeGetTrampoline2
#define NativeBridgeIsNativeBridgeFunctionPointer \
  AospNativeBridgeIsNativeBridgeFunctionPointer
namespace android {
// Android's NativeBridge receives a platform SONAME and resolves it through
// the system linker namespace.  Darwin's hardened dyld rejects a bare
// relative dlopen, so preserve the same namespace lookup with an explicit
// absolute path supplied by the runtime's capability boundary.  The macro
// below routes only libnativebridge's OpenSystemLibrary call through this
// shim; ordinary app ELF loading keeps its own resolver and namespace rules.
void* DarwinNativeBridgeDlopen(const char* path, int flags) {
  if (path != nullptr && path[0] != '\0' && path[0] != '/' &&
      std::string(path).find('/') == std::string::npos &&
      std::string(path).find('\\') == std::string::npos) {
    for (const char* variable : {"DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR",
                                 "DARWIN_ART_APK_APP_NATIVE_DIR"}) {
      const char* directory = std::getenv(variable);
      if (std::getenv("DARWIN_ART_DEBUG_NATIVE_BRIDGE") != nullptr) {
        std::fprintf(stderr, "Darwin NativeBridge search path=%s dir=%s\\n", path,
                     directory == nullptr ? "<none>" : directory);
      }
      if (directory == nullptr || directory[0] != '/') continue;
      const std::string candidate = std::string(directory) + "/" + path;
      struct stat status {};
      if (stat(candidate.c_str(), &status) == 0 && S_ISREG(status.st_mode)) {
        return ::dlopen(candidate.c_str(), flags);
      }
    }
  }
  return ::dlopen(path, flags);
}
extern "C" void* AospNativeBridgeGetTrampoline2(
    void* handle,
    const char* name,
    const char* shorty,
    uint32_t len,
    JNICallType call_type);
}
#define dlopen DarwinNativeBridgeDlopen
#include "../_aosp/art-native-library-control-flow/libnativebridge/native_bridge.cc"
#undef NativeBridgeIsNativeBridgeFunctionPointer
#undef NativeBridgeGetTrampoline2
#undef dlopen

namespace android {
extern "C" {

void* DarwinNativeBridgeGetTrampoline2(void* handle,
                                       const char* name,
                                       const char* shorty,
                                       uint32_t len,
                                       JNICallType call_type);
bool DarwinNativeBridgeIsNativeBridgeFunctionPointer(const void* pointer);

void* NativeBridgeGetTrampoline2(void* handle,
                                 const char* name,
                                 const char* shorty,
                                 uint32_t len,
                                 JNICallType call_type) {
  if (void* trampoline = DarwinNativeBridgeGetTrampoline2(
          handle, name, shorty, len, call_type);
      trampoline != nullptr) {
    return trampoline;
  }
  return AospNativeBridgeGetTrampoline2(handle, name, shorty, len, call_type);
}

bool NativeBridgeIsNativeBridgeFunctionPointer(const void* pointer) {
  return DarwinNativeBridgeIsNativeBridgeFunctionPointer(pointer) ||
         AospNativeBridgeIsNativeBridgeFunctionPointer(pointer);
}

void NativeLoaderFreeErrorMessage(char* message) {
  std::free(message);
}

void InitializeNativeLoader() {
  darwin_art::loader::InitializeClassLoaderNamespaces();
}

void ResetNativeLoader() {
  darwin_art::loader::ResetClassLoaderNamespaces();
}

}  // extern "C"
}  // namespace android
